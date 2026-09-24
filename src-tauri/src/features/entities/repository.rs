use chrono::Utc;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

use super::{
    events::{EntityEvent, NameAnchor},
    model::Entity,
    projection,
};

fn row_to_entity(row: &rusqlite::Row) -> rusqlite::Result<Entity> {
    Ok(Entity {
        id: row.get(0)?,
        story_id: row.get(1)?,
        kind: row.get(2)?,
        name: row.get(3)?,
        appearance_anchor: row.get(4)?,
        created_at: row.get(5)?,
    })
}

pub fn list_entities_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    filter_kind: Option<&str>,
) -> AppResult<Vec<Entity>> {
    let mut sql = "SELECT entities.id, entities.story_id, entities.kind,
                          story_entity_state.name, story_entity_state.appearance_anchor, entities.created_at
                   FROM entities JOIN story_entity_state ON story_entity_state.entity_id = entities.id
                   WHERE entities.story_id = ?1 AND story_entity_state.story_id = ?1 AND story_entity_state.is_present = 1".to_string();
    if filter_kind.is_some() {
        sql.push_str(" AND entities.kind = ?2");
    }
    sql.push_str(" ORDER BY entities.created_at ASC");
    let mut stmt = conn.prepare(&sql)?;
    let mut out = Vec::new();
    if let Some(filter_kind) = filter_kind {
        for row in stmt.query_map(rusqlite::params![story_id, filter_kind], row_to_entity)? {
            out.push(row?);
        }
    } else {
        for row in stmt.query_map([story_id], row_to_entity)? {
            out.push(row?);
        }
    }
    Ok(out)
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
    entity_kind: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
) -> AppResult<Entity> {
    create_entity_with_id_in_turn(
        conn,
        id,
        story_id,
        entity_kind,
        name,
        appearance_anchor,
        source,
        target_entry_id,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_entity_with_id_in_turn(
    conn: &rusqlite::Connection,
    id: &str,
    story_id: &str,
    entity_kind: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    let now = Utc::now().to_rfc3339();
    let anchor = appearance_anchor.map(str::trim).filter(|s| !s.is_empty());
    let event = EntityEvent::Created {
        entity_id: id.to_string(),
        kind: entity_kind.to_string(),
        name: name.to_string(),
        appearance_anchor: anchor.map(str::to_string),
        source: source.to_string(),
        created_at: now.clone(),
    };
    let entry = projection::record(
        conn,
        story_id,
        target_entry_id,
        &format!("{name} was added as a {entity_kind}."),
        &event,
        turn_id,
    )?;
    Ok(Entity {
        id: id.to_string(),
        story_id: story_id.into(),
        kind: entity_kind.into(),
        name: name.into(),
        appearance_anchor: anchor.map(str::to_string),
        created_at: entry.created_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn create_entity_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
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
        entity_kind,
        name,
        appearance_anchor,
        source,
        target_entry_id,
    )
}

/// Renames/updates an entity's appearance, mirroring `create_entity_sync`'s
/// `source`/`target_entry_id` shape so both the user-facing command and a
/// narrator tool's staged commit can call it identically.
pub fn update_entity_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
) -> AppResult<Entity> {
    update_entity_in_turn(
        conn,
        story_id,
        entity_id,
        name,
        appearance_anchor,
        source,
        target_entry_id,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_entity_in_turn(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    let before: Entity = conn.query_row(
        "SELECT entities.id, entities.story_id, entities.kind, story_entity_state.name,
                story_entity_state.appearance_anchor, entities.created_at
         FROM entities JOIN story_entity_state ON story_entity_state.entity_id = entities.id
         WHERE entities.id = ?1 AND story_entity_state.story_id = ?2 AND story_entity_state.is_present = 1",
        rusqlite::params![entity_id, story_id], row_to_entity,
    ).optional()?.ok_or_else(|| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let anchor = appearance_anchor.map(str::trim).filter(|s| !s.is_empty());
    let content = format!(
        "{} is now named {name}; appearance details were updated.",
        before.name
    );
    let event = EntityEvent::Updated {
        entity_id: entity_id.to_string(),
        before: NameAnchor {
            name: before.name.clone(),
            appearance_anchor: before.appearance_anchor.clone(),
        },
        after: NameAnchor {
            name: name.to_string(),
            appearance_anchor: anchor.map(str::to_string),
        },
        source: source.to_string(),
    };
    projection::record(conn, story_id, target_entry_id, &content, &event, turn_id)?;
    Ok(Entity {
        name: name.into(),
        appearance_anchor: anchor.map(str::to_string),
        ..before
    })
}

pub(crate) fn delete_entity_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
) -> AppResult<()> {
    let name: String = conn.query_row("SELECT name FROM story_entity_state WHERE story_id = ?1 AND entity_id = ?2 AND is_present = 1", rusqlite::params![story_id, entity_id], |r| r.get(0))
        .map_err(|_| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let event = EntityEvent::Deleted {
        entity_id: entity_id.to_string(),
        name: name.clone(),
        source: "user".into(),
    };
    projection::record(
        conn,
        story_id,
        None,
        &format!("{name} was removed from the authoritative entity state."),
        &event,
        None,
    )?;
    Ok(())
}
