pub mod attributes;
pub mod model;

use chrono::Utc;
use rusqlite::OptionalExtension;
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::features::timeline::{model::kind, repository::append_entry};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};
use model::Entity;

fn row_to_entity(row: &rusqlite::Row) -> rusqlite::Result<Entity> {
    Ok(Entity {
        id: row.get(0)?,
        story_id: row.get(1)?,
        branch_id: row.get(2)?,
        kind: row.get(3)?,
        name: row.get(4)?,
        appearance_anchor: row.get(5)?,
        created_at: row.get(6)?,
    })
}

pub fn list_entities_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    branch_id: &str,
    filter_kind: Option<&str>,
) -> AppResult<Vec<Entity>> {
    let mut sql = "SELECT entities.id, entities.story_id, branch_entity_state.branch_id, entities.kind,
                          branch_entity_state.name, branch_entity_state.appearance_anchor, entities.created_at
                   FROM entities JOIN branch_entity_state ON branch_entity_state.entity_id = entities.id
                   WHERE entities.story_id = ?1 AND branch_entity_state.branch_id = ?2 AND branch_entity_state.is_present = 1".to_string();
    if filter_kind.is_some() {
        sql.push_str(" AND entities.kind = ?3");
    }
    sql.push_str(" ORDER BY entities.created_at ASC");
    let mut stmt = conn.prepare(&sql)?;
    let mut out = Vec::new();
    if let Some(filter_kind) = filter_kind {
        for row in stmt.query_map(
            rusqlite::params![story_id, branch_id, filter_kind],
            row_to_entity,
        )? {
            out.push(row?);
        }
    } else {
        for row in stmt.query_map(rusqlite::params![story_id, branch_id], row_to_entity)? {
            out.push(row?);
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn list_entities(
    pool: State<Pool>,
    story_id: String,
    branch_id: String,
    kind: Option<String>,
) -> AppResult<Vec<Entity>> {
    let conn = pool.get()?;
    list_entities_sync(&conn, &story_id, &branch_id, kind.as_deref())
}

/// Inserts a new entity with a caller-supplied id — split out of
/// `create_entity_sync` so a narrator tool can synthesize an entity's id
/// before this row exists (a later tool call in the same turn may need to
/// reference an entity that's only staged, not yet committed) and reuse the
/// exact same id when the staged write is actually applied.
#[allow(clippy::too_many_arguments)]
pub fn create_entity_with_id_sync(
    conn: &rusqlite::Connection,
    id: &str,
    story_id: &str,
    branch_id: &str,
    entity_kind: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
) -> AppResult<Entity> {
    let branch_story: String = conn
        .query_row(
            "SELECT story_id FROM branches WHERE id = ?1",
            [branch_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("branch {branch_id} not found")))?;
    if branch_story != story_id {
        return Err(AppError::Invalid("branch does not belong to story".into()));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    let now = Utc::now().to_rfc3339();
    let anchor = appearance_anchor.map(str::trim).filter(|s| !s.is_empty());
    conn.execute(
        "INSERT INTO entities (id, story_id, kind, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, story_id, entity_kind, now],
    )?;
    let event = append_entry(
        conn,
        branch_id,
        kind::ENTITY_CREATED,
        "hidden",
        Some(&format!("{name} was added as a {entity_kind}.")),
        &json!({
            "entity_id": id, "kind": entity_kind, "name": name, "appearance_anchor": anchor, "source": source
        }),
        target_entry_id,
    )?;
    conn.execute(
        "INSERT INTO branch_entity_state (branch_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
        rusqlite::params![branch_id, id, name, anchor, now, event.id],
    )?;
    Ok(Entity {
        id: id.to_string(),
        story_id: story_id.into(),
        branch_id: branch_id.into(),
        kind: entity_kind.into(),
        name: name.into(),
        appearance_anchor: anchor.map(str::to_string),
        created_at: now,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn create_entity_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    branch_id: &str,
    entity_kind: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
) -> AppResult<Entity> {
    let id = Uuid::new_v4().to_string();
    create_entity_with_id_sync(
        conn,
        &id,
        story_id,
        branch_id,
        entity_kind,
        name,
        appearance_anchor,
        source,
        target_entry_id,
    )
}

#[tauri::command]
pub fn create_entity(
    pool: State<Pool>,
    story_id: String,
    branch_id: String,
    kind: String,
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let entity = create_entity_sync(
        &tx,
        &story_id,
        &branch_id,
        &kind,
        &name,
        appearance_anchor.as_deref(),
        "user",
        None,
    )?;
    tx.commit()?;
    Ok(entity)
}

/// Renames/updates an entity's appearance, mirroring `create_entity_sync`'s
/// `source`/`target_entry_id` shape so both the user-facing command and a
/// narrator tool's staged commit can call it identically.
pub fn update_entity_sync(
    conn: &rusqlite::Connection,
    branch_id: &str,
    entity_id: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
) -> AppResult<Entity> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    let before: Entity = conn.query_row(
        "SELECT entities.id, entities.story_id, branch_entity_state.branch_id, entities.kind, branch_entity_state.name,
                branch_entity_state.appearance_anchor, entities.created_at
         FROM entities JOIN branch_entity_state ON branch_entity_state.entity_id = entities.id
         WHERE entities.id = ?1 AND branch_entity_state.branch_id = ?2 AND branch_entity_state.is_present = 1",
        rusqlite::params![entity_id, branch_id], row_to_entity,
    ).optional()?.ok_or_else(|| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let anchor = appearance_anchor.map(str::trim).filter(|s| !s.is_empty());
    let event = append_entry(
        conn,
        branch_id,
        kind::ENTITY_UPDATED,
        "hidden",
        Some(&format!(
            "{} is now named {name}; appearance details were updated.",
            before.name
        )),
        &json!({
            "entity_id": entity_id, "before": {"name": before.name, "appearance_anchor": before.appearance_anchor},
            "after": {"name": name, "appearance_anchor": anchor}, "source": source
        }),
        target_entry_id,
    )?;
    let now = Utc::now().to_rfc3339();
    conn.execute("UPDATE branch_entity_state SET name = ?1, appearance_anchor = ?2, updated_at = ?3, last_event_id = ?4 WHERE branch_id = ?5 AND entity_id = ?6",
        rusqlite::params![name, anchor, now, event.id, branch_id, entity_id])?;
    Ok(Entity {
        name: name.into(),
        appearance_anchor: anchor.map(str::to_string),
        ..before
    })
}

#[tauri::command]
pub fn update_entity(
    pool: State<Pool>,
    branch_id: String,
    entity_id: String,
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let entity = update_entity_sync(
        &tx,
        &branch_id,
        &entity_id,
        &name,
        appearance_anchor.as_deref(),
        "user",
        None,
    )?;
    tx.commit()?;
    Ok(entity)
}

#[tauri::command]
pub fn delete_entity(pool: State<Pool>, branch_id: String, entity_id: String) -> AppResult<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let name: String = tx.query_row("SELECT name FROM branch_entity_state WHERE branch_id = ?1 AND entity_id = ?2 AND is_present = 1", rusqlite::params![branch_id, entity_id], |r| r.get(0))
        .map_err(|_| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let event = append_entry(
        &tx,
        &branch_id,
        kind::ENTITY_DELETED,
        "hidden",
        Some(&format!(
            "{name} was removed from the authoritative entity state."
        )),
        &json!({"entity_id": entity_id, "name": name, "source": "user"}),
        None,
    )?;
    tx.execute("UPDATE branch_entity_state SET is_present = 0, updated_at = ?1, last_event_id = ?2 WHERE branch_id = ?3 AND entity_id = ?4", rusqlite::params![Utc::now().to_rfc3339(), event.id, branch_id, entity_id])?;
    tx.commit()?;
    Ok(())
}
