pub mod model;

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::ai;
use crate::features::settings;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};
use model::Story;

/// Placeholder shown until the model (or the user) supplies a real title —
/// mirrors `DEFAULT_STORY_TITLE` in `src/lib/types.ts`.
pub const DEFAULT_STORY_TITLE: &str = "New story";

fn read_settings_json(pool: &State<Pool>, story_id: &str) -> AppResult<serde_json::Value> {
    let conn = pool.get()?;
    let raw: String = conn
        .query_row(
            "SELECT settings_json FROM stories WHERE id = ?1",
            [story_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("story {story_id} not found")))?;
    Ok(serde_json::from_str(&raw).unwrap_or_else(|_| json!({})))
}

#[tauri::command]
pub fn list_stories(pool: State<Pool>) -> AppResult<Vec<Story>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, settings_json, default_branch_id
         FROM stories ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Story {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            settings_json: row.get(4)?,
            default_branch_id: row.get(5)?,
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
    let title = title.unwrap_or_default();
    let title = title.trim();
    let title = if title.is_empty() {
        DEFAULT_STORY_TITLE
    } else {
        title
    };
    let settings_json = settings.unwrap_or_else(|| json!({})).to_string();
    let mut conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    let story_id = Uuid::new_v4().to_string();
    let branch_id = Uuid::new_v4().to_string();

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO stories (id, title, created_at, updated_at, settings_json, default_branch_id)
         VALUES (?1, ?2, ?3, ?3, ?4, NULL)",
        rusqlite::params![story_id, title, now, settings_json],
    )?;
    tx.execute(
        "INSERT INTO branches (id, story_id, parent_branch_id, forked_at_passage_id, name, created_at)
         VALUES (?1, ?2, NULL, NULL, 'main', ?3)",
        rusqlite::params![branch_id, story_id, now],
    )?;
    tx.execute(
        "UPDATE stories SET default_branch_id = ?1 WHERE id = ?2",
        rusqlite::params![branch_id, story_id],
    )?;
    tx.commit()?;

    Ok(Story {
        id: story_id,
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
        settings_json,
        default_branch_id: Some(branch_id),
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
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let paths: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT images.path FROM images
             JOIN passages ON passages.id = images.passage_id
             JOIN branches ON branches.id = passages.branch_id
             WHERE branches.story_id = ?1",
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
    tx.commit()?;

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

const TITLE_PREAMBLE: &str = "You name interactive stories. Given the opening of a story, give it \
a short, evocative title of 2 to 5 words that fits its tone and language. Respond with the \
structured output only.";

/// Characters of opening text folded into the title prompt — enough for the
/// model to name the story, capped so a long first passage doesn't balloon
/// the request.
const TITLE_INPUT_LIMIT: usize = 2_000;
/// Hard cap on the persisted title, so a runaway model can't produce an
/// unusable sidebar entry.
const TITLE_MAX_CHARS: usize = 60;

/// Auto-title (the ChatGPT/Gemini pattern): once a story has its first
/// passage, name it with the text model and emit `story-title-updated`. Only
/// runs while the story still carries the placeholder title, so a user rename
/// before or during generation always wins. Best-effort throughout — a
/// failure leaves the placeholder in place and the next passage retries.
pub fn maybe_auto_title(app: &AppHandle, pool: &Pool, story_id: &str) {
    let Ok(conn) = pool.get() else { return };
    let Ok((title, branch_id)) = conn.query_row(
        "SELECT title, default_branch_id FROM stories WHERE id = ?1",
        [story_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    ) else {
        return;
    };
    if title != DEFAULT_STORY_TITLE {
        return;
    }
    let Some(branch_id) = branch_id else { return };

    // The opening exchange: whatever the earliest one or two passages are
    // (player turn + narration, a Story-mode passage, or a continued scene).
    let opening = {
        let Ok(mut stmt) = conn.prepare(
            "SELECT role, content FROM passages WHERE branch_id = ?1 ORDER BY seq ASC LIMIT 2",
        ) else {
            return;
        };
        let rows = match stmt.query_map([&branch_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            Ok(rows) => rows,
            Err(_) => return,
        };
        match rows.collect::<rusqlite::Result<Vec<_>>>() {
            Ok(v) => v,
            Err(_) => return,
        }
    };
    if opening.is_empty() {
        return;
    }

    let Ok(config) = settings::resolve_text_model(app, pool) else {
        // No text model configured yet (or no API key): keep the placeholder;
        // a later passage retries once the model is set up.
        return;
    };

    let opening_text: String = opening
        .iter()
        .map(|(role, content)| {
            format!(
                "{}: {}",
                if role == "player" {
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
            ai::prompt_typed::<GeneratedTitle>(&config, TITLE_PREAMBLE, prompt).await
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

/// Author's Note (spec §6.6): a persistent instruction folded into every
/// narration call's preamble for this story — tone, style, ongoing
/// constraints, whatever the player wants the narrator to keep in mind.
#[tauri::command]
pub fn get_author_note(pool: State<Pool>, story_id: String) -> AppResult<String> {
    let settings = read_settings_json(&pool, &story_id)?;
    Ok(settings
        .get("author_note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

#[tauri::command]
pub fn save_author_note(pool: State<Pool>, story_id: String, note: String) -> AppResult<()> {
    let mut settings = read_settings_json(&pool, &story_id)?;
    settings["author_note"] = json!(note.trim());
    let conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![settings.to_string(), now, story_id],
    )?;
    Ok(())
}

pub fn read_author_note(pool: &Pool, story_id: &str) -> AppResult<Option<String>> {
    let conn = pool.get()?;
    let raw: String = conn
        .query_row(
            "SELECT settings_json FROM stories WHERE id = ?1",
            [story_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("story {story_id} not found")))?;
    let settings: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let note = settings
        .get("author_note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(if note.is_empty() { None } else { Some(note) })
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
