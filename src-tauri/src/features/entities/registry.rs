//! Attribute registry lookup, deduplication, aliasing, and embedding resolution.

use chrono::Utc;
use rig_agent::prelude::*;
use rig_core::embeddings::distance::VectorDistance;
use rig_core::providers::openrouter;
use uuid::Uuid;

use crate::shared::db::Pool;
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

pub(crate) fn list_attribute_registry_sync(
    conn: &rusqlite::Connection,
) -> AppResult<Vec<AttributeRegistryEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLUMNS} FROM attribute_registry ORDER BY canonical_name ASC"
    ))?;
    let rows = stmt.query_map([], row_to_entry)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn find_exact_match(
    conn: &rusqlite::Connection,
    proposed_name: &str,
) -> AppResult<Option<AttributeRegistryEntry>> {
    let needle = proposed_name.trim();
    let mut stmt = conn.prepare(&format!("SELECT {SELECT_COLUMNS} FROM attribute_registry"))?;
    let rows = stmt.query_map([], row_to_entry)?;
    for r in rows {
        let entry = r?;
        if entry.canonical_name.eq_ignore_ascii_case(needle) {
            return Ok(Some(entry));
        }
        let aliases: Vec<String> = serde_json::from_str(&entry.aliases_json).unwrap_or_default();
        if aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(needle))
        {
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
    if let Some(existing) = find_exact_match(conn, &entry.canonical_name)? {
        return Ok(existing.id);
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
    find_exact_match(conn, &entry.canonical_name)?
        .map(|existing| existing.id)
        .ok_or_else(|| {
            AppError::Other(format!(
                "minted attribute was not found after insert: {}",
                entry.canonical_name
            ))
        })
}

pub(crate) enum AttributeResolution {
    Existing(AttributeRegistryEntry),
    AddAlias {
        attribute: AttributeRegistryEntry,
        alias: String,
    },
    Mint(AttributeRegistryEntry),
}

/// Decides how a proposed attribute should resolve. The caller applies the
/// returned registry operation immediately before staging dependent writes.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_attribute_remaps_when_name_becomes_an_alias_before_commit() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let staged = build_minted_attribute("Resonance", "artifact", "story");
        let existing = find_exact_match(&conn, "Accuracy").unwrap().unwrap();

        add_alias(&conn, &existing.id, "Resonance").unwrap();
        let resolved_id = insert_minted_attribute(&conn, &staged).unwrap();

        assert_eq!(resolved_id, existing.id);
        let duplicate_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attribute_registry WHERE lower(canonical_name) = lower('Resonance')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(duplicate_count, 0);
    }
}
