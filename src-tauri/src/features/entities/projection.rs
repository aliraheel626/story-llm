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
            link,
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
            if let Some(link) = link {
                conn.execute(
                    "INSERT INTO relationships (entity_id, from_id, to_id, label, direction, description)
                     SELECT id, ?3, ?4, ?5, ?6, ?7 FROM entities WHERE story_id=?1 AND id=?2
                     ON CONFLICT(entity_id) DO UPDATE SET
                         from_id=excluded.from_id, to_id=excluded.to_id, label=excluded.label,
                         direction=excluded.direction, description=excluded.description",
                    rusqlite::params![
                        story_id, entity_id, link.from_id, link.to_id, link.label,
                        link.direction, link.description
                    ],
                )?;
            }
        }
        EntityEvent::Updated {
            entity_id, before, after, ..
        } => {
            // Relationship names are computed display text; their stored name stays NULL.
            let character = before.link.is_none() && after.link.is_none();
            conn.execute(
                "UPDATE entities
                 SET name=CASE WHEN ?1 THEN ?2 ELSE name END,
                     appearance_anchor=CASE WHEN ?3 THEN ?4 ELSE appearance_anchor END,
                     is_present=?5, updated_at=?6
                 WHERE story_id=?7 AND id=?8",
                rusqlite::params![
                    character && before.name != after.name,
                    after.name,
                    character && before.appearance_anchor != after.appearance_anchor,
                    after.appearance_anchor,
                    is_present,
                    now,
                    story_id,
                    entity_id
                ],
            )?;
            if let (Some(before), Some(after)) = (&before.link, &after.link) {
                conn.execute(
                    "UPDATE relationships
                     SET label=CASE WHEN ?1 THEN ?2 ELSE label END,
                         direction=CASE WHEN ?3 THEN ?4 ELSE direction END,
                         description=CASE WHEN ?5 THEN ?6 ELSE description END
                     WHERE entity_id IN (SELECT id FROM entities WHERE story_id=?7 AND id=?8)",
                    rusqlite::params![
                        before.label != after.label, after.label,
                        before.direction != after.direction, after.direction,
                        before.description != after.description, after.description,
                        story_id, entity_id
                    ],
                )?;
            }
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
        entities::{attributes, model::{EntityLink, CHARACTER}, repository as entity_repository},
        transcript::repository as transcript_repository,
    };
    use crate::shared::{db::with_transaction, test_support};

    type StateSnapshot = Vec<(String, Option<String>, Option<String>, i64)>;
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
                "later inference",
                &passage.id,
                true,
                None,
            )
            .unwrap(),
            (7.0, 3.0)
        );
        attributes::remove_entity_attribute_sync(&conn, "story", "guard", &accuracy_id).unwrap();

        entity_repository::create_entity_with_id_sync(
            &conn,
            "temporary",
            "story",
            "character",
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

    #[test]
    fn replaying_an_endpoint_preserves_relationships_and_their_stats() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "story");
        for (id, name) in [("mira", "Mira"), ("varro", "Varro")] {
            entity_repository::create_entity_with_id_sync(
                &conn, id, "story", CHARACTER, name, None, "user", None, None,
            ).unwrap();
        }
        let relationship = entity_repository::create_link_sync(
            &conn, "story", EntityLink {
                from_id: "mira".into(), to_id: "varro".into(), label: "friendly rivals".into(),
                direction: "one_way".into(), description: None,
            }, "user", None, None,
        ).unwrap();
        entity_repository::update_link_sync(
            &conn, "story", &relationship.id, Some("rivals"), None,
            Some(Some("An old competition.".into())), "user", None, None,
        ).unwrap();
        let affection = attributes::find_exact_match(&conn, "Affection").unwrap().unwrap();
        attributes::set_entity_attribute_sync(
            &conn, "story", &relationship.id, &affection.id, -3.0,
        ).unwrap();
        // Relationship replay regenerates updated_at; compare semantic state here.
        let snapshot = |conn: &rusqlite::Connection| {
            let relation = conn.query_row(
                "SELECT entity_id, from_id, to_id, label, direction, description
                 FROM relationships WHERE entity_id=?1",
                [&relationship.id], |row| Ok((
                    row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, Option<String>>(5)?,
                )),
            ).unwrap();
            let identity = conn.query_row(
                "SELECT name, is_present, created_at FROM entities WHERE id=?1",
                [&relationship.id], |row| Ok((
                    row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?,
                )),
            ).unwrap();
            let stats = attributes::list_entity_attributes_sync(conn, "story", &relationship.id)
                .unwrap().into_iter().map(|stat| (stat.attribute_id, stat.value, stat.source))
                .collect::<Vec<_>>();
            (relation, identity, stats)
        };
        let before = snapshot(&conn);
        assert_eq!(before.1.0, None);
        assert_eq!(before.2[0].1, -3.0);
        let timestamps = conn.query_row(
            "SELECT e.updated_at, a.updated_at FROM entities e
             JOIN entity_attributes a ON a.entity_id=e.id WHERE e.id=?1",
            [&relationship.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).unwrap();
        replay(&conn, "story", &HashSet::from(["mira".into()]), None).unwrap();
        assert_eq!(snapshot(&conn), before);
        assert_eq!(conn.query_row(
            "SELECT e.updated_at, a.updated_at FROM entities e
             JOIN entity_attributes a ON a.entity_id=e.id WHERE e.id=?1",
            [&relationship.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).unwrap(), timestamps);
        replay(&conn, "story", &HashSet::from([relationship.id.clone()]), None).unwrap();
        assert_eq!(snapshot(&conn), before);

        let (turn_id, _, narration_id) =
            test_support::exchange(&conn, "story", "do", "make peace", Some("An uneasy truce."));
        entity_repository::update_link_sync(
            &conn, "story", &relationship.id, None, Some("both"),
            Some(Some("An uneasy truce.".into())), "narrator_tool", narration_id.as_deref(), Some(&turn_id),
        ).unwrap();
        entity_repository::update_link_sync(
            &conn, "story", &relationship.id, Some("former rivals"), None, None,
            "user", None, None,
        ).unwrap();
        replay(
            &conn, "story", &HashSet::from([relationship.id.clone()]), Some(&turn_id),
        ).unwrap();
        let mut expected = before;
        expected.0.3 = "former rivals".into();
        assert_eq!(snapshot(&conn), expected);
    }

    #[test]
    fn erasing_relationship_creation_removes_its_subtype_and_stats() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "story");
        for (id, name) in [("mira", "Mira"), ("varro", "Varro")] {
            entity_repository::create_entity_with_id_sync(
                &conn, id, "story", CHARACTER, name, None, "user", None, None,
            ).unwrap();
        }
        let (turn_id, action_id, narration_id) =
            test_support::exchange(&conn, "story", "do", "meet Varro", Some("They become rivals."));
        let relationship = entity_repository::create_link_sync(
            &conn, "story", EntityLink {
                from_id: "mira".into(), to_id: "varro".into(), label: "rivals".into(),
                direction: "one_way".into(), description: None,
            }, "narrator_tool", narration_id.as_deref(), Some(&turn_id),
        ).unwrap();
        let affection = attributes::find_exact_match(&conn, "Affection").unwrap().unwrap();
        attributes::apply_attribute_delta(
            &conn, "story", &relationship.id, &affection, -3.0, "a rivalry",
            narration_id.as_deref().unwrap(), false, Some(&turn_id),
        ).unwrap();
        entity_repository::update_link_sync(
            &conn, "story", &relationship.id, Some("former rivals"), None, None,
            "user", None, None,
        ).unwrap();
        attributes::set_entity_attribute_sync(
            &conn, "story", &relationship.id, &affection.id, -2.0,
        ).unwrap();
        assert_eq!(conn.query_row(
            "SELECT name FROM entities WHERE id=?1", [&relationship.id],
            |row| row.get::<_, Option<String>>(0),
        ).unwrap(), None);
        let relationship_id = relationship.id;
        drop(conn);

        let removed_ids = with_transaction(&pool, |tx| {
            let removed = crate::features::transcript::erase::erase_last_exchange_in_tx(tx, "story")?
                .unwrap();
            replay_after_erase(tx, "story", &removed.entries)?;
            Ok(removed.visible_ids)
        }).unwrap();
        assert_eq!(removed_ids, vec![action_id, narration_id.unwrap()]);
        let conn = pool.get().unwrap();
        let counts = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM entities WHERE id=?1),
                    (SELECT COUNT(*) FROM relationships WHERE entity_id=?1),
                    (SELECT COUNT(*) FROM entity_attributes WHERE entity_id=?1)",
            [&relationship_id], |row| Ok((
                row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?,
            )),
        ).unwrap();
        assert_eq!(counts, (0, 0, 0));
        assert_eq!(entity_repository::list_entities_sync(&conn, "story", Some(CHARACTER)).unwrap().len(), 2);
        assert_eq!(conn.query_row(
            "SELECT COUNT(*) FROM transcript_entries WHERE kind='entity_attribute_changed'",
            [], |row| row.get::<_, i64>(0),
        ).unwrap(), 1);
    }
}
