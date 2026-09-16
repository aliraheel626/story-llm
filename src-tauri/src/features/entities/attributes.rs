//! The attribute registry: dedup so the model can't freely invent
//! Evasion/Dodge/Agility as three incomparable stats, plus the read/write
//! helpers for per-entity attribute values.

use std::collections::HashMap;

use chrono::Utc;
use rig_agent::prelude::*;
use rig_core::embeddings::distance::VectorDistance;
use rig_core::providers::openrouter;
use rusqlite::OptionalExtension;
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::features::timeline::{model::kind as timeline_kind, repository::append_entry};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{AttributeRegistryEntry, EntityAttributeValue};

const EMBEDDING_MODEL: &str = "openai/text-embedding-3-small";
const SIMILARITY_THRESHOLD: f64 = 0.85;

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<AttributeRegistryEntry> {
    Ok(AttributeRegistryEntry {
        id: row.get(0)?,
        canonical_name: row.get(1)?,
        aliases_json: row.get(2)?,
        entity_kinds_json: row.get(3)?,
        min: row.get(4)?,
        max: row.get(5)?,
        category: row.get(6)?,
        is_user_created: row.get::<_, i64>(7)? != 0,
        created_in_story_id: row.get(8)?,
        created_at: row.get(9)?,
    })
}

const SELECT_COLUMNS: &str =
    "id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at";

pub(crate) fn find_exact_match(
    conn: &rusqlite::Connection,
    proposed_name: &str,
) -> AppResult<Option<AttributeRegistryEntry>> {
    let needle = proposed_name.trim().to_lowercase();
    let mut stmt = conn.prepare(&format!("SELECT {SELECT_COLUMNS} FROM attribute_registry"))?;
    let rows = stmt.query_map([], row_to_entry)?;
    for r in rows {
        let entry = r?;
        if entry.canonical_name.to_lowercase() == needle {
            return Ok(Some(entry));
        }
        let aliases: Vec<String> = serde_json::from_str(&entry.aliases_json).unwrap_or_default();
        if aliases.iter().any(|a| a.to_lowercase() == needle) {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

fn load_registry_for_kind(
    conn: &rusqlite::Connection,
    entity_kind: &str,
) -> AppResult<Vec<AttributeRegistryEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLUMNS} FROM attribute_registry
         WHERE EXISTS (SELECT 1 FROM json_each(entity_kinds_json) WHERE json_each.value = ?1)"
    ))?;
    let rows = stmt.query_map([entity_kind], row_to_entry)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub(crate) fn find_attribute_by_id(
    conn: &rusqlite::Connection,
    attribute_id: &str,
) -> AppResult<AttributeRegistryEntry> {
    conn.query_row(
        &format!("SELECT {SELECT_COLUMNS} FROM attribute_registry WHERE id = ?1"),
        [attribute_id],
        row_to_entry,
    )
    .map_err(Into::into)
}

pub(crate) fn add_alias(
    conn: &rusqlite::Connection,
    attribute_id: &str,
    alias: &str,
) -> AppResult<()> {
    let current: String = conn.query_row(
        "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
        [attribute_id],
        |r| r.get(0),
    )?;
    let mut aliases: Vec<String> = serde_json::from_str(&current).unwrap_or_default();
    if !aliases.iter().any(|a| a.eq_ignore_ascii_case(alias)) {
        aliases.push(alias.to_string());
        let updated = serde_json::to_string(&aliases).unwrap_or(current);
        conn.execute(
            "UPDATE attribute_registry SET aliases_json = ?1 WHERE id = ?2",
            rusqlite::params![updated, attribute_id],
        )?;
    }
    Ok(())
}

fn build_minted_attribute(
    proposed_name: &str,
    entity_kind: &str,
    story_id: &str,
) -> AttributeRegistryEntry {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let kinds_json = serde_json::to_string(&[entity_kind]).unwrap_or_else(|_| "[]".to_string());
    AttributeRegistryEntry {
        id,
        canonical_name: proposed_name.trim().to_string(),
        aliases_json: "[]".to_string(),
        entity_kinds_json: kinds_json,
        min: 0.0,
        max: 10.0,
        category: "user".to_string(),
        is_user_created: true,
        created_in_story_id: Some(story_id.to_string()),
        created_at: now,
    }
}

pub(crate) fn insert_minted_attribute(
    conn: &rusqlite::Connection,
    entry: &AttributeRegistryEntry,
) -> AppResult<String> {
    if let Some(id) = find_canonical_id_case_insensitive(conn, &entry.canonical_name)? {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO attribute_registry
         (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT DO NOTHING",
        rusqlite::params![
            entry.id,
            entry.canonical_name,
            entry.aliases_json,
            entry.entity_kinds_json,
            entry.min,
            entry.max,
            entry.category,
            entry.is_user_created,
            entry.created_in_story_id,
            entry.created_at,
        ],
    )?;
    find_canonical_id_case_insensitive(conn, &entry.canonical_name)?.ok_or_else(|| {
        AppError::Other(format!(
            "minted attribute was not found after insert: {}",
            entry.canonical_name
        ))
    })
}

fn find_canonical_id_case_insensitive(
    conn: &rusqlite::Connection,
    canonical_name: &str,
) -> AppResult<Option<String>> {
    let needle = canonical_name.trim().to_lowercase();
    let mut stmt = conn.prepare("SELECT id, canonical_name FROM attribute_registry")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, candidate) = row?;
        if candidate.to_lowercase() == needle {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

pub(crate) enum AttributeResolution {
    Existing(AttributeRegistryEntry),
    AddAlias {
        attribute: AttributeRegistryEntry,
        alias: String,
    },
    Mint(AttributeRegistryEntry),
}

impl AttributeResolution {
    pub(crate) fn attribute(&self) -> &AttributeRegistryEntry {
        match self {
            Self::Existing(attribute)
            | Self::AddAlias { attribute, .. }
            | Self::Mint(attribute) => attribute,
        }
    }
}

/// Decides how a proposed attribute should resolve without writing anything.
/// Callers can apply the returned registry operation immediately or stage it
/// alongside a larger transaction.
pub(crate) async fn resolve_attribute(
    pool: &Pool,
    api_key: &str,
    proposed_name: &str,
    entity_kind: &str,
    story_id: &str,
) -> AppResult<AttributeResolution> {
    let proposed_name = proposed_name.trim();
    if proposed_name.is_empty() {
        return Err(AppError::Invalid("attribute name must not be empty".into()));
    }

    if let Some(entry) = {
        let conn = pool.get()?;
        find_exact_match(&conn, proposed_name)?
    } {
        return Ok(AttributeResolution::Existing(entry));
    }

    let candidates = {
        let conn = pool.get()?;
        load_registry_for_kind(&conn, entity_kind)?
    };
    if candidates.is_empty() {
        return Ok(AttributeResolution::Mint(build_minted_attribute(
            proposed_name,
            entity_kind,
            story_id,
        )));
    }

    let client = openrouter::Client::builder()
        .api_key(api_key.to_string())
        .build()
        .map_err(|e| AppError::Other(format!("failed to build OpenRouter client: {e}")))?;
    let model = client.embedding_model(EMBEDDING_MODEL);

    let mut texts = vec![proposed_name.to_string()];
    texts.extend(candidates.iter().map(|c| c.canonical_name.clone()));
    let embeddings = model
        .embed_texts(texts)
        .await
        .map_err(|e| AppError::Other(format!("attribute embedding request failed: {e}")))?;

    let Some((proposed_emb, candidate_embs)) = embeddings.split_first() else {
        return Ok(AttributeResolution::Mint(build_minted_attribute(
            proposed_name,
            entity_kind,
            story_id,
        )));
    };

    let mut best: Option<(f64, &AttributeRegistryEntry)> = None;
    for (candidate, emb) in candidates.iter().zip(candidate_embs.iter()) {
        let similarity = proposed_emb.cosine_similarity(emb, false);
        if best.as_ref().is_none_or(|(s, _)| similarity > *s) {
            best = Some((similarity, candidate));
        }
    }

    if let Some((similarity, entry)) = best {
        if similarity >= SIMILARITY_THRESHOLD {
            return Ok(AttributeResolution::AddAlias {
                attribute: entry.clone(),
                alias: proposed_name.to_string(),
            });
        }
    }

    Ok(AttributeResolution::Mint(build_minted_attribute(
        proposed_name,
        entity_kind,
        story_id,
    )))
}

/// Reads an entity's current value for an attribute without writing
/// anything — unlike `get_or_init_entity_attribute`, a "just checking" read
/// (e.g. a narrator tool call previewing state mid-turn, before anything is
/// committed) must not persist an init event as a side effect. The optional
/// source is `None` for the implicit midpoint and identifies player-locked
/// rows without a second write-oriented lookup.
pub fn peek_entity_attribute(
    conn: &rusqlite::Connection,
    branch_id: &str,
    entity_id: &str,
    attribute: &AttributeRegistryEntry,
) -> AppResult<(f64, Option<String>)> {
    let existing: Option<(f64, String)> = conn
        .query_row(
            "SELECT value, source FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
            rusqlite::params![branch_id, entity_id, attribute.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match existing {
        Some((value, source)) => (value, Some(source)),
        None => ((attribute.min + attribute.max) / 2.0, None),
    })
}

/// Reads an entity's current value for an attribute, initializing it to the
/// attribute's midpoint on first use (a fresh entity has no story yet — it
/// shouldn't start maxed or bottomed on a stat nobody's set).
pub fn get_or_init_entity_attribute(
    conn: &rusqlite::Connection,
    branch_id: &str,
    entity_id: &str,
    attribute: &AttributeRegistryEntry,
    source_entry_id: &str,
) -> AppResult<f64> {
    let existing: Option<f64> = conn
        .query_row(
            "SELECT value FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
            rusqlite::params![branch_id, entity_id, attribute.id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(v) = existing {
        return Ok(v);
    }
    let midpoint = (attribute.min + attribute.max) / 2.0;
    let now = Utc::now().to_rfc3339();
    let event = append_entry(
        conn,
        branch_id,
        timeline_kind::ENTITY_ATTRIBUTE_CHANGED,
        "hidden",
        Some(&format!(
            "{} was initialized to {}.",
            attribute.canonical_name, midpoint
        )),
        &serde_json::json!({"entity_id": entity_id, "attribute_id": attribute.id, "attribute_name": attribute.canonical_name,
            "before": null, "after": midpoint, "source": "default"}),
        Some(source_entry_id),
    )?;
    conn.execute(
        "INSERT INTO entity_attributes (branch_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
         VALUES (?1, ?2, ?3, ?4, 'default', ?5, ?6)",
        rusqlite::params![branch_id, entity_id, attribute.id, midpoint, now, event.id],
    )?;
    Ok(midpoint)
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
/// append-only timeline event. Returns `(before, after)`.
#[allow(clippy::too_many_arguments)]
pub fn apply_attribute_delta(
    conn: &rusqlite::Connection,
    branch_id: &str,
    entity_id: &str,
    attribute: &AttributeRegistryEntry,
    delta: f64,
    cause: &str,
    entry_id: &str,
    dramatic: bool,
) -> AppResult<(f64, f64)> {
    let before = get_or_init_entity_attribute(conn, branch_id, entity_id, attribute, entry_id)?;
    let current_source: String = conn.query_row(
        "SELECT source FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
        rusqlite::params![branch_id, entity_id, attribute.id],
        |r| r.get(0),
    )?;
    if current_source == "user" {
        // The narrator preamble tells the model user overrides take
        // precedence over inferred updates; honor that here rather than
        // silently overwriting a value the player explicitly set.
        return Ok((before, before));
    }
    let after = clamp_delta(before, delta, dramatic, attribute);

    let now = Utc::now().to_rfc3339();
    let event = append_entry(
        conn,
        branch_id,
        timeline_kind::ENTITY_ATTRIBUTE_CHANGED,
        "hidden",
        Some(&format!(
            "{} changed from {} to {}: {cause}",
            attribute.canonical_name, before, after
        )),
        &serde_json::json!({"entity_id": entity_id, "attribute_id": attribute.id, "attribute_name": attribute.canonical_name,
            "before": before, "after": after, "delta": after - before, "cause": cause, "source": "inferred"}),
        Some(entry_id),
    )?;
    conn.execute(
        "UPDATE entity_attributes SET value = ?1, source = 'inferred', updated_at = ?2, last_event_id = ?3
         WHERE branch_id = ?4 AND entity_id = ?5 AND attribute_id = ?6",
        rusqlite::params![after, now, event.id, branch_id, entity_id, attribute.id],
    )?;
    Ok((before, after))
}

fn row_to_entity_attribute(row: &rusqlite::Row) -> rusqlite::Result<EntityAttributeValue> {
    Ok(EntityAttributeValue {
        branch_id: row.get(0)?,
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
    branch_id: &str,
    entity_id: &str,
) -> AppResult<Vec<EntityAttributeValue>> {
    let mut stmt = conn.prepare(
        "SELECT entity_attributes.branch_id, entity_attributes.entity_id, entity_attributes.attribute_id, attribute_registry.canonical_name,
                entity_attributes.value, attribute_registry.min, attribute_registry.max, entity_attributes.updated_at, entity_attributes.source
         FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id
         WHERE entity_attributes.branch_id = ?1 AND entity_attributes.entity_id = ?2 ORDER BY attribute_registry.canonical_name ASC")?;
    let rows = stmt.query_map(
        rusqlite::params![branch_id, entity_id],
        row_to_entity_attribute,
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Loads committed attributes for many entities with one registry join.
/// Entries for requested entities with no stored attributes are empty.
pub(crate) fn list_entity_attributes_for_entities_sync(
    conn: &rusqlite::Connection,
    branch_id: &str,
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
        "SELECT entity_attributes.branch_id, entity_attributes.entity_id, entity_attributes.attribute_id, attribute_registry.canonical_name,
                entity_attributes.value, attribute_registry.min, attribute_registry.max, entity_attributes.updated_at, entity_attributes.source
         FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id
         WHERE entity_attributes.branch_id = ? AND entity_attributes.entity_id IN ({placeholders})
         ORDER BY entity_attributes.entity_id, attribute_registry.canonical_name ASC"
    ))?;
    let params = std::iter::once(branch_id).chain(entity_ids.iter().copied());
    let rows = stmt.query_map(rusqlite::params_from_iter(params), row_to_entity_attribute)?;
    for row in rows {
        let attribute = row?;
        out.entry(attribute.entity_id.clone())
            .or_default()
            .push(attribute);
    }
    Ok(out)
}

#[tauri::command]
pub fn list_entity_attributes(
    pool: State<Pool>,
    branch_id: String,
    entity_id: String,
) -> AppResult<Vec<EntityAttributeValue>> {
    let conn = pool.get()?;
    list_entity_attributes_sync(&conn, &branch_id, &entity_id)
}

#[tauri::command]
pub fn list_attribute_registry(pool: State<Pool>) -> AppResult<Vec<AttributeRegistryEntry>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLUMNS} FROM attribute_registry ORDER BY canonical_name ASC"
    ))?;
    let rows = stmt.query_map([], row_to_entry)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[tauri::command]
pub fn set_entity_attribute(
    pool: State<Pool>,
    branch_id: String,
    entity_id: String,
    attribute_id: String,
    value: f64,
) -> AppResult<EntityAttributeValue> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (name, min, max): (String, f64, f64) = tx
        .query_row(
            "SELECT canonical_name, min, max FROM attribute_registry WHERE id = ?1",
            [&attribute_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| AppError::NotFound(format!("attribute {attribute_id} not found")))?;
    if !value.is_finite() || value < min || value > max {
        return Err(AppError::Invalid(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    tx.query_row("SELECT 1 FROM branch_entity_state WHERE branch_id = ?1 AND entity_id = ?2 AND is_present = 1", rusqlite::params![branch_id, entity_id], |_| Ok(()))
        .map_err(|_| AppError::NotFound(format!("entity {entity_id} not found")))?;
    let before: Option<f64> = tx.query_row("SELECT value FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3", rusqlite::params![branch_id, entity_id, attribute_id], |r| r.get(0)).optional()?;
    let event = append_entry(
        &tx,
        &branch_id,
        timeline_kind::ENTITY_ATTRIBUTE_CHANGED,
        "hidden",
        Some(&format!(
            "User changed {name} from {} to {value}.",
            before
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unset".into())
        )),
        &json!({"entity_id": entity_id, "attribute_id": attribute_id, "attribute_name": name, "before": before, "after": value, "source": "user"}),
        None,
    )?;
    let now = Utc::now().to_rfc3339();
    tx.execute("INSERT INTO entity_attributes (branch_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
                VALUES (?1, ?2, ?3, ?4, 'user', ?5, ?6)
                ON CONFLICT(branch_id, entity_id, attribute_id) DO UPDATE SET value=excluded.value, source='user', updated_at=excluded.updated_at, last_event_id=excluded.last_event_id",
        rusqlite::params![branch_id, entity_id, attribute_id, value, now, event.id])?;
    tx.commit()?;
    Ok(EntityAttributeValue {
        branch_id,
        entity_id,
        attribute_id,
        canonical_name: name,
        value,
        min,
        max,
        updated_at: now,
        source: "user".into(),
    })
}

#[tauri::command]
pub fn remove_entity_attribute(
    pool: State<Pool>,
    branch_id: String,
    entity_id: String,
    attribute_id: String,
) -> AppResult<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let prior: Option<(f64, String)> = tx.query_row(
        "SELECT entity_attributes.value, attribute_registry.canonical_name FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id WHERE entity_attributes.branch_id = ?1 AND entity_attributes.entity_id = ?2 AND entity_attributes.attribute_id = ?3",
        rusqlite::params![branch_id, entity_id, attribute_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((before, name)) = prior else {
        return Ok(());
    };
    append_entry(
        &tx,
        &branch_id,
        timeline_kind::ENTITY_ATTRIBUTE_REMOVED,
        "hidden",
        Some(&format!("User removed {name} (previously {before}).")),
        &json!({"entity_id": entity_id, "attribute_id": attribute_id, "attribute_name": name, "before": before, "source": "user"}),
        None,
    )?;
    tx.execute("DELETE FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3", rusqlite::params![branch_id, entity_id, attribute_id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
