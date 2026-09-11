use chrono::Utc;
use tauri::State;
use uuid::Uuid;

use crate::db::Pool;
use crate::error::{AppError, AppResult};
use crate::models::Entity;

fn row_to_entity(row: &rusqlite::Row) -> rusqlite::Result<Entity> {
    Ok(Entity {
        id: row.get(0)?,
        story_id: row.get(1)?,
        kind: row.get(2)?,
        name: row.get(3)?,
        card_json: row.get(4)?,
        appearance_anchor: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[tauri::command]
pub fn list_entities(pool: State<Pool>, story_id: String, kind: Option<String>) -> AppResult<Vec<Entity>> {
    let conn = pool.get()?;
    let mut out = Vec::new();
    if let Some(kind) = kind {
        let mut stmt = conn.prepare(
            "SELECT id, story_id, kind, name, card_json, appearance_anchor, created_at
             FROM entities WHERE story_id = ?1 AND kind = ?2 ORDER BY created_at ASC",
        )?;
        for r in stmt.query_map(rusqlite::params![story_id, kind], row_to_entity)? {
            out.push(r?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, story_id, kind, name, card_json, appearance_anchor, created_at
             FROM entities WHERE story_id = ?1 ORDER BY created_at ASC",
        )?;
        for r in stmt.query_map([story_id], row_to_entity)? {
            out.push(r?);
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn create_entity(
    pool: State<Pool>,
    story_id: String,
    kind: String,
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    let conn = pool.get()?;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let anchor = appearance_anchor.as_deref().map(str::trim).filter(|s| !s.is_empty());
    conn.execute(
        "INSERT INTO entities (id, story_id, kind, name, card_json, appearance_anchor, created_at)
         VALUES (?1, ?2, ?3, ?4, '{}', ?5, ?6)",
        rusqlite::params![id, story_id, kind, name, anchor, now],
    )?;
    Ok(Entity {
        id,
        story_id,
        kind,
        name: name.to_string(),
        card_json: "{}".to_string(),
        appearance_anchor: anchor.map(str::to_string),
        created_at: now,
    })
}

#[tauri::command]
pub fn update_entity(
    pool: State<Pool>,
    entity_id: String,
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    let conn = pool.get()?;
    let anchor = appearance_anchor.as_deref().map(str::trim).filter(|s| !s.is_empty());
    conn.execute(
        "UPDATE entities SET name = ?1, appearance_anchor = ?2 WHERE id = ?3",
        rusqlite::params![name, anchor, entity_id],
    )?;
    conn.query_row(
        "SELECT id, story_id, kind, name, card_json, appearance_anchor, created_at FROM entities WHERE id = ?1",
        [&entity_id],
        row_to_entity,
    )
    .map_err(|_| AppError::NotFound(format!("entity {entity_id} not found")))
}

#[tauri::command]
pub fn delete_entity(pool: State<Pool>, entity_id: String) -> AppResult<()> {
    let conn = pool.get()?;
    conn.execute("DELETE FROM entities WHERE id = ?1", [&entity_id])?;
    Ok(())
}
