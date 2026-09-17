use std::collections::HashSet;

use chrono::Utc;

use crate::shared::error::AppResult;

use super::{model::kind, repository};

/// Replays only entities whose authoritative events were removed by a
/// destructive tail erase. Unrelated projection rows remain untouched.
pub fn replay_entities(
    conn: &rusqlite::Connection,
    branch_id: &str,
    entity_ids: &HashSet<String>,
) -> AppResult<()> {
    if entity_ids.is_empty() {
        return Ok(());
    }
    for entity_id in entity_ids {
        conn.execute(
            "DELETE FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2",
            rusqlite::params![branch_id, entity_id],
        )?;
        conn.execute(
            "DELETE FROM branch_entity_state WHERE branch_id = ?1 AND entity_id = ?2",
            rusqlite::params![branch_id, entity_id],
        )?;
    }
    let now = Utc::now().to_rfc3339();
    for event in repository::list_logical_entries(conn, branch_id)? {
        let entity_id = event.payload.get("entity_id").and_then(|v| v.as_str());
        if !entity_id.is_some_and(|id| entity_ids.contains(id)) {
            continue;
        }
        match event.kind.as_str() {
            kind::ENTITY_CREATED => {
                let (Some(entity_id), Some(name)) = (
                    entity_id,
                    event.payload.get("name").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let anchor = event
                    .payload
                    .get("appearance_anchor")
                    .and_then(|v| v.as_str());
                conn.execute("INSERT INTO branch_entity_state (branch_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
                    VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)
                    ON CONFLICT(branch_id, entity_id) DO UPDATE SET name=excluded.name, appearance_anchor=excluded.appearance_anchor, is_present=1, updated_at=excluded.updated_at, last_event_id=excluded.last_event_id",
                    rusqlite::params![branch_id, entity_id, name, anchor, now, event.id])?;
            }
            kind::ENTITY_UPDATED => {
                let Some(entity_id) = entity_id else { continue };
                let after = event.payload.get("after").cloned().unwrap_or_default();
                let name = after.get("name").and_then(|v| v.as_str());
                let anchor = after.get("appearance_anchor").and_then(|v| v.as_str());
                if let Some(name) = name {
                    conn.execute("UPDATE branch_entity_state SET name=?1, appearance_anchor=?2, is_present=1, updated_at=?3, last_event_id=?4 WHERE branch_id=?5 AND entity_id=?6",
                        rusqlite::params![name, anchor, now, event.id, branch_id, entity_id])?;
                }
            }
            kind::ENTITY_DELETED => {
                if let Some(entity_id) = entity_id {
                    conn.execute("UPDATE branch_entity_state SET is_present=0, updated_at=?1, last_event_id=?2 WHERE branch_id=?3 AND entity_id=?4",
                    rusqlite::params![now, event.id, branch_id, entity_id])?;
                }
            }
            kind::ENTITY_ATTRIBUTE_CHANGED => {
                let (Some(entity_id), Some(attribute_id), Some(value)) = (
                    entity_id,
                    event.payload.get("attribute_id").and_then(|v| v.as_str()),
                    event.payload.get("after").and_then(|v| v.as_f64()),
                ) else {
                    continue;
                };
                let source = event
                    .payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("inferred");
                conn.execute("INSERT INTO entity_attributes (branch_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                    ON CONFLICT(branch_id, entity_id, attribute_id) DO UPDATE SET value=excluded.value, source=excluded.source, updated_at=excluded.updated_at, last_event_id=excluded.last_event_id",
                    rusqlite::params![branch_id, entity_id, attribute_id, value, source, now, event.id])?;
            }
            kind::ENTITY_ATTRIBUTE_REMOVED => {
                let (Some(entity_id), Some(attribute_id)) = (
                    entity_id,
                    event.payload.get("attribute_id").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                conn.execute("DELETE FROM entity_attributes WHERE branch_id=?1 AND entity_id=?2 AND attribute_id=?3", rusqlite::params![branch_id, entity_id, attribute_id])?;
            }
            _ => {}
        }
    }
    Ok(())
}
