use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use super::repository::DEFAULT_STORY_TITLE;
use crate::ai;
use crate::features::{
    ledger::{
        model::kind as ledger_kind, reducer as ledger_reducer, repository as ledger_repository,
    },
    settings as global_settings,
};
use crate::prompts;
use crate::shared::db::Pool;

#[derive(Debug, Clone, Serialize)]
struct StoryTitleUpdatedPayload {
    story_id: String,
    title: String,
}

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

/// Auto-title (the ChatGPT/Gemini pattern): once a story has its first
/// exchange, name it with the text model and emit `story-title-updated`. Only
/// runs while the story still carries the placeholder title, so a user rename
/// before or during generation always wins. Best-effort throughout — a
/// failure leaves the placeholder in place and the next exchange retries.
pub fn maybe_auto_title(app: &AppHandle, pool: &Pool, story_id: &str) {
    let Ok(conn) = pool.get() else { return };
    let Ok(title) = conn.query_row("SELECT title FROM stories WHERE id = ?1", [story_id], |r| {
        r.get::<_, String>(0)
    }) else {
        return;
    };
    if title != DEFAULT_STORY_TITLE {
        return;
    }
    // The opening exchange: the earliest one or two visible ledger entries,
    // folded through the reducer so an edit/swipe made before this fires
    // (auto-title only runs once, right after the first exchange) titles
    // from what the player actually sees rather than the discarded original.
    let opening: Vec<(String, String)> = {
        let Ok(raw) = ledger_repository::list_logical_entries(&conn, story_id) else {
            return;
        };
        ledger_reducer::active_visible_entries(&raw)
            .into_iter()
            .take(2)
            .map(|e| (e.kind, e.content.unwrap_or_default()))
            .collect()
    };
    if opening.is_empty() {
        return;
    }

    let Ok(config) = global_settings::resolve_text_model(app, pool) else {
        // No text model configured yet (or no API key): keep the placeholder;
        // a later exchange retries once the model is set up.
        return;
    };

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

    let app = app.clone();
    let pool = pool.clone();
    let story_id = story_id.to_string();
    tauri::async_runtime::spawn(async move {
        let prompt = format!("The story opens:\n\n{opening_text}\n\nGive it a title.");
        let Ok(generated) =
            ai::prompt_typed::<GeneratedTitle>(&config, prompts::TITLE_SYSTEM_PROMPT, prompt).await
        else {
            return;
        };
        let Some(title) = sanitize_title(&generated.title) else {
            return;
        };
        let Ok(conn) = pool.get() else { return };
        // Conditional write: if the user renamed the story while the model was
        // thinking, their title stands and the generated one is dropped.
        let updated = conn.execute(
            "UPDATE stories SET title = ?1 WHERE id = ?2 AND title = ?3",
            rusqlite::params![title, story_id, DEFAULT_STORY_TITLE],
        );
        if matches!(updated, Ok(n) if n > 0) {
            let _ = app.emit(
                "story-title-updated",
                StoryTitleUpdatedPayload { story_id, title },
            );
        }
    });
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
    use super::sanitize_title;

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
