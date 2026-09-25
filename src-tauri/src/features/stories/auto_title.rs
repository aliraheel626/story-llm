use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use tauri::AppHandle;

use super::repository::DEFAULT_STORY_TITLE;
use crate::ai;
use crate::features::{
    ledger::{
        model::kind as ledger_kind, reducer as ledger_reducer, repository as ledger_repository,
        turn_tx::TurnTx,
    },
    settings as global_settings,
};
use crate::prompts;
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct GeneratedTitle {
    /// A short, evocative title of 2-5 words.
    title: String,
}

/// Characters of opening text folded into the title prompt — enough for the
/// model to name the story, capped so a long opening entry doesn't balloon
/// the request.
const TITLE_INPUT_LIMIT: usize = 2_000;
/// Hard cap on the persisted title, so a runaway model can't produce an
/// unusable sidebar entry.
const TITLE_MAX_CHARS: usize = 60;

/// Read the first two active entries from the same connection as the turn,
/// including the narration that has not been committed yet.
pub fn opening_exchange(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<(String, String)>> {
    let raw = ledger_repository::list_logical_entries(conn, story_id)?;
    Ok(ledger_reducer::active_visible_entries(&raw)
        .into_iter()
        .take(2)
        .map(|e| (e.kind, e.content.unwrap_or_default()))
        .collect())
}

/// A rename made before the generated title is written always wins.
pub fn write_title(conn: &rusqlite::Connection, story_id: &str, title: &str) -> AppResult<bool> {
    Ok(conn.execute(
        "UPDATE stories SET title = ?1 WHERE id = ?2 AND title = ?3",
        rusqlite::params![title, story_id, DEFAULT_STORY_TITLE],
    )? > 0)
}

/// Best-effort title generation within the narration turn. The caller emits
/// the update only after committing the transaction.
pub async fn title_in_turn(app: &AppHandle, settings_pool: &Pool, turn: &TurnTx) -> Option<String> {
    let story_id = turn.story_id();
    let opening = turn
        .with(|conn| {
            let title: String = conn.query_row(
                "SELECT title FROM stories WHERE id = ?1",
                [story_id],
                |row| row.get(0),
            )?;
            if title == DEFAULT_STORY_TITLE {
                opening_exchange(conn, story_id)
            } else {
                Ok(Vec::new())
            }
        })
        .await
        .ok()?;
    if opening.is_empty() {
        return None;
    }

    let config = global_settings::resolve_text_model(app, settings_pool).ok()?;

    let opening_text: String = opening
        .iter()
        .map(|(role, content)| {
            format!(
                "{}: {}",
                if role == ledger_kind::PLAYER_MESSAGE {
                    "Player"
                } else {
                    "Narrator"
                },
                content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
        .chars()
        .take(TITLE_INPUT_LIMIT)
        .collect();

    let prompt = format!("The story opens:\n\n{opening_text}\n\nGive it a title.");
    let generated = tokio::time::timeout(
        Duration::from_secs(30),
        ai::prompt_typed::<GeneratedTitle>(&config, prompts::TITLE_SYSTEM_PROMPT, prompt),
    )
    .await
    .ok()?
    .ok()?;
    let title = sanitize_title(&generated.title)?;
    match turn.with(|conn| write_title(conn, story_id, &title)).await {
        Ok(true) => Some(title),
        Ok(false) => None,
        Err(error) => {
            log::error!("auto-title write failed for story {story_id}: {error}");
            None
        }
    }
}

fn sanitize_title(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .trim_matches(|c: char| c.is_whitespace() || "\"'“”‘’.".contains(c))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() || cleaned == DEFAULT_STORY_TITLE {
        return None;
    }
    Some(cleaned.chars().take(TITLE_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::{opening_exchange, sanitize_title, write_title};
    use crate::features::ledger::{
        model::kind,
        repository as ledger_repository,
        turn_tx::{TurnGate, TurnTx},
    };
    use crate::shared::db::test_pool;

    #[tokio::test]
    async fn title_rollback_leaves_placeholder_in_pool() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'New story', 'now', 'now')",
            [],
        )
        .unwrap();
        drop(conn);

        let turn = TurnTx::begin(&pool, &TurnGate::default(), "s").unwrap();
        assert!(turn
            .with(|conn| write_title(conn, "s", "The Iron Crown"))
            .await
            .unwrap());
        turn.rollback().await.unwrap();
        let title: String = pool
            .get()
            .unwrap()
            .query_row("SELECT title FROM stories WHERE id = 's'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(title, "New story");
    }

    #[tokio::test]
    async fn rename_wins_when_title_is_not_placeholder() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'New story', 'now', 'now')",
            [],
        )
        .unwrap();
        drop(conn);

        let turn = TurnTx::begin(&pool, &TurnGate::default(), "s").unwrap();
        turn.with(|conn| {
            conn.execute("UPDATE stories SET title = 'My title' WHERE id = 's'", [])?;
            assert!(!write_title(conn, "s", "Generated title")?);
            Ok(())
        })
        .await
        .unwrap();
        turn.commit().await.unwrap();
        let title: String = pool
            .get()
            .unwrap()
            .query_row("SELECT title FROM stories WHERE id = 's'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(title, "My title");
    }

    #[tokio::test]
    async fn opening_exchange_sees_uncommitted_narration() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'New story', 'now', 'now')",
            [],
        )
        .unwrap();
        drop(conn);

        let turn = TurnTx::begin(&pool, &TurnGate::default(), "s").unwrap();
        let opening = turn
            .with(|conn| {
                ledger_repository::append_story_message(
                    conn,
                    "s",
                    "player",
                    "do",
                    "Open the gate",
                    None,
                    None,
                )?;
                ledger_repository::append_story_message(
                    conn,
                    "s",
                    "narrator",
                    "generated",
                    "A bell rings",
                    None,
                    None,
                )?;
                opening_exchange(conn, "s")
            })
            .await
            .unwrap();
        assert_eq!(
            opening,
            vec![
                (
                    kind::PLAYER_MESSAGE.to_string(),
                    "Open the gate".to_string()
                ),
                (kind::NARRATION.to_string(), "A bell rings".to_string()),
            ]
        );
        assert!(
            ledger_repository::list_logical_entries(&pool.get().unwrap(), "s")
                .unwrap()
                .is_empty()
        );
        turn.rollback().await.unwrap();
    }

    #[test]
    fn sanitize_title_strips_quotes_punctuation_and_whitespace() {
        assert_eq!(
            sanitize_title("  \"The Iron Crown.\" ").as_deref(),
            Some("The Iron Crown")
        );
        assert_eq!(
            sanitize_title("‘Salt   and Pine’").as_deref(),
            Some("Salt and Pine")
        );
        assert_eq!(
            sanitize_title("...Whispers at Dusk...").as_deref(),
            Some("Whispers at Dusk")
        );
    }

    #[test]
    fn sanitize_title_rejects_empty_and_placeholder() {
        assert_eq!(sanitize_title(""), None);
        assert_eq!(sanitize_title("   "), None);
        assert_eq!(sanitize_title("\"\""), None);
        assert_eq!(sanitize_title("New story"), None);
    }

    #[test]
    fn sanitize_title_caps_length() {
        let long = "a".repeat(200);
        let out = sanitize_title(&long).unwrap();
        assert_eq!(out.chars().count(), 60);
    }
}
