use chrono::Utc;
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::db::Pool;
use crate::error::{AppError, AppResult};
use crate::models::Story;

fn read_settings_json(pool: &State<Pool>, story_id: &str) -> AppResult<serde_json::Value> {
    let conn = pool.get()?;
    let raw: String = conn
        .query_row("SELECT settings_json FROM stories WHERE id = ?1", [story_id], |r| r.get(0))
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

#[tauri::command]
pub fn create_story(pool: State<Pool>, title: String) -> AppResult<Story> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Invalid("title must not be empty".into()));
    }
    let mut conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    let story_id = Uuid::new_v4().to_string();
    let branch_id = Uuid::new_v4().to_string();

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO stories (id, title, created_at, updated_at, settings_json, default_branch_id)
         VALUES (?1, ?2, ?3, ?3, '{}', NULL)",
        rusqlite::params![story_id, title, now],
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
        settings_json: "{}".to_string(),
        default_branch_id: Some(branch_id),
    })
}

/// Author's Note (spec §6.6): a persistent instruction folded into every
/// narration call's preamble for this story — tone, style, ongoing
/// constraints, whatever the player wants the narrator to keep in mind.
#[tauri::command]
pub fn get_author_note(pool: State<Pool>, story_id: String) -> AppResult<String> {
    let settings = read_settings_json(&pool, &story_id)?;
    Ok(settings.get("author_note").and_then(|v| v.as_str()).unwrap_or("").to_string())
}

#[tauri::command]
pub fn save_author_note(pool: State<Pool>, story_id: String, note: String) -> AppResult<()> {
    let mut settings = read_settings_json(&pool, &story_id)?;
    settings["author_note"] = json!(note.trim());
    let conn = pool.get()?;
    conn.execute("UPDATE stories SET settings_json = ?1 WHERE id = ?2", rusqlite::params![settings.to_string(), story_id])?;
    Ok(())
}

pub fn read_author_note(pool: &Pool, story_id: &str) -> AppResult<Option<String>> {
    let conn = pool.get()?;
    let raw: String = conn
        .query_row("SELECT settings_json FROM stories WHERE id = ?1", [story_id], |r| r.get(0))
        .map_err(|_| AppError::NotFound(format!("story {story_id} not found")))?;
    let settings: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let note = settings.get("author_note").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    Ok(if note.is_empty() { None } else { Some(note) })
}
