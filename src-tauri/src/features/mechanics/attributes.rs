//! The attribute registry: dedup so the model can't freely invent
//! Evasion/Dodge/Agility as three incomparable stats, plus the read/write
//! helpers for per-entity attribute values.

use chrono::Utc;
use rig_agent::prelude::*;
use rig_core::embeddings::distance::VectorDistance;
use rig_core::providers::openrouter;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::features::timeline::{model::kind as timeline_kind, repository::append_entry};
use crate::shared::db::{Pool, PooledConn};
use crate::shared::error::{AppError, AppResult};

use super::model::AttributeRegistryEntry;

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

fn find_exact_match(
    conn: &PooledConn,
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
    conn: &PooledConn,
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

fn add_alias(conn: &PooledConn, attribute_id: &str, alias: &str) -> AppResult<()> {
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

fn mint_new(
    conn: &PooledConn,
    proposed_name: &str,
    entity_kind: &str,
    story_id: &str,
) -> AppResult<AttributeRegistryEntry> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let kinds_json = serde_json::to_string(&[entity_kind]).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO attribute_registry
         (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
         VALUES (?1, ?2, '[]', ?3, 0.0, 10.0, 'user', 1, ?4, ?5)",
        rusqlite::params![id, proposed_name.trim(), kinds_json, story_id, now],
    )?;
    Ok(AttributeRegistryEntry {
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
    })
}

/// Resolves a model-proposed attribute name against the registry: exact
/// match (name or alias) wins immediately; otherwise an embedding
/// similarity pass against same-kind candidates either registers the
/// proposal as an alias of the closest match or, below threshold, mints a
/// genuinely new canonical entry flagged `is_user_created`.
pub async fn resolve_or_create_attribute(
    pool: &Pool,
    api_key: &str,
    proposed_name: &str,
    entity_kind: &str,
    story_id: &str,
) -> AppResult<AttributeRegistryEntry> {
    let proposed_name = proposed_name.trim();
    if proposed_name.is_empty() {
        return Err(AppError::Invalid("attribute name must not be empty".into()));
    }

    if let Some(entry) = {
        let conn = pool.get()?;
        find_exact_match(&conn, proposed_name)?
    } {
        return Ok(entry);
    }

    let candidates = {
        let conn = pool.get()?;
        load_registry_for_kind(&conn, entity_kind)?
    };
    if candidates.is_empty() {
        let conn = pool.get()?;
        return mint_new(&conn, proposed_name, entity_kind, story_id);
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
        let conn = pool.get()?;
        return mint_new(&conn, proposed_name, entity_kind, story_id);
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
            let conn = pool.get()?;
            add_alias(&conn, &entry.id, proposed_name)?;
            return Ok(entry.clone());
        }
    }

    let conn = pool.get()?;
    mint_new(&conn, proposed_name, entity_kind, story_id)
}

/// Reads an entity's current value for an attribute, initializing it to the
/// attribute's midpoint on first use (a fresh entity has no story yet — it
/// shouldn't start maxed or bottomed on a stat nobody's set).
pub fn get_or_init_entity_attribute(
    conn: &PooledConn,
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

/// Applies a proposed delta with clamping and rate-limiting, and logs an
/// append-only timeline event. Returns `(before, after)`.
#[allow(clippy::too_many_arguments)]
pub fn apply_attribute_delta(
    conn: &PooledConn,
    branch_id: &str,
    entity_id: &str,
    attribute: &AttributeRegistryEntry,
    delta: f64,
    cause: &str,
    entry_id: &str,
    dramatic: bool,
) -> AppResult<(f64, f64)> {
    let before = get_or_init_entity_attribute(conn, branch_id, entity_id, attribute, entry_id)?;
    let range = attribute.max - attribute.min;
    let max_step = if dramatic {
        range
    } else {
        range * NON_DRAMATIC_MAX_FRACTION
    };
    let clamped_delta = delta.clamp(-max_step, max_step);
    let after = (before + clamped_delta).clamp(attribute.min, attribute.max);

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
