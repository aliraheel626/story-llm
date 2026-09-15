use chrono::Utc;

use crate::shared::error::AppResult;

use super::{model::kind, repository};

/// Replays authoritative state events after destructive tail erasure. The
/// timeline remains the source of truth; projection tables are disposable.
pub fn rebuild_branch(conn: &rusqlite::Connection, branch_id: &str) -> AppResult<()> {
    conn.execute(
        "DELETE FROM entity_attributes WHERE branch_id = ?1",
        [branch_id],
    )?;
    conn.execute(
        "DELETE FROM branch_entity_state WHERE branch_id = ?1",
        [branch_id],
    )?;
    let now = Utc::now().to_rfc3339();
    for event in repository::list_logical_entries(conn, branch_id)? {
        let entity_id = event.payload.get("entity_id").and_then(|v| v.as_str());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::timeline::{model::kind, repository::append_entry};
    use serde_json::json;

    #[test]
    fn rebuild_replays_entity_and_attribute_events() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE branches(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, parent_branch_id TEXT, forked_at_entry_id TEXT);
            CREATE TABLE timeline_entries(id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(branch_id,seq));
            CREATE TABLE branch_entity_state(branch_id TEXT NOT NULL, entity_id TEXT NOT NULL, name TEXT NOT NULL, appearance_anchor TEXT, is_present INTEGER NOT NULL, updated_at TEXT NOT NULL, last_event_id TEXT, PRIMARY KEY(branch_id,entity_id));
            CREATE TABLE entity_attributes(branch_id TEXT NOT NULL, entity_id TEXT NOT NULL, attribute_id TEXT NOT NULL, value REAL NOT NULL, source TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_id TEXT, PRIMARY KEY(branch_id,entity_id,attribute_id));
            INSERT INTO stories VALUES ('s','now');
            INSERT INTO branches VALUES ('b','s',NULL,NULL);").unwrap();

        append_entry(
            &conn,
            "b",
            kind::ENTITY_CREATED,
            "hidden",
            Some("Mira was added."),
            &json!({"entity_id":"mira","name":"Mira","appearance_anchor":"silver hair"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            kind::ENTITY_ATTRIBUTE_CHANGED,
            "hidden",
            Some("Stealth changed."),
            &json!({"entity_id":"mira","attribute_id":"stealth","after":8.0,"source":"user"}),
            None,
        )
        .unwrap();

        conn.execute(
            "INSERT INTO branch_entity_state VALUES ('b','stale','Stale',NULL,1,'now',NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_attributes VALUES ('b','stale','stealth',1,'inferred','now',NULL)",
            [],
        )
        .unwrap();
        rebuild_branch(&conn, "b").unwrap();

        let state: (String, Option<String>, i64) = conn.query_row(
            "SELECT name, appearance_anchor, is_present FROM branch_entity_state WHERE branch_id='b' AND entity_id='mira'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(state, ("Mira".into(), Some("silver hair".into()), 1));
        let attribute: (f64, String) = conn.query_row(
            "SELECT value, source FROM entity_attributes WHERE branch_id='b' AND entity_id='mira'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(attribute, (8.0, "user".into()));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM branch_entity_state WHERE entity_id='stale'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
