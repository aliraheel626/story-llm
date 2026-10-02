use chrono::Utc;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

use super::{
    events::{EntityEvent, NameAnchor},
    model::{relationship_id, CharacterFields, CharacterPatch, Entity, EntityLink, CHARACTER, RELATIONSHIP},
    projection,
};

const ENTITY_SELECT: &str = "SELECT e.id, e.story_id, e.kind,
    COALESCE(e.name, f.name || CASE l.direction WHEN 'both' THEN ' \u{2194} ' ELSE ' \u{2192} ' END || t.name || ': ' || l.label, e.id),
    c.appearance_anchor, e.created_at, l.from_id, l.to_id, l.label, l.direction, l.description,
    c.known_as, c.gender, c.age, c.role, c.location, c.outfit
    FROM entities e
    LEFT JOIN characters c ON c.entity_id = e.id
    LEFT JOIN relationships l ON l.entity_id = e.id
    LEFT JOIN entities f ON f.id = l.from_id AND f.story_id = e.story_id
        AND f.kind = 'character' AND f.is_present = 1
    LEFT JOIN entities t ON t.id = l.to_id AND t.story_id = e.story_id
        AND t.kind = 'character' AND t.is_present = 1
    WHERE e.story_id = ?1 AND e.is_present = 1";

fn row_to_entity(row: &rusqlite::Row) -> rusqlite::Result<Entity> {
    let link = match row.get::<_, Option<String>>(6)? {
        Some(from_id) => Some(EntityLink {
            from_id,
            to_id: row.get(7)?,
            label: row.get(8)?,
            direction: row.get(9)?,
            description: row.get(10)?,
        }),
        None => None,
    };
    Ok(Entity {
        id: row.get(0)?,
        story_id: row.get(1)?,
        kind: row.get(2)?,
        name: row.get(3)?,
        character: CharacterFields {
            known_as: row.get(11)?,
            appearance_anchor: row.get(4)?,
            gender: row.get(12)?,
            age: row.get(13)?,
            role: row.get(14)?,
            location: row.get(15)?,
            outfit: row.get(16)?,
        },
        created_at: row.get(5)?,
        link,
    })
}

/// Stored present state, including links hidden because an endpoint is absent.
pub fn load_entity_raw(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
) -> AppResult<Option<Entity>> {
    Ok(conn
        .query_row(
            &format!("{ENTITY_SELECT} AND e.id = ?2"),
            rusqlite::params![story_id, entity_id],
            row_to_entity,
        )
        .optional()?)
}

pub fn list_entities_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    filter_kind: Option<&str>,
) -> AppResult<Vec<Entity>> {
    let mut sql = format!(
        "{ENTITY_SELECT} AND (l.entity_id IS NULL OR (f.id IS NOT NULL AND t.id IS NOT NULL))"
    );
    if filter_kind.is_some() {
        sql.push_str(" AND e.kind = ?2");
    }
    sql.push_str(" ORDER BY e.created_at ASC");
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

fn recorded_content(source: &str, text: String) -> String {
    if source == "user" {
        format!("User edit: {text}")
    } else {
        text
    }
}

fn trimmed_text(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub(crate) fn validated_name(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name must not be empty".into()));
    }
    if name.contains('\u{2192}') || name.contains("->") {
        return Err(AppError::Invalid("names can't contain arrows".into()));
    }
    Ok(name)
}

fn snapshot(entity: &Entity) -> NameAnchor {
    NameAnchor {
        name: entity.name.clone(),
        character: entity.character.clone(),
        link: entity.link.clone(),
    }
}

fn link_name(link: &EntityLink, from: &Entity, to: &Entity) -> String {
    let arrow = if link.direction == "both" {
        '\u{2194}'
    } else {
        '\u{2192}'
    };
    format!("{} {arrow} {}: {}", from.name, to.name, link.label)
}

fn validate_link(
    conn: &rusqlite::Connection,
    story_id: &str,
    link: &EntityLink,
    excluding_id: Option<&str>,
) -> AppResult<(Entity, Entity)> {
    if link.label.trim().is_empty() {
        return Err(AppError::Invalid("label must not be empty".into()));
    }
    if !matches!(link.direction.as_str(), "one_way" | "both") {
        return Err(AppError::Invalid("direction must be one_way or both".into()));
    }
    if link.from_id == link.to_id {
        return Err(AppError::Invalid("relationship endpoints must be distinct".into()));
    }
    let endpoint = |id: &str| {
        load_entity_raw(conn, story_id, id)?
            .filter(|entity| entity.kind == CHARACTER)
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "endpoint {id} must be a present character in this story"
                ))
            })
    };
    let from = endpoint(&link.from_id)?;
    let to = endpoint(&link.to_id)?;
    let conflicting_id: Option<String> = conn
        .query_row(
            "SELECT e.id FROM relationships l JOIN entities e ON e.id = l.entity_id
             WHERE e.story_id = ?1 AND e.is_present = 1
               AND (?5 IS NULL OR e.id != ?5)
               AND ((l.from_id = ?2 AND l.to_id = ?3)
                 OR (l.from_id = ?3 AND l.to_id = ?2
                   AND (?4 = 'both' OR l.direction = 'both')))
             LIMIT 1",
            rusqlite::params![story_id, link.from_id, link.to_id, link.direction, excluding_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = conflicting_id {
        return Err(AppError::Invalid(format!(
            "relationship conflicts with existing relationship {id}"
        )));
    }
    Ok((from, to))
}

#[allow(clippy::too_many_arguments)]
fn record_create(
    conn: &rusqlite::Connection,
    id: &str,
    story_id: &str,
    kind: &str,
    name: &str,
    character: CharacterFields,
    link: Option<EntityLink>,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    let character = character.normalize()?;
    let identity: Option<(String, String)> = conn
        .query_row(
            "SELECT story_id, kind FROM entities WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if identity.is_some_and(|(stored_story, stored_kind)| stored_story != story_id || stored_kind != kind) {
        return Err(AppError::Invalid(format!("entity {id} belongs to a different story or kind")));
    }
    let text = if kind == RELATIONSHIP {
        format!("{name} (relationship recorded).")
    } else {
        format!("{name} was added as a character.")
    };
    let event = EntityEvent::Created {
        entity_id: id.to_string(),
        kind: kind.to_string(),
        name: (kind == CHARACTER).then(|| name.to_string()),
        character,
        link,
        source: source.to_string(),
        created_at: Utc::now().to_rfc3339(),
    };
    projection::record(
        conn,
        story_id,
        target_entry_id,
        &recorded_content(source, text),
        &event,
        turn_id,
    )?;
    load_entity_raw(conn, story_id, id)?
        .ok_or_else(|| AppError::Other("created entity was not projected".into()))
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
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    if entity_kind != CHARACTER {
        return Err(AppError::Invalid("kind must be character".into()));
    }
    let name = validated_name(name)?;
    if load_entity_raw(conn, story_id, id)?.is_some() {
        return update_entity_sync(
            conn, story_id, id, name, appearance_anchor, source, target_entry_id, turn_id,
        );
    }
    record_create(
        conn,
        id,
        story_id,
        CHARACTER,
        name,
        CharacterFields {
            appearance_anchor: trimmed_text(appearance_anchor),
            ..Default::default()
        },
        None,
        source,
        target_entry_id,
        turn_id,
    )
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
        None,
    )
}

pub fn create_character_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    name: &str,
    fields: CharacterFields,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    record_create(
        conn, &Uuid::new_v4().to_string(), story_id, CHARACTER, validated_name(name)?,
        fields, None, source, target_entry_id, turn_id,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_entity_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    name: &str,
    appearance_anchor: Option<&str>,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    update_character_sync(
        conn, story_id, entity_id, Some(name),
        &CharacterPatch {
            appearance_anchor: Some(appearance_anchor.map(str::to_string)),
            ..Default::default()
        },
        source, target_entry_id, turn_id,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_character_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    name: Option<&str>,
    patch: &CharacterPatch,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    let before = load_entity_raw(conn, story_id, entity_id)?
        .ok_or_else(|| AppError::NotFound(format!("entity {entity_id} not found")))?;
    if before.kind != CHARACTER {
        return Err(AppError::Invalid("kind must be character".into()));
    }
    let patch = patch.clone().normalize()?;
    let after = Entity {
        name: match name {
            Some(name) => validated_name(name)?.to_string(),
            None => before.name.clone(),
        },
        character: patch.apply_to(&before.character),
        ..before.clone()
    };
    let mut changes = Vec::new();
    if before.name != after.name {
        changes.push(format!("name {} \u{2192} {}", before.name, after.name));
    }
    for ((field, old), (_, new)) in before.character.fields().into_iter().zip(after.character.fields()) {
        if old == new {
            continue;
        }
        let label = match field {
            "known_as" => "known as",
            "appearance_anchor" => "appearance",
            _ => field,
        };
        changes.push(match new {
            Some(value) if field == "appearance_anchor" => format!("{label} \u{2192} {value:?}"),
            Some(value) => match old.as_deref() {
                Some(previous) => format!("{label} {previous} \u{2192} {value}"),
                None => format!("{label} \u{2192} {value}"),
            },
            None => format!("{label} cleared"),
        });
    }
    if changes.is_empty() {
        return Ok(before);
    }
    let event = EntityEvent::Updated {
        entity_id: entity_id.to_string(),
        before: snapshot(&before),
        after: Box::new(snapshot(&after)),
        source: source.to_string(),
    };
    projection::record(
        conn,
        story_id,
        target_entry_id,
        &recorded_content(source, format!("{}: {}.", before.name, changes.join("; "))),
        &event,
        turn_id,
    )?;
    Ok(after)
}

pub fn create_link_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    mut link: EntityLink,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    link.label = link.label.trim().to_string();
    link.description = trimmed_text(link.description.as_deref());
    let requested_id = relationship_id(&link.from_id, &link.to_id);
    let reversed_id = relationship_id(&link.to_id, &link.from_id);
    // A present reversed mutual link wins over a same-orientation tombstone.
    let stored: Option<(String, String, String)> = conn
        .query_row(
            "SELECT e.id, l.from_id, l.to_id
             FROM entities e JOIN relationships l ON l.entity_id = e.id
             WHERE e.story_id = ?1 AND (e.id = ?2
               OR (?4 = 'both' AND e.id = ?3
                 AND (e.is_present = 0 OR l.direction = 'both')))
             ORDER BY e.is_present DESC, (e.id = ?2) DESC LIMIT 1",
            rusqlite::params![story_id, requested_id, reversed_id, link.direction],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let id = if let Some((id, from_id, to_id)) = stored {
        link.from_id = from_id;
        link.to_id = to_id;
        id
    } else {
        requested_id
    };
    let (from, to) = validate_link(conn, story_id, &link, Some(&id))?;
    if let Some(before) = load_entity_raw(conn, story_id, &id)? {
        if before.link.as_ref().is_some_and(|old| old.direction != link.direction) {
            return Err(AppError::Invalid(format!(
                "relationship conflicts with existing relationship {}", before.id
            )));
        }
        return update_link_sync(
            conn, story_id, &id, Some(&link.label), Some(&link.direction),
            Some(link.description.clone()), source, target_entry_id, turn_id,
        );
    }
    record_create(
        conn,
        &id,
        story_id,
        RELATIONSHIP,
        &link_name(&link, &from, &to),
        CharacterFields::default(),
        Some(link),
        source,
        target_entry_id,
        turn_id,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_link_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    label: Option<&str>,
    direction: Option<&str>,
    description: Option<Option<String>>,
    source: &str,
    target_entry_id: Option<&str>,
    turn_id: Option<&str>,
) -> AppResult<Entity> {
    let before = load_entity_raw(conn, story_id, entity_id)?
        .ok_or_else(|| AppError::NotFound(format!("entity {entity_id} not found")))?;
    if before.kind != RELATIONSHIP {
        return Err(AppError::Invalid("kind must be relationship".into()));
    }
    let old_link = before.link.as_ref()
        .ok_or_else(|| AppError::Invalid("relationship has no stored link".into()))?;
    let mut link = old_link.clone();
    if let Some(label) = label {
        link.label = label.trim().to_string();
    }
    if let Some(direction) = direction {
        link.direction = direction.to_string();
    }
    if let Some(description) = description {
        link.description = trimmed_text(description.as_deref());
    }
    let (from, to) = validate_link(conn, story_id, &link, Some(entity_id))?;
    if *old_link == link {
        return Ok(before);
    }
    let mut changes = Vec::new();
    if old_link.label != link.label {
        changes.push(format!("label {} \u{2192} {}", old_link.label, link.label));
    }
    if old_link.direction != link.direction {
        changes.push(format!("direction {} \u{2192} {}", old_link.direction, link.direction));
    }
    if old_link.description != link.description {
        changes.push(match &link.description {
            Some(description) => format!("description \u{2192} {description:?}"),
            None => "description cleared".into(),
        });
    }
    let after = Entity {
        name: link_name(&link, &from, &to),
        link: Some(link),
        ..before.clone()
    };
    let event = EntityEvent::Updated {
        entity_id: entity_id.to_string(),
        before: snapshot(&before),
        after: Box::new(snapshot(&after)),
        source: source.to_string(),
    };
    projection::record(
        conn,
        story_id,
        target_entry_id,
        &recorded_content(source, format!("{}: {}.", before.name, changes.join("; "))),
        &event,
        turn_id,
    )?;
    Ok(after)
}

pub(crate) fn delete_entity_sync(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
) -> AppResult<()> {
    let before = load_entity_raw(conn, story_id, entity_id)?
        .ok_or_else(|| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let event = EntityEvent::Deleted {
        entity_id: entity_id.to_string(),
        name: before.name.clone(),
        source: "user".into(),
    };
    projection::record(
        conn,
        story_id,
        None,
        &recorded_content("user", format!(
            "{} was removed from the authoritative entity state.", before.name
        )),
        &event,
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{entities::attributes, transcript};
    use crate::shared::{db::{with_transaction, Pool}, test_support};
    use serde_json::{json, Value};

    fn fixture() -> Pool {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "story");
        for (id, name) in [("mira", "Mira"), ("varro", "Varro")] {
            create_entity_with_id_sync(
                &conn, id, "story", CHARACTER, name, None, "user", None, None,
            ).unwrap();
        }
        drop(conn);
        pool
    }

    fn link(from: &str, to: &str, direction: &str) -> EntityLink {
        EntityLink {
            from_id: from.into(), to_id: to.into(), label: "friends".into(),
            direction: direction.into(), description: Some("shared history".into()),
        }
    }

    fn entries(conn: &rusqlite::Connection) -> Vec<transcript::model::TranscriptEntry> {
        transcript::repository::list_logical_entries(conn, "story").unwrap()
    }

    fn stored_link(conn: &rusqlite::Connection, id: &str) -> Value {
        conn.query_row(
            "SELECT e.name, c.appearance_anchor, e.is_present, e.created_at, e.updated_at,
                    l.from_id, l.to_id, l.label, l.direction, l.description
             FROM entities e JOIN relationships l ON l.entity_id = e.id
             LEFT JOIN characters c ON c.entity_id = e.id
             WHERE e.story_id = 'story' AND e.id = ?1",
            [id], |row| Ok(json!({
                "name": row.get::<_, Option<String>>(0)?,
                "appearance_anchor": row.get::<_, Option<String>>(1)?,
                "present": row.get::<_, i64>(2)?,
                "created_at": row.get::<_, String>(3)?,
                "updated_at": row.get::<_, String>(4)?,
                "from_id": row.get::<_, String>(5)?, "to_id": row.get::<_, String>(6)?,
                "label": row.get::<_, String>(7)?, "direction": row.get::<_, String>(8)?,
                "description": row.get::<_, Option<String>>(9)?,
            })),
        ).unwrap()
    }

    #[test]
    fn link_validation_rejects_invalid_endpoints_fields_and_mutual_conflicts() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "other");
        create_entity_with_id_sync(
            &conn, "outsider", "other", CHARACTER, "Outsider", None, "user", None, None,
        ).unwrap();
        create_entity_with_id_sync(
            &conn, "gone", "story", CHARACTER, "Gone", None, "user", None, None,
        ).unwrap();
        delete_entity_sync(&conn, "story", "gone").unwrap();
        let original = create_link_sync(
            &conn, "story", link("mira", "varro", "one_way"), "user", None, None,
        ).unwrap();
        let mut empty_label = link("mira", "varro", "one_way");
        empty_label.label = "  ".into();
        let before = entries(&conn);
        for invalid in [
            link("mira", "missing", "one_way"), link("mira", "mira", "one_way"),
            link("mira", &original.id, "one_way"), link("mira", "outsider", "one_way"),
            link("mira", "gone", "one_way"), link("mira", "varro", "bad"), empty_label,
            link("varro", "mira", "both"),
        ] {
            assert!(matches!(
                create_link_sync(&conn, "story", invalid, "user", None, None),
                Err(AppError::Invalid(_))
            ));
        }
        assert_eq!(entries(&conn), before);
        assert!(matches!(
            create_link_sync(&conn, "story", link("mira", "varro", "both"), "user", None, None),
            Err(AppError::Invalid(message)) if message.contains(&original.id)
        ));
        let mutual = update_link_sync(
            &conn, "story", &original.id, None, Some("both"), None, "user", None, None,
        ).unwrap();
        assert!(matches!(
            create_link_sync(&conn, "story", link("varro", "mira", "one_way"), "user", None, None),
            Err(AppError::Invalid(message)) if message.contains(&mutual.id)
        ));
        assert!(matches!(
            create_link_sync(&conn, "story", link("mira", "varro", "one_way"), "user", None, None),
            Err(AppError::Invalid(message)) if message.contains(&mutual.id)
        ));
    }

    #[test]
    fn character_writers_reject_other_kinds_and_arrow_names_without_events() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let before = entries(&conn);
        for kind in [RELATIONSHIP, "invalid"] {
            assert!(matches!(
                create_entity_with_id_sync(&conn, "new", "story", kind, "New", None, "user", None, None),
                Err(AppError::Invalid(message)) if message == "kind must be character"
            ));
            assert!(matches!(
                create_entity_sync(&conn, "story", kind, "New", None, "user", None),
                Err(AppError::Invalid(_))
            ));
        }
        for name in [" ", "Mira -> Varro", "Mira \u{2192} Varro"] {
            assert!(matches!(
                create_entity_with_id_sync(&conn, "new", "story", CHARACTER, name, None, "user", None, None),
                Err(AppError::Invalid(_))
            ));
            assert!(matches!(
                update_entity_sync(&conn, "story", "mira", name, None, "user", None, None),
                Err(AppError::Invalid(_))
            ));
        }
        assert_eq!(entries(&conn), before);
        assert_eq!(load_entity_raw(&conn, "story", "mira").unwrap().unwrap().name, "Mira");
        assert!(load_entity_raw(&conn, "story", "new").unwrap().is_none());
    }

    #[test]
    fn relationship_pair_index_counts_present_and_soft_deleted_rows() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let original = create_link_sync(
            &conn, "story", link("mira", "varro", "one_way"), "user", None, None,
        ).unwrap();
        for deleted in [false, true] {
            if deleted {
                delete_entity_sync(&conn, "story", &original.id).unwrap();
            }
            let before = stored_link(&conn, &original.id);
            conn.execute_batch("SAVEPOINT duplicate_probe").unwrap();
            conn.execute(
                "INSERT INTO entities (id, story_id, kind, name, is_present, created_at, updated_at)
                 VALUES ('duplicate', 'story', 'relationship', NULL, 1, 'now', 'now')", [],
            ).unwrap();
            let error = conn.execute(
                "INSERT INTO relationships (entity_id, from_id, to_id, label, direction)
                 VALUES ('duplicate', 'mira', 'varro', 'rivals', 'one_way')", [],
            ).unwrap_err();
            assert!(matches!(
                error, rusqlite::Error::SqliteFailure(error, _)
                    if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            ));
            conn.execute_batch("ROLLBACK TO duplicate_probe; RELEASE duplicate_probe").unwrap();
            assert_eq!(stored_link(&conn, &original.id), before);
            assert!(before["name"].is_null());
            assert!(load_entity_raw(&conn, "story", "duplicate").unwrap().is_none());
            assert_eq!(list_entities_sync(&conn, "story", Some(RELATIONSHIP)).unwrap().len(), usize::from(!deleted));
        }
    }

    #[test]
    fn reviving_links_reuses_id_orientation_created_at_and_stats() {
        for (direction, reverse) in [("one_way", false), ("both", true)] {
            let pool = fixture();
            let conn = pool.get().unwrap();
            let original = create_link_sync(
                &conn, "story", link("mira", "varro", direction), "user", None, None,
            ).unwrap();
            let affection: String = conn.query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Affection'", [], |row| row.get(0),
            ).unwrap();
            attributes::set_entity_attribute_sync(&conn, "story", &original.id, &affection, -3.0).unwrap();
            let stats = serde_json::to_value(
                attributes::list_entity_attributes_sync(&conn, "story", &original.id).unwrap(),
            ).unwrap();
            delete_entity_sync(&conn, "story", &original.id).unwrap();
            let mut request = if reverse { link("varro", "mira", direction) } else { link("mira", "varro", direction) };
            request.label = " allies ".into();
            request.description = Some(" renewed history ".into());
            let revived = create_link_sync(&conn, "story", request.clone(), "user", None, None).unwrap();
            assert_eq!(revived.id, original.id);
            assert_eq!(revived.created_at, original.created_at);
            let stored = revived.link.as_ref().unwrap();
            assert_eq!((stored.from_id.as_str(), stored.to_id.as_str()), ("mira", "varro"));
            assert_eq!(stored.label, "allies");
            assert_eq!(stored.description.as_deref(), Some("renewed history"));
            assert_eq!(serde_json::to_value(
                attributes::list_entity_attributes_sync(&conn, "story", &revived.id).unwrap(),
            ).unwrap(), stats);
            let before = entries(&conn);
            let again = create_link_sync(&conn, "story", request, "user", None, None).unwrap();
            assert_eq!(serde_json::to_value(&again).unwrap(), serde_json::to_value(&revived).unwrap());
            assert_eq!(entries(&conn), before);
            assert_eq!(conn.query_row(
                "SELECT COUNT(*) FROM relationships WHERE (from_id = 'mira' AND to_id = 'varro')
                    OR (from_id = 'varro' AND to_id = 'mira')", [], |row| row.get::<_, i64>(0),
            ).unwrap(), 1);
            assert!(stored_link(&conn, &revived.id)["name"].is_null());
        }
    }

    #[test]
    fn present_reversed_mutual_link_wins_over_direct_tombstone() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let forward = create_link_sync(
            &conn, "story", link("mira", "varro", "one_way"), "user", None, None,
        ).unwrap();
        let reverse = create_link_sync(
            &conn, "story", link("varro", "mira", "one_way"), "user", None, None,
        ).unwrap();
        delete_entity_sync(&conn, "story", &forward.id).unwrap();
        let mutual = update_link_sync(
            &conn, "story", &reverse.id, None, Some("both"), None, "user", None, None,
        ).unwrap();
        let tombstone = stored_link(&conn, &forward.id);
        let before = entries(&conn);
        let reused = create_link_sync(
            &conn, "story", link("mira", "varro", "both"), "user", None, None,
        ).unwrap();
        assert_eq!(reused.id, reverse.id);
        assert_eq!(snapshot(&reused), snapshot(&mutual));
        assert_eq!(entries(&conn), before);
        assert_eq!(stored_link(&conn, &forward.id), tombstone);
        assert_eq!(conn.query_row(
            "SELECT COUNT(*) FROM relationships l JOIN entities e ON e.id = l.entity_id
             WHERE e.story_id = 'story' AND e.is_present = 1 AND l.direction = 'both'",
            [], |row| row.get::<_, i64>(0),
        ).unwrap(), 1);
    }

    #[test]
    fn displayed_links_follow_renames_and_return_after_endpoint_deletion_is_erased() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let original = create_link_sync(
            &conn, "story", link("mira", "varro", "both"), "user", None, None,
        ).unwrap();
        update_entity_sync(&conn, "story", "mira", "Mira Vale", None, "user", None, None).unwrap();
        update_entity_sync(&conn, "story", "varro", "Varro Grey", None, "user", None, None).unwrap();
        assert_eq!(list_entities_sync(&conn, "story", Some(RELATIONSHIP)).unwrap()[0].name,
            "Mira Vale \u{2194} Varro Grey: friends");
        let stored_before = stored_link(&conn, &original.id);
        let (turn_id, _, narration) = test_support::exchange(&conn, "story", "do", "leave", Some("Mira leaves."));
        projection::record(
            &conn, "story", narration.as_deref(), "Mira Vale was removed.",
            &EntityEvent::Deleted { entity_id: "mira".into(), name: "Mira Vale".into(), source: "narrator_tool".into() },
            Some(&turn_id),
        ).unwrap();
        assert!(list_entities_sync(&conn, "story", Some(RELATIONSHIP)).unwrap().is_empty());
        let raw = load_entity_raw(&conn, "story", &original.id).unwrap().unwrap();
        assert_eq!(raw.name, original.id);
        assert_eq!(raw.link, original.link);
        assert_eq!(stored_link(&conn, &original.id), stored_before);
        drop(conn);
        with_transaction(&pool, |tx| {
            let removed = transcript::erase::erase_last_exchange_in_tx(tx, "story")?.unwrap();
            projection::replay_after_erase(tx, "story", &removed.entries)
        }).unwrap();
        let conn = pool.get().unwrap();
        assert_eq!(list_entities_sync(&conn, "story", Some(RELATIONSHIP)).unwrap()[0].name,
            "Mira Vale \u{2194} Varro Grey: friends");
        assert_eq!(stored_link(&conn, &original.id), stored_before);
    }

    #[test]
    fn hidden_link_mutations_use_raw_state_without_overwriting_link_or_null_name() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let original = create_link_sync(
            &conn, "story", link("mira", "varro", "one_way"), "user", None, None,
        ).unwrap();
        delete_entity_sync(&conn, "story", "mira").unwrap();
        update_entity_sync(&conn, "story", "varro", "Varro Grey", None, "user", None, None).unwrap();
        let stored_before = stored_link(&conn, &original.id);
        let before = entries(&conn);
        assert!(list_entities_sync(&conn, "story", Some(RELATIONSHIP)).unwrap().is_empty());
        let raw = load_entity_raw(&conn, "story", &original.id).unwrap().unwrap();
        assert_eq!(raw.name, original.id);
        assert_eq!(raw.link, original.link);
        assert!(load_entity_raw(&conn, "other", &original.id).unwrap().is_none());
        for label in [None, Some("former friends")] {
            assert!(matches!(
                update_link_sync(&conn, "story", &original.id, label, None, None, "user", None, None),
                Err(AppError::Invalid(message)) if message.contains("mira")
            ));
        }
        assert!(matches!(
            update_entity_sync(&conn, "story", &original.id, "Not a character", None, "user", None, None),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            create_entity_with_id_sync(&conn, &original.id, "story", CHARACTER, "Not a character", None, "user", None, None),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            delete_entity_sync(&conn, "other", &original.id), Err(AppError::NotFound(_))
        ));
        assert_eq!(entries(&conn), before);
        assert_eq!(stored_link(&conn, &original.id), stored_before);
        assert!(stored_before["name"].is_null());
        delete_entity_sync(&conn, "story", &original.id).unwrap();
        assert!(load_entity_raw(&conn, "story", &original.id).unwrap().is_none());
        let deletion = entries(&conn).pop().unwrap();
        assert!(deletion.content.unwrap().starts_with(&format!("User edit: {} was removed", original.id)));
        assert!(stored_link(&conn, &original.id)["name"].is_null());
        let deleted_before = stored_link(&conn, &original.id);
        let before = entries(&conn);
        assert!(matches!(
            create_entity_with_id_sync(&conn, &original.id, "story", CHARACTER, "Not a character", None, "user", None, None),
            Err(AppError::Invalid(_))
        ));
        assert_eq!(stored_link(&conn, &original.id), deleted_before);
        assert_eq!(entries(&conn), before);
    }

    #[test]
    fn link_updates_keep_endpoints_record_full_snapshots_actual_diffs_and_noops() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let original = create_link_sync(
            &conn, "story", link("mira", "varro", "one_way"), "user", None, None,
        ).unwrap();
        let created = entries(&conn).pop().unwrap();
        assert!(created.content.as_deref().unwrap().starts_with("User edit: Mira \u{2192} Varro"));
        assert!(created.payload["name"].is_null());
        assert_eq!(created.payload["link"], json!(original.link));
        let before = entries(&conn);
        let stored_before = stored_link(&conn, &original.id);
        let same = update_link_sync(
            &conn, "story", &original.id, Some(" friends "), Some("one_way"),
            Some(Some(" shared history ".into())), "user", None, None,
        ).unwrap();
        assert_eq!(snapshot(&same), snapshot(&original));
        assert_eq!(entries(&conn), before);
        assert_eq!(stored_link(&conn, &original.id), stored_before);
        for (label, direction) in [(Some(" "), None), (None, Some("bad"))] {
            assert!(matches!(
                update_link_sync(&conn, "story", &original.id, label, direction, None, "user", None, None),
                Err(AppError::Invalid(_))
            ));
        }
        assert_eq!(entries(&conn), before);
        assert!(matches!(
            update_link_sync(&conn, "story", "mira", None, None, None, "user", None, None),
            Err(AppError::Invalid(_))
        ));
        for (story_id, id) in [("other", original.id.as_str()), ("story", "missing")] {
            assert!(matches!(
                update_link_sync(&conn, story_id, id, None, None, None, "user", None, None),
                Err(AppError::NotFound(_))
            ));
        }
        assert_eq!(entries(&conn), before);
        let renamed = update_link_sync(
            &conn, "story", &original.id, Some("former friends"), None, None, "user", None, None,
        ).unwrap();
        let entry = entries(&conn).pop().unwrap();
        let content = entry.content.as_deref().unwrap();
        assert!(content.starts_with("User edit: "));
        assert!(content.contains("label friends \u{2192} former friends"));
        assert!(!content.contains("direction") && !content.contains("description"));
        let EntityEvent::Updated { before, after, .. } = EntityEvent::from_entry(&entry).unwrap() else { panic!("expected Updated") };
        assert_eq!(before, snapshot(&original));
        assert_eq!(*after, snapshot(&renamed));
        let reverse = create_link_sync(
            &conn, "story", link("varro", "mira", "one_way"), "user", None, None,
        ).unwrap();
        let entries_before = entries(&conn);
        assert!(matches!(
            update_link_sync(&conn, "story", &original.id, None, Some("both"), None, "user", None, None),
            Err(AppError::Invalid(message)) if message.contains(&reverse.id)
        ));
        assert_eq!(entries(&conn), entries_before);
        delete_entity_sync(&conn, "story", &reverse.id).unwrap();
        let mutual = update_link_sync(
            &conn, "story", &original.id, None, Some("both"), Some(None), "narrator_tool", None, None,
        ).unwrap();
        let entry = entries(&conn).pop().unwrap();
        let content = entry.content.as_deref().unwrap();
        assert!(!content.starts_with("User edit: "));
        assert!(content.contains("direction one_way \u{2192} both") && content.contains("description cleared"));
        assert!(!content.contains("label "));
        let final_link = mutual.link.as_ref().unwrap();
        assert_eq!((final_link.from_id.as_str(), final_link.to_id.as_str()), ("mira", "varro"));
        assert_eq!(final_link.label, "former friends");
        assert_eq!(final_link.description, None);
        assert_eq!(mutual.name, "Mira \u{2194} Varro: former friends");
        assert!(stored_link(&conn, &original.id)["name"].is_null());
        let unchanged = entries(&conn);
        update_link_sync(&conn, "story", &original.id, None, None, None, "user", None, None).unwrap();
        assert_eq!(entries(&conn), unchanged);
    }

    #[test]
    fn character_updates_record_actual_name_appearance_diffs_and_noops() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let original = load_entity_raw(&conn, "story", "mira").unwrap().unwrap();
        let before = entries(&conn);
        let same = update_entity_sync(&conn, "story", "mira", " Mira ", Some(" "), "user", None, None).unwrap();
        assert_eq!(snapshot(&same), snapshot(&original));
        assert_eq!(entries(&conn), before);
        let renamed = update_entity_sync(&conn, "story", "mira", "Mira Vale", None, "user", None, None).unwrap();
        let entry = entries(&conn).pop().unwrap();
        assert_eq!(entry.content.as_deref(), Some("User edit: Mira: name Mira \u{2192} Mira Vale."));
        let EntityEvent::Updated { before, after, .. } = EntityEvent::from_entry(&entry).unwrap() else { panic!("expected Updated") };
        assert_eq!(before, snapshot(&original));
        assert_eq!(*after, snapshot(&renamed));
        let appearance = update_entity_sync(&conn, "story", "mira", "Mira Vale", Some(" silver hair "), "narrator_tool", None, None).unwrap();
        assert_eq!(entries(&conn).pop().unwrap().content.as_deref(), Some("Mira Vale: appearance \u{2192} \"silver hair\"."));
        assert_eq!(appearance.character.appearance_anchor.as_deref(), Some("silver hair"));
        assert_eq!(appearance.created_at, original.created_at);
        update_entity_sync(&conn, "story", "mira", "Mira Vale", None, "user", None, None).unwrap();
        assert_eq!(entries(&conn).pop().unwrap().content.as_deref(), Some("User edit: Mira Vale: appearance cleared."));
        let before = entries(&conn);
        create_entity_with_id_sync(&conn, "mira", "story", CHARACTER, "Mira Vale", None, "user", None, None).unwrap();
        assert_eq!(entries(&conn), before);
        assert!(load_entity_raw(&conn, "other", "mira").unwrap().is_none());
    }

    #[test]
    fn deserialized_character_patch_sets_clears_preserves_and_records_only_changes() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let fields = json!({
            "known_as": "the stranger", "appearance_anchor": "silver hair", "gender": "woman",
            "age": "30", "role": "pirate", "location": "tavern", "outfit": "cloak",
        });
        let original = create_character_sync(
            &conn, "story", " Kael ", serde_json::from_value(fields.clone()).unwrap(), "user", None, None,
        ).unwrap();
        assert_eq!(serde_json::to_value(&original.character).unwrap(), fields);
        assert_eq!(original.name, "Kael");
        let patch: CharacterPatch = serde_json::from_value(json!({
            "known_as": " ", "location": " the docks ", "outfit": null,
        })).unwrap();
        assert_eq!(patch.outfit, Some(None));
        assert_eq!(patch.role, None);
        let count = entries(&conn).len();
        let updated = update_character_sync(
            &conn, "story", &original.id, None, &patch, "user", None, None,
        ).unwrap();
        assert_eq!(entries(&conn).len(), count + 1);
        assert_eq!(serde_json::to_value(&updated.character).unwrap(), json!({
            "known_as": null, "appearance_anchor": "silver hair", "gender": "woman", "age": "30",
            "role": "pirate", "location": "the docks", "outfit": null,
        }));
        let entry = entries(&conn).pop().unwrap();
        assert_eq!(entry.content.as_deref(), Some(
            "User edit: Kael: known as cleared; location tavern \u{2192} the docks; outfit cleared."
        ));
        let EntityEvent::Updated { before, after, .. } = EntityEvent::from_entry(&entry).unwrap() else { panic!("expected Updated") };
        assert_eq!(before, snapshot(&original));
        assert_eq!(*after, snapshot(&updated));
        assert_eq!(snapshot(&load_entity_raw(&conn, "story", &original.id).unwrap().unwrap()), snapshot(&updated));
        for unchanged in [patch, serde_json::from_value(json!({})).unwrap()] {
            let same = update_character_sync(
                &conn, "story", &original.id, Some(" Kael "), &unchanged, "user", None, None,
            ).unwrap();
            assert_eq!(snapshot(&same), snapshot(&updated));
        }
        assert_eq!(entries(&conn).len(), count + 1);
    }

    #[test]
    fn short_character_fields_count_unicode_scalars_and_anchor_is_unlimited() {
        let pool = fixture();
        let conn = pool.get().unwrap();
        let count = entries(&conn).len();
        for field in ["known_as", "gender", "age", "role", "location", "outfit"] {
            let mut input = json!({});
            input[field] = json!("x".repeat(201));
            assert!(matches!(
                create_character_sync(&conn, "story", "Invalid", serde_json::from_value(input.clone()).unwrap(), "user", None, None),
                Err(AppError::Invalid(_))
            ));
            let patch = serde_json::from_value(input).unwrap();
            assert!(matches!(
                update_character_sync(&conn, "story", "mira", None, &patch, "user", None, None),
                Err(AppError::Invalid(_))
            ));
        }
        assert_eq!(entries(&conn).len(), count);
        let boundary = "\u{754c}".repeat(200);
        let fields = serde_json::from_value(json!({
            "known_as": boundary, "gender": boundary, "age": boundary, "role": boundary,
            "location": boundary, "outfit": boundary, "appearance_anchor": "a".repeat(401),
        })).unwrap();
        let created = create_character_sync(&conn, "story", "Boundary", fields, "user", None, None).unwrap();
        assert_eq!(created.character.known_as.as_deref(), Some(boundary.as_str()));
        assert_eq!(created.character.appearance_anchor.as_deref().unwrap().len(), 401);
        let patch = serde_json::from_value(json!({"role": boundary, "appearance_anchor": "b".repeat(501)})).unwrap();
        let updated = update_character_sync(&conn, "story", "mira", None, &patch, "user", None, None).unwrap();
        assert_eq!(updated.character.role.as_deref(), Some(boundary.as_str()));
        assert_eq!(updated.character.appearance_anchor.as_deref().unwrap().len(), 501);
    }
}
