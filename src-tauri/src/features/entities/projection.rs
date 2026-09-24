use std::collections::HashSet;

use chrono::Utc;

use crate::features::ledger::{model::LedgerEntry, repository};
use crate::shared::error::{AppError, AppResult};

use super::events::EntityEvent;

pub fn apply(
    conn: &rusqlite::Connection,
    story_id: &str,
    event_id: &str,
    event: &EntityEvent,
) -> AppResult<()> {
    let now = Utc::now().to_rfc3339();
    match event {
        EntityEvent::Created {
            entity_id,
            kind,
            name,
            appearance_anchor,
            created_at,
            ..
        } => {
            conn.execute(
                "INSERT OR IGNORE INTO entities (id, story_id, kind, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![entity_id, story_id, kind, created_at],
            )?;
            conn.execute(
                "INSERT INTO story_entity_state
                 (story_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)
                 ON CONFLICT(story_id, entity_id) DO UPDATE SET
                     name=excluded.name, appearance_anchor=excluded.appearance_anchor,
                     is_present=1, updated_at=excluded.updated_at,
                     last_event_id=excluded.last_event_id",
                rusqlite::params![
                    story_id,
                    entity_id,
                    name,
                    appearance_anchor,
                    now,
                    event_id
                ],
            )?;
        }
        EntityEvent::Updated {
            entity_id, after, ..
        } => {
            conn.execute(
                "UPDATE story_entity_state
                 SET name=?1, appearance_anchor=?2, is_present=1, updated_at=?3, last_event_id=?4
                 WHERE story_id=?5 AND entity_id=?6",
                rusqlite::params![
                    after.name,
                    after.appearance_anchor,
                    now,
                    event_id,
                    story_id,
                    entity_id
                ],
            )?;
        }
        EntityEvent::Deleted { entity_id, .. } => {
            conn.execute(
                "UPDATE story_entity_state
                 SET is_present=0, updated_at=?1, last_event_id=?2
                 WHERE story_id=?3 AND entity_id=?4",
                rusqlite::params![now, event_id, story_id, entity_id],
            )?;
        }
        EntityEvent::AttributeChanged {
            entity_id,
            attribute_id,
            after,
            source,
            ..
        } => {
            conn.execute(
                "INSERT INTO entity_attributes
                 (story_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(story_id, entity_id, attribute_id) DO UPDATE SET
                     value=excluded.value, source=excluded.source,
                     updated_at=excluded.updated_at, last_event_id=excluded.last_event_id",
                rusqlite::params![
                    story_id,
                    entity_id,
                    attribute_id,
                    after,
                    source,
                    now,
                    event_id
                ],
            )?;
        }
        EntityEvent::AttributeRemoved {
            entity_id,
            attribute_id,
            ..
        } => {
            conn.execute(
                "DELETE FROM entity_attributes
                 WHERE story_id=?1 AND entity_id=?2 AND attribute_id=?3",
                rusqlite::params![story_id, entity_id, attribute_id],
            )?;
        }
    }
    Ok(())
}

pub fn record(
    conn: &rusqlite::Connection,
    story_id: &str,
    target_entry_id: Option<&str>,
    content: &str,
    event: &EntityEvent,
    turn_id: Option<&str>,
) -> AppResult<LedgerEntry> {
    let entry = repository::append_entry(
        conn,
        story_id,
        event.kind(),
        "hidden",
        Some(content),
        &event.payload(),
        target_entry_id,
        turn_id,
    )?;
    let recorded_event = EntityEvent::from_entry(&entry)
        .ok_or_else(|| AppError::Other("recorded entity event could not be parsed".into()))?;
    apply(conn, story_id, &entry.id, &recorded_event)?;
    Ok(entry)
}

pub fn replay(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_ids: &HashSet<String>,
    exclude_turn: Option<&str>,
) -> AppResult<()> {
    if entity_ids.is_empty() {
        return Ok(());
    }
    for entity_id in entity_ids {
        conn.execute(
            "DELETE FROM entity_attributes WHERE story_id = ?1 AND entity_id = ?2",
            rusqlite::params![story_id, entity_id],
        )?;
        conn.execute(
            "DELETE FROM story_entity_state WHERE story_id = ?1 AND entity_id = ?2",
            rusqlite::params![story_id, entity_id],
        )?;
        conn.execute(
            "DELETE FROM entities WHERE story_id = ?1 AND id = ?2",
            rusqlite::params![story_id, entity_id],
        )?;
    }
    let mut created = HashSet::new();
    for entry in repository::list_logical_entries(conn, story_id)? {
        if exclude_turn.is_some_and(|turn_id| entry.turn_id.as_deref() == Some(turn_id)) {
            continue;
        }
        let Some(event) = EntityEvent::from_entry(&entry) else {
            continue;
        };
        if !entity_ids.contains(event.entity_id()) {
            continue;
        }
        match &event {
            EntityEvent::Created { entity_id, .. } => {
                apply(conn, story_id, &entry.id, &event)?;
                created.insert(entity_id.clone());
            }
            _ if created.contains(event.entity_id()) => {
                apply(conn, story_id, &entry.id, &event)?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn rebuild_story(
    conn: &rusqlite::Connection,
    story_id: &str,
    exclude_turn: Option<&str>,
) -> AppResult<()> {
    let mut entity_ids = {
        let mut stmt = conn.prepare("SELECT id FROM entities WHERE story_id = ?1")?;
        let rows = stmt.query_map([story_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<HashSet<_>, _>>()?
    };
    for entry in repository::list_logical_entries(conn, story_id)? {
        if let Some(event) = EntityEvent::from_entry(&entry) {
            entity_ids.insert(event.entity_id().to_string());
        }
    }
    replay(conn, story_id, &entity_ids, exclude_turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        entities::{attributes, repository as entity_repository},
        ledger::repository as ledger_repository,
    };

    type StateSnapshot = Vec<(String, String, Option<String>, i64, String)>;
    type AttributeSnapshot = Vec<(String, String, f64, String, String)>;

    fn snapshots(conn: &rusqlite::Connection) -> (StateSnapshot, AttributeSnapshot) {
        let state = {
            let mut stmt = conn
                .prepare(
                    "SELECT entity_id, name, appearance_anchor, is_present, last_event_id
                     FROM story_entity_state WHERE story_id = 'story' ORDER BY entity_id",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        let attributes = {
            let mut stmt = conn
                .prepare(
                    "SELECT entity_id, attribute_id, value, source, last_event_id
                     FROM entity_attributes WHERE story_id = 'story'
                     ORDER BY entity_id, attribute_id",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        (state, attributes)
    }

    #[test]
    fn writes_and_rebuild_produce_identical_state() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('story', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let passage = ledger_repository::append_story_message(
            &conn,
            "story",
            "narrator",
            "generated",
            "A scene",
            None,
            None,
        )
        .unwrap();
        let accuracy_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Accuracy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let accuracy = attributes::find_attribute_by_id(&conn, &accuracy_id).unwrap();

        entity_repository::create_entity_with_id_sync(
            &conn,
            "mira",
            "story",
            "character",
            "Mira",
            Some("silver hair"),
            "test",
            None,
            None,
        )
        .unwrap();
        entity_repository::update_entity_sync(
            &conn,
            "story",
            "mira",
            "Mira Vale",
            Some("silver hair and a red cloak"),
            "narrator_tool",
            Some(&passage.id),
            None,
        )
        .unwrap();
        attributes::apply_attribute_delta(
            &conn,
            "story",
            "mira",
            &accuracy,
            2.0,
            "first inference",
            &passage.id,
            false,
            None,
        )
        .unwrap();
        attributes::apply_attribute_delta(
            &conn,
            "story",
            "mira",
            &accuracy,
            1.0,
            "follow-up inference",
            &passage.id,
            false,
            None,
        )
        .unwrap();
        attributes::apply_attribute_delta(
            &conn,
            "story",
            "mira",
            &accuracy,
            100.0,
            "dramatic inference",
            &passage.id,
            true,
            None,
        )
        .unwrap();

        entity_repository::create_entity_with_id_sync(
            &conn,
            "guard",
            "story",
            "character",
            "Guard",
            None,
            "test",
            None,
            None,
        )
        .unwrap();
        attributes::set_entity_attribute_sync(&conn, "story", "guard", &accuracy_id, 7.0).unwrap();
        assert_eq!(
            attributes::apply_attribute_delta(
                &conn,
                "story",
                "guard",
                &accuracy,
                -4.0,
                "locked inference",
                &passage.id,
                true,
                None,
            )
            .unwrap(),
            (7.0, 7.0)
        );
        attributes::remove_entity_attribute_sync(&conn, "story", "guard", &accuracy_id).unwrap();

        entity_repository::create_entity_with_id_sync(
            &conn,
            "temporary",
            "story",
            "location",
            "Temporary Camp",
            None,
            "test",
            None,
            None,
        )
        .unwrap();
        entity_repository::delete_entity_sync(&conn, "story", "temporary").unwrap();

        let before = snapshots(&conn);
        rebuild_story(&conn, "story", None).unwrap();
        assert_eq!(snapshots(&conn), before);
    }
}
