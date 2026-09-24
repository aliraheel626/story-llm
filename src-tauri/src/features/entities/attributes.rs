//! Per-entity attribute value reads, clamping, and writes.

use std::collections::HashMap;

use rusqlite::OptionalExtension;

use crate::shared::error::{AppError, AppResult};

pub(crate) use super::registry::{
    add_alias, find_attribute_by_id, find_exact_match, insert_minted_attribute, resolve_attribute,
    AttributeResolution,
};
use super::{
    events::EntityEvent,
    model::{AttributeRegistryEntry, EntityAttributeValue},
    projection,
};

/// Reads an entity's current value for an attribute without writing anything.
/// The optional source is `None` for the implicit midpoint and identifies
/// player-locked rows without a second lookup.
pub fn peek_entity_attribute(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    attribute: &AttributeRegistryEntry,
) -> AppResult<(f64, Option<String>)> {
    let existing: Option<(f64, String)> = conn
        .query_row(
            "SELECT value, source FROM entity_attributes WHERE story_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
            rusqlite::params![story_id, entity_id, attribute.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match existing {
        Some((value, source)) => (value, Some(source)),
        None => ((attribute.min + attribute.max) / 2.0, None),
    })
}

/// Max magnitude a single non-dramatic update may move an attribute, as a
/// fraction of its full range — rejects an implausible ±9 swing on a 0-10
/// scale unless the caller flags the change as dramatic.
const NON_DRAMATIC_MAX_FRACTION: f64 = 0.3;

/// Applies clamping and rate-limiting to a proposed delta, without touching
/// the database — shared by the real write path below and by narrator-tool
/// staging previews that need to show what a pending delta *would* do before
/// it's committed.
pub fn clamp_delta(
    before: f64,
    delta: f64,
    dramatic: bool,
    attribute: &AttributeRegistryEntry,
) -> f64 {
    let range = attribute.max - attribute.min;
    let max_step = if dramatic {
        range
    } else {
        range * NON_DRAMATIC_MAX_FRACTION
    };
    let clamped_delta = delta.clamp(-max_step, max_step);
    (before + clamped_delta).clamp(attribute.min, attribute.max)
}

/// Applies a proposed delta with clamping and rate-limiting, and logs an
/// append-only ledger event. Returns `(before, after)`.
#[allow(clippy::too_many_arguments)]
pub fn apply_attribute_delta(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    attribute: &AttributeRegistryEntry,
    delta: f64,
    cause: &str,
    entry_id: &str,
    dramatic: bool,
    turn_id: Option<&str>,
) -> AppResult<(f64, f64)> {
    let (before, current_source) = peek_entity_attribute(conn, story_id, entity_id, attribute)?;
    if current_source.as_deref() == Some("user") {
        // The narrator context tells the model user overrides take
        // precedence over inferred updates; honor that here rather than
        // silently overwriting a value the player explicitly set.
        return Ok((before, before));
    }
    let after = clamp_delta(before, delta, dramatic, attribute);

    let event = EntityEvent::AttributeChanged {
        entity_id: entity_id.to_string(),
        attribute_id: attribute.id.clone(),
        attribute_name: attribute.canonical_name.clone(),
        before: Some(before),
        after,
        source: "inferred".into(),
        delta: Some(after - before),
        cause: Some(cause.to_string()),
    };
    projection::record(
        conn,
        story_id,
        Some(entry_id),
        &format!(
            "{} changed from {} to {}: {cause}",
            attribute.canonical_name, before, after
        ),
        &event,
        turn_id,
    )?;
    Ok((before, after))
}

fn row_to_entity_attribute(row: &rusqlite::Row) -> rusqlite::Result<EntityAttributeValue> {
    Ok(EntityAttributeValue {
        story_id: row.get(0)?,
        entity_id: row.get(1)?,
        attribute_id: row.get(2)?,
        canonical_name: row.get(3)?,
        value: row.get(4)?,
        min: row.get(5)?,
        max: row.get(6)?,
        updated_at: row.get(7)?,
        source: row.get(8)?,
    })
}

pub(crate) fn list_entity_attributes_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
) -> AppResult<Vec<EntityAttributeValue>> {
    let mut stmt = conn.prepare(
        "SELECT entity_attributes.story_id, entity_attributes.entity_id, entity_attributes.attribute_id, attribute_registry.canonical_name,
                entity_attributes.value, attribute_registry.min, attribute_registry.max, entity_attributes.updated_at, entity_attributes.source
         FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id
         WHERE entity_attributes.story_id = ?1 AND entity_attributes.entity_id = ?2 ORDER BY attribute_registry.canonical_name ASC")?;
    let rows = stmt.query_map(
        rusqlite::params![story_id, entity_id],
        row_to_entity_attribute,
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Loads committed attributes for many entities with one registry join.
/// Entries for requested entities with no stored attributes are empty.
pub(crate) fn list_entity_attributes_for_entities_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_ids: &[&str],
) -> AppResult<HashMap<String, Vec<EntityAttributeValue>>> {
    let mut out: HashMap<String, Vec<EntityAttributeValue>> = entity_ids
        .iter()
        .map(|entity_id| ((*entity_id).to_string(), Vec::new()))
        .collect();
    if entity_ids.is_empty() {
        return Ok(out);
    }

    let placeholders = std::iter::repeat_n("?", entity_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT entity_attributes.story_id, entity_attributes.entity_id, entity_attributes.attribute_id, attribute_registry.canonical_name,
                entity_attributes.value, attribute_registry.min, attribute_registry.max, entity_attributes.updated_at, entity_attributes.source
         FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id
         WHERE entity_attributes.story_id = ? AND entity_attributes.entity_id IN ({placeholders})
         ORDER BY entity_attributes.entity_id, attribute_registry.canonical_name ASC"
    ))?;
    let params = std::iter::once(story_id).chain(entity_ids.iter().copied());
    let rows = stmt.query_map(rusqlite::params_from_iter(params), row_to_entity_attribute)?;
    for row in rows {
        let attribute = row?;
        out.entry(attribute.entity_id.clone())
            .or_default()
            .push(attribute);
    }
    Ok(out)
}

pub(crate) fn set_entity_attribute_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    attribute_id: &str,
    value: f64,
) -> AppResult<EntityAttributeValue> {
    let (name, min, max): (String, f64, f64) = conn
        .query_row(
            "SELECT canonical_name, min, max FROM attribute_registry WHERE id = ?1",
            [attribute_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| AppError::NotFound(format!("attribute {attribute_id} not found")))?;
    if !value.is_finite() || value < min || value > max {
        return Err(AppError::Invalid(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    conn.query_row("SELECT 1 FROM story_entity_state WHERE story_id = ?1 AND entity_id = ?2 AND is_present = 1", rusqlite::params![story_id, entity_id], |_| Ok(()))
        .map_err(|_| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let before: Option<f64> = conn.query_row("SELECT value FROM entity_attributes WHERE story_id = ?1 AND entity_id = ?2 AND attribute_id = ?3", rusqlite::params![story_id, entity_id, attribute_id], |r| r.get(0)).optional()?;
    let event = EntityEvent::AttributeChanged {
        entity_id: entity_id.to_string(),
        attribute_id: attribute_id.to_string(),
        attribute_name: name.clone(),
        before,
        after: value,
        source: "user".into(),
        delta: None,
        cause: None,
    };
    projection::record(
        conn,
        story_id,
        None,
        &format!(
            "User changed {name} from {} to {value}.",
            before
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unset".into())
        ),
        &event,
        None,
    )?;
    let updated_at = conn.query_row(
        "SELECT updated_at FROM entity_attributes
         WHERE story_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
        rusqlite::params![story_id, entity_id, attribute_id],
        |row| row.get(0),
    )?;
    Ok(EntityAttributeValue {
        story_id: story_id.to_string(),
        entity_id: entity_id.to_string(),
        attribute_id: attribute_id.to_string(),
        canonical_name: name,
        value,
        min,
        max,
        updated_at,
        source: "user".into(),
    })
}

pub(crate) fn remove_entity_attribute_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    attribute_id: &str,
) -> AppResult<()> {
    let prior: Option<(f64, String)> = conn.query_row(
        "SELECT entity_attributes.value, attribute_registry.canonical_name FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id WHERE entity_attributes.story_id = ?1 AND entity_attributes.entity_id = ?2 AND entity_attributes.attribute_id = ?3",
        rusqlite::params![story_id, entity_id, attribute_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((before, name)) = prior else {
        return Ok(());
    };
    let event = EntityEvent::AttributeRemoved {
        entity_id: entity_id.to_string(),
        attribute_id: attribute_id.to_string(),
        attribute_name: name.clone(),
        before,
        source: "user".into(),
    };
    projection::record(
        conn,
        story_id,
        None,
        &format!("User removed {name} (previously {before})."),
        &event,
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::model::kind as ledger_kind;
    use crate::shared::db::Pool;
    use serde_json::json;

    fn attribute_helper_fixture() -> (Pool, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
                 VALUES ('story', 'Story', 'now', 'now', '{}');
             INSERT INTO entities (id, story_id, kind, created_at)
                 VALUES ('entity', 'story', 'character', 'now');
             INSERT INTO story_entity_state
                 (story_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
                 VALUES ('story', 'entity', 'Mira', NULL, 1, 'now', NULL);",
        )
        .unwrap();
        let attribute_id = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Accuracy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        (pool, attribute_id)
    }

    fn attribute(min: f64, max: f64) -> AttributeRegistryEntry {
        AttributeRegistryEntry {
            id: "test".into(),
            canonical_name: "Test".into(),
            aliases_json: "[]".into(),
            entity_kinds_json: r#"["character"]"#.into(),
            min,
            max,
            category: "test".into(),
            is_user_created: false,
            created_in_story_id: None,
            created_at: "now".into(),
        }
    }

    #[test]
    fn set_attribute_helper_inserts_and_updates_with_before_payload() {
        let (pool, attribute_id) = attribute_helper_fixture();
        let conn = pool.get().unwrap();

        let inserted =
            set_entity_attribute_sync(&conn, "story", "entity", &attribute_id, 3.0).unwrap();
        let updated =
            set_entity_attribute_sync(&conn, "story", "entity", &attribute_id, 7.0).unwrap();

        assert_eq!(inserted.value, 3.0);
        assert_eq!(updated.value, 7.0);
        let mut stmt = conn
            .prepare(
                "SELECT payload_json FROM ledger_entries
                 WHERE kind = ?1 ORDER BY seq",
            )
            .unwrap();
        let payloads = stmt
            .query_map([ledger_kind::ENTITY_ATTRIBUTE_CHANGED], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .map(|row| serde_json::from_str::<serde_json::Value>(&row.unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert!(payloads[0]["before"].is_null());
        assert_eq!(payloads[0]["after"], json!(3.0));
        assert_eq!(payloads[1]["before"], json!(3.0));
        assert_eq!(payloads[1]["after"], json!(7.0));
        let stored: (f64, String) = conn
            .query_row(
                "SELECT value, source FROM entity_attributes
                 WHERE story_id = 'story' AND entity_id = 'entity' AND attribute_id = ?1",
                [&attribute_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored, (7.0, "user".into()));
    }

    #[test]
    fn remove_attribute_helper_is_idempotent() {
        let (pool, attribute_id) = attribute_helper_fixture();
        let conn = pool.get().unwrap();
        set_entity_attribute_sync(&conn, "story", "entity", &attribute_id, 4.0).unwrap();

        remove_entity_attribute_sync(&conn, "story", "entity", &attribute_id).unwrap();
        remove_entity_attribute_sync(&conn, "story", "entity", &attribute_id).unwrap();

        let stored: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_attributes", [], |row| {
                row.get(0)
            })
            .unwrap();
        let removal_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::ENTITY_ATTRIBUTE_REMOVED],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, 0);
        assert_eq!(removal_events, 1);
    }

    #[test]
    fn set_attribute_helper_rejects_invalid_or_absent_inputs() {
        let (pool, attribute_id) = attribute_helper_fixture();
        let conn = pool.get().unwrap();

        assert!(matches!(
            set_entity_attribute_sync(&conn, "story", "entity", &attribute_id, 11.0),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            set_entity_attribute_sync(&conn, "story", "missing", &attribute_id, 5.0),
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            set_entity_attribute_sync(&conn, "story", "entity", "missing", 5.0),
            Err(AppError::NotFound(_))
        ));
        let changed_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::ENTITY_ATTRIBUTE_CHANGED],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(changed_events, 0);
    }

    #[test]
    fn first_inferred_delta_records_one_event_from_the_midpoint() {
        let (pool, attribute_id) = attribute_helper_fixture();
        let conn = pool.get().unwrap();
        let passage = crate::features::ledger::repository::append_story_message(
            &conn,
            "story",
            "narrator",
            "generated",
            "Scene",
            None,
            None,
        )
        .unwrap();
        let attribute = find_attribute_by_id(&conn, &attribute_id).unwrap();

        let (before, after) = apply_attribute_delta(
            &conn,
            "story",
            "entity",
            &attribute,
            2.0,
            "first change",
            &passage.id,
            false,
            None,
        )
        .unwrap();

        let entries = crate::features::ledger::repository::list_logical_entries(&conn, "story")
            .unwrap()
            .into_iter()
            .filter(|entry| entry.kind == ledger_kind::ENTITY_ATTRIBUTE_CHANGED)
            .collect::<Vec<_>>();
        assert_eq!((before, after), (5.0, 7.0));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload["before"], json!(5.0));
        assert_eq!(entries[0].payload["after"], json!(7.0));
        assert_eq!(entries[0].payload["delta"], json!(2.0));
        assert_eq!(entries[0].payload["cause"], json!("first change"));
    }

    #[test]
    fn plain_deltas_pass_through_unchanged() {
        let attr = attribute(0.0, 10.0);
        assert_eq!(clamp_delta(5.0, 1.0, false, &attr), 6.0);
        assert_eq!(clamp_delta(5.0, -1.0, false, &attr), 4.0);
    }

    #[test]
    fn non_dramatic_deltas_are_capped_at_30_percent_of_range() {
        let attr = attribute(0.0, 10.0);
        assert_eq!(clamp_delta(5.0, 9.0, false, &attr), 8.0);
        assert_eq!(clamp_delta(5.0, -9.0, false, &attr), 2.0);
        // The cap is a rate limit, not a target: an in-range result that
        // exceeds it is still clamped by the cap.
        assert_eq!(clamp_delta(9.0, 3.0, false, &attr), 10.0);
    }

    #[test]
    fn dramatic_deltas_may_span_the_full_range_but_not_exceed_it() {
        let attr = attribute(0.0, 10.0);
        assert_eq!(clamp_delta(5.0, 4.0, true, &attr), 9.0);
        assert_eq!(clamp_delta(5.0, 20.0, true, &attr), 10.0);
        assert_eq!(clamp_delta(1.0, -20.0, true, &attr), 0.0);
    }

    #[test]
    fn negative_ranges_scale_by_width_not_absolute_value() {
        // Trust-style attribute: -10..10, so the non-dramatic cap is 6.
        let attr = attribute(-10.0, 10.0);
        assert_eq!(clamp_delta(0.0, 4.0, false, &attr), 4.0);
        assert_eq!(clamp_delta(0.0, 9.0, false, &attr), 6.0);
        assert_eq!(clamp_delta(0.0, -9.0, false, &attr), -6.0);
    }

    #[test]
    fn results_never_leave_the_attribute_bounds() {
        let attr = attribute(0.0, 10.0);
        assert_eq!(clamp_delta(0.0, -5.0, true, &attr), 0.0);
        assert_eq!(clamp_delta(10.0, 5.0, true, &attr), 10.0);
    }
}
