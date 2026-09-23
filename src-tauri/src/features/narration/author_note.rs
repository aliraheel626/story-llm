//! Per-story author's note storage and message-context injection.

use chrono::Utc;
use serde_json::{json, Value};
use tauri::State;

use crate::features::ledger::repository;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

fn story_settings(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Value> {
    let raw: String = conn
        .query_row(
            "SELECT settings_json FROM stories WHERE id = ?1",
            [story_id],
            |row| row.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("story {story_id} not found")))?;
    Ok(serde_json::from_str(&raw).unwrap_or_else(|_| json!({})))
}

fn read_author_note(pool: &Pool, story_id: &str) -> AppResult<String> {
    let conn = pool.get()?;
    let settings = story_settings(&conn, story_id)?;
    Ok(settings
        .get("author_note")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string())
}

fn write_author_note(pool: &Pool, story_id: &str, note: &str) -> AppResult<()> {
    let note = note.trim();
    with_transaction(pool, |tx| {
        let mut settings = story_settings(tx, story_id)?;
        if note.is_empty() {
            if let Some(object) = settings.as_object_mut() {
                object.remove("author_note");
            }
        } else {
            settings["author_note"] = json!(note);
        }
        tx.execute(
            "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
        )?;
        // The prior app version reads these events; current narration history ignores them.
        repository::append_entry(
            tx,
            story_id,
            "context_note_updated",
            "hidden",
            Some(&format!("Author's note was updated: {note}")),
            &json!({"author_note": note}),
            None,
        )?;
        Ok(())
    })
}

fn read_author_note_enabled(pool: &Pool, story_id: &str) -> AppResult<bool> {
    let conn = pool.get()?;
    let settings = story_settings(&conn, story_id)?;
    Ok(settings
        .get("author_note_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true))
}

fn write_author_note_enabled(pool: &Pool, story_id: &str, enabled: bool) -> AppResult<()> {
    with_transaction(pool, |tx| {
        let mut settings = story_settings(tx, story_id)?;
        settings["author_note_enabled"] = json!(enabled);
        tx.execute(
            "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
        )?;
        Ok(())
    })
}

#[tauri::command]
pub fn get_author_note(pool: State<Pool>, story_id: String) -> AppResult<String> {
    read_author_note(pool.inner(), &story_id)
}

#[tauri::command]
pub fn save_author_note(pool: State<Pool>, story_id: String, note: String) -> AppResult<()> {
    write_author_note(pool.inner(), &story_id, &note)
}

#[tauri::command]
pub fn get_author_note_enabled(pool: State<Pool>, story_id: String) -> AppResult<bool> {
    read_author_note_enabled(pool.inner(), &story_id)
}

#[tauri::command]
pub fn set_author_note_enabled(
    pool: State<Pool>,
    story_id: String,
    enabled: bool,
) -> AppResult<()> {
    write_author_note_enabled(pool.inner(), &story_id, enabled)
}

/// Tagged per-message note, or an empty block when muted or blank.
pub(super) fn context_block(pool: &Pool, story_id: &str) -> AppResult<String> {
    let conn = pool.get()?;
    let settings = story_settings(&conn, story_id)?;
    let enabled = settings
        .get("author_note_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let note = settings
        .get("author_note")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    Ok(if enabled && !note.is_empty() {
        format!("<author_note>{note}</author_note>")
    } else {
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_round_trips_through_story_settings_and_can_be_muted() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        drop(conn);

        write_author_note(&pool, "s", "  Keep it terse.  ").unwrap();
        assert_eq!(read_author_note(&pool, "s").unwrap(), "Keep it terse.");
        assert_eq!(
            context_block(&pool, "s").unwrap(),
            "<author_note>Keep it terse.</author_note>"
        );

        write_author_note_enabled(&pool, "s", false).unwrap();
        assert!(!read_author_note_enabled(&pool, "s").unwrap());
        assert_eq!(context_block(&pool, "s").unwrap(), "");

        write_author_note_enabled(&pool, "s", true).unwrap();
        write_author_note(&pool, "s", "   ").unwrap();
        assert_eq!(read_author_note(&pool, "s").unwrap(), "");
        assert_eq!(context_block(&pool, "s").unwrap(), "");
        let settings = story_settings(&pool.get().unwrap(), "s").unwrap();
        assert!(settings.get("author_note").is_none());
        let conn = pool.get().unwrap();
        let latest_note: String = conn
            .query_row(
                "SELECT payload_json FROM ledger_entries WHERE story_id = 's' AND kind = 'context_note_updated' ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&latest_note).unwrap()["author_note"],
            ""
        );
    }
}
