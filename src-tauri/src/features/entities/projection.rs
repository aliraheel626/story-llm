use std::collections::HashSet;

use chrono::Utc;

use crate::features::transcript::{model::TranscriptEntry, repository};
use crate::shared::error::{AppError, AppResult};

use super::events::EntityEvent;

pub enum ApplyMode {
    Live,
    Replay,
}

pub fn apply(
    conn: &rusqlite::Connection,
    story_id: &str,
    event: &EntityEvent,
    mode: ApplyMode,
) -> AppResult<()> {
    let now = Utc::now().to_rfc3339();
    let is_present = matches!(mode, ApplyMode::Live);
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
                "INSERT INTO entities
                 (id, story_id, kind, name, appearance_anchor, is_present, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                     name=excluded.name, appearance_anchor=excluded.appearance_anchor,
                     is_present=excluded.is_present, updated_at=excluded.updated_at
                 WHERE entities.story_id=excluded.story_id",
                rusqlite::params![
                    entity_id,
                    story_id,
                    kind,
                    name,
                    appearance_anchor,
                    is_present,
                    created_at,
                    now
                ],
            )?;
        }
        EntityEvent::Updated {
            entity_id, before, after, ..
        } => {
            conn.execute(
                "UPDATE entities
                 SET name=CASE WHEN ?1 THEN ?2 ELSE name END,
                     appearance_anchor=CASE WHEN ?3 THEN ?4 ELSE appearance_anchor END,
                     is_present=?5, updated_at=?6
                 WHERE story_id=?7 AND id=?8",
                rusqlite::params![
                    before.name != after.name,
                    after.name,
                    before.appearance_anchor != after.appearance_anchor,
                    after.appearance_anchor,
                    is_present,
                    now,
                    story_id,
                    entity_id
                ],
            )?;
        }
        EntityEvent::Deleted { entity_id, .. } => {
            conn.execute(
                "UPDATE entities
                 SET is_present=CASE WHEN ?1 THEN 0 ELSE is_present END, updated_at=?2
                 WHERE story_id=?3 AND id=?4",
                rusqlite::params![is_present, now, story_id, entity_id],
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
                 (entity_id, attribute_id, value, source, updated_at)
                 SELECT id, ?3, ?4, ?5, ?6 FROM entities WHERE story_id=?1 AND id=?2
                 ON CONFLICT(entity_id, attribute_id) DO UPDATE SET
                     value=excluded.value, source=excluded.source,
                     updated_at=excluded.updated_at",
                rusqlite::params![
                    story_id,
                    entity_id,
                    attribute_id,
                    after,
                    source,
                    now
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
                 WHERE entity_id IN (SELECT id FROM entities WHERE story_id=?1 AND id=?2)
                   AND attribute_id=?3",
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
) -> AppResult<TranscriptEntry> {
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
    apply(conn, story_id, &recorded_event, ApplyMode::Live)?;
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
            "DELETE FROM entities WHERE story_id = ?1 AND id = ?2",
            rusqlite::params![story_id, entity_id],
        )?;
    }
    let mut created = HashSet::new();
    let mut present = HashSet::new();
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
        if matches!(&event, EntityEvent::Created { .. }) {
            created.insert(event.entity_id().to_string());
        } else if !created.contains(event.entity_id()) {
            continue;
        }
        apply(conn, story_id, &event, ApplyMode::Replay)?;
        match event {
            EntityEvent::Created { entity_id, .. } | EntityEvent::Updated { entity_id, .. } => {
                present.insert(entity_id);
            }
            EntityEvent::Deleted { entity_id, .. } => {
                present.remove(&entity_id);
            }
            _ => {}
        }
    }
    // Historical names may overlap; enforce uniqueness only on the final present set.
    if !present.is_empty() {
        let placeholders = std::iter::repeat_n("?", present.len())
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute(
            &format!("UPDATE entities SET is_present = 1 WHERE story_id = ? AND id IN ({placeholders})"),
            rusqlite::params_from_iter(
                std::iter::once(story_id).chain(present.iter().map(String::as_str)),
            ),
        )?;
    }
    Ok(())
}

pub fn replay_after_erase(
    conn: &rusqlite::Connection,
    story_id: &str,
    entries: &[TranscriptEntry],
) -> AppResult<()> {
    let affected = entries
        .iter()
        .filter_map(EntityEvent::from_entry)
        .map(|event| event.entity_id().to_string())
        .collect();
    replay(conn, story_id, &affected, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        entities::{attributes, repository as entity_repository},
        transcript::repository as transcript_repository,
    };

    type StateSnapshot = Vec<(String, String, Option<String>, i64)>;
    type AttributeSnapshot = Vec<(String, String, f64, String)>;

    fn snapshots(conn: &rusqlite::Connection) -> (StateSnapshot, AttributeSnapshot) {
        let state = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, appearance_anchor, is_present
                     FROM entities WHERE story_id = 'story' ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        let attributes = {
            let mut stmt = conn
                .prepare(
                    "SELECT entity_id, attribute_id, value, source
                     FROM entity_attributes JOIN entities ON entities.id = entity_attributes.entity_id
                     WHERE entities.story_id = 'story' ORDER BY entity_id, attribute_id",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
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
        let passage = transcript_repository::append_story_message(
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
        replay(
            &conn,
            "story",
            &HashSet::from(["guard".into(), "temporary".into()]),
            None,
        )
        .unwrap();
        assert_eq!(snapshots(&conn), before);
    }
}
