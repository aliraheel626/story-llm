pub mod author_note;
mod commands;
pub mod model;
pub mod settings;

pub use commands::*;

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use self::settings::{normalize_reasoning_effort, NarratorToolSettings};
use crate::ai;
use crate::features::{
    ledger::{
        model::kind as ledger_kind, reducer as ledger_reducer, repository as ledger_repository,
    },
    settings as global_settings,
};
use crate::prompts;
use crate::shared::db::{seed_player_entity, with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};
use model::Story;

/// Placeholder shown until the model (or the user) supplies a real title —
/// mirrors `DEFAULT_STORY_TITLE` in `src/lib/types.ts`.
pub const DEFAULT_STORY_TITLE: &str = "New story";

#[tauri::command]
pub fn list_stories(pool: State<Pool>) -> AppResult<Vec<Story>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, settings_json
         FROM stories ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Story {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            settings_json: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// `settings` carries mechanics choices made while the story was still a
/// frontend draft, so they are written in the same transaction as the story
/// itself rather than a follow-up save that could fail on its own.
#[tauri::command]
pub fn create_story(
    pool: State<Pool>,
    title: Option<String>,
    settings: Option<serde_json::Value>,
) -> AppResult<Story> {
    create_story_in_pool(pool.inner(), title, settings)
}

fn create_story_in_pool(
    pool: &Pool,
    title: Option<String>,
    settings: Option<serde_json::Value>,
) -> AppResult<Story> {
    let title = title.unwrap_or_default();
    let title = title.trim();
    let title = if title.is_empty() {
        DEFAULT_STORY_TITLE
    } else {
        title
    };
    let mut settings = settings.unwrap_or_else(|| json!({}));
    let object = settings
        .as_object_mut()
        .ok_or_else(|| AppError::Invalid("story settings must be an object".into()))?;
    object.remove("attributes_enabled");
    object.remove("dice_mode");
    let tools = match object.get("narrator_tools") {
        Some(value) => serde_json::from_value::<NarratorToolSettings>(value.clone())
            .map_err(|error| AppError::Invalid(format!("invalid narrator tools: {error}")))?,
        None => NarratorToolSettings::default(),
    };
    object.insert("narrator_tools".into(), json!(tools));
    if let Some(value) = object.get("reasoning_effort") {
        match value {
            serde_json::Value::Null => {
                object.remove("reasoning_effort");
            }
            serde_json::Value::String(effort) if effort.trim().is_empty() => {
                object.remove("reasoning_effort");
            }
            serde_json::Value::String(effort) => {
                let normalized = normalize_reasoning_effort(effort).ok_or_else(|| {
                    AppError::Invalid(format!("unsupported reasoning effort: {effort}"))
                })?;
                object.insert("reasoning_effort".into(), json!(normalized));
            }
            _ => {
                return Err(AppError::Invalid(
                    "reasoning effort must be a string".into(),
                ))
            }
        }
    }
    let settings_json = settings.to_string();
    let now = Utc::now().to_rfc3339();
    let story_id = Uuid::new_v4().to_string();
    with_transaction(pool, |tx| {
        tx.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![story_id, title, now, settings_json],
        )?;
        seed_player_entity(tx, &story_id)
    })?;

    Ok(Story {
        id: story_id,
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
        settings_json,
    })
}

/// Click-to-edit rename from the story header. Auto-titling never overwrites
/// a title that isn't the placeholder, so any rename permanently ends it.
/// Returns nothing — the caller already knows the new title.
#[tauri::command]
pub fn rename_story(pool: State<Pool>, story_id: String, title: String) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Invalid("title must not be empty".into()));
    }
    let conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    let updated = conn.execute(
        "UPDATE stories SET title = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![title, now, story_id],
    )?;
    if updated == 0 {
        return Err(AppError::NotFound(format!("story {story_id} not found")));
    }
    Ok(())
}

/// Collects image file paths before the cascade delete removes their rows
/// (SQLite FK cascade cleans up every table, but can't touch files on disk).
#[tauri::command]
pub fn delete_story(pool: State<Pool>, story_id: String) -> AppResult<()> {
    let paths = with_transaction(pool.inner(), |tx| {
        let paths: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT image_assets.path FROM image_assets
                 JOIN ledger_entries ON ledger_entries.id = image_assets.entry_id
                 WHERE ledger_entries.story_id = ?1",
            )?;
            let paths = stmt
                .query_map([&story_id], |row| row.get(0))?
                .filter_map(Result::ok)
                .collect();
            paths
        };

        let deleted = tx.execute("DELETE FROM stories WHERE id = ?1", [&story_id])?;
        if deleted == 0 {
            return Err(AppError::NotFound(format!("story {story_id} not found")));
        }
        Ok(paths)
    })?;

    for path in paths {
        let _ = std::fs::remove_file(path); // best-effort; a missing file shouldn't fail the delete
    }

    Ok(())
}

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
    use super::settings::NarratorToolSettings;
    use super::{create_story_in_pool, sanitize_title};
    use crate::shared::db::test_pool;
    use serde_json::json;

    #[test]
    fn story_creation_validates_draft_and_seeds_canonical_player() {
        let pool = test_pool();
        let tools = NarratorToolSettings {
            roll_check: false,
            ..NarratorToolSettings::default()
        };
        let created = create_story_in_pool(
            &pool,
            None,
            Some(json!({
                "narrator_tools": tools,
                "reasoning_effort": "HIGH",
                "author_note": "keep",
                "attributes_enabled": false,
                "dice_mode": "never"
            })),
        )
        .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&created.settings_json).unwrap();
        assert_eq!(settings["narrator_tools"], json!(tools));
        assert_eq!(settings["reasoning_effort"], "high");
        assert_eq!(settings["author_note"], "keep");
        assert!(settings.get("attributes_enabled").is_none());
        assert!(settings.get("dice_mode").is_none());
        let conn = pool.get().unwrap();
        let (name, kind): (String, String) = conn
            .query_row(
                "SELECT story_entity_state.name, entities.kind FROM story_entity_state
             JOIN entities ON entities.id = story_entity_state.entity_id
             WHERE story_entity_state.story_id = ?1",
                [&created.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((name.as_str(), kind.as_str()), ("You", "character"));
        drop(conn);
        let fresh = create_story_in_pool(&pool, None, None).unwrap();
        let defaults: serde_json::Value = serde_json::from_str(&fresh.settings_json).unwrap();
        assert_eq!(
            defaults["narrator_tools"],
            json!(NarratorToolSettings::default())
        );
        assert!(create_story_in_pool(
            &pool,
            None,
            Some(json!({"narrator_tools":{"roll_check":true}}))
        )
        .is_err());
        let count: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM stories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
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
