use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use tauri::AppHandle;

use super::repository::DEFAULT_STORY_TITLE;
use crate::ai;
use crate::features::{
    ledger::{
        model::kind as ledger_kind, query as ledger_query, turns,
    },
    settings as global_settings,
    stats::model::{UsageKind, UsageRecord},
    turn::TurnTx,
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
/// Titling is attempted on a story's first three turns only. Each attempt runs
/// inside the turn, so a title model that keeps failing would otherwise make
/// every later turn wait for it.
const TITLE_ATTEMPT_TURNS: usize = 3;

/// The current turn's row already exists, so it is included in the limit.
fn needs_title(conn: &rusqlite::Connection, story_id: &str) -> AppResult<bool> {
    let title: String = conn.query_row(
        "SELECT title FROM stories WHERE id = ?1",
        [story_id],
        |row| row.get(0),
    )?;
    Ok(title == DEFAULT_STORY_TITLE
        && turns::list_summaries(conn, story_id)?.len() <= TITLE_ATTEMPT_TURNS)
}

/// Read the first two active entries from the same connection as the turn,
/// including the narration that has not been committed yet.
pub fn opening_exchange(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<(String, String)>> {
    Ok(ledger_query::select(conn, story_id, &Default::default())?
        .into_iter()
        .filter(|entry| entry.kind == ledger_kind::PLAYER_MESSAGE || entry.kind == ledger_kind::NARRATION)
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
            if needs_title(conn, story_id)? {
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
    let (generated, usage) = tokio::time::timeout(
        Duration::from_secs(30),
        ai::prompt_typed::<GeneratedTitle>(&config, prompts::TITLE_SYSTEM_PROMPT, prompt),
    )
    .await
    .ok()?;
    for call in usage {
        turn.record_usage(UsageRecord::text(UsageKind::Title, &config, call));
    }
    let generated = generated.ok()?;
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
    use super::{needs_title, opening_exchange, sanitize_title, write_title};
    use crate::features::ledger::{model::kind, repository as ledger_repository, turns};
    use crate::features::turn::{TurnGate, TurnTx};
    use crate::shared::db::test_pool;

    #[test]
    fn title_is_tried_only_on_the_first_three_turns() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'New story', 'now', 'now')",
            [],
        )
        .unwrap();
        for _ in 0..3 {
            turns::create_turn(&conn, "s").unwrap();
        }
        assert!(needs_title(&conn, "s").unwrap());
        turns::create_turn(&conn, "s").unwrap();
        assert!(!needs_title(&conn, "s").unwrap());
    }

    #[test]
    fn renamed_story_needs_no_title() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'My title', 'now', 'now')",
            [],
        )
        .unwrap();
        turns::create_turn(&conn, "s").unwrap();
        assert!(!needs_title(&conn, "s").unwrap());
    }

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
