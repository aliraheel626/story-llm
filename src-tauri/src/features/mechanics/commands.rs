use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{AttributeRegistryEntry, EntityAttributeValue, Roll, RollDetail};
use super::pipeline::DiceMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MechanicsSettings {
    pub dice_mode: String,
    pub attributes_enabled: bool,
}

fn read_story_settings_json(pool: &State<Pool>, story_id: &str) -> AppResult<serde_json::Value> {
    let conn = pool.get()?;
    let raw: String = conn
        .query_row(
            "SELECT settings_json FROM stories WHERE id = ?1",
            [story_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("story {story_id} not found")))?;
    Ok(serde_json::from_str(&raw).unwrap_or_else(|_| json!({})))
}

#[tauri::command]
pub fn get_story_mechanics_settings(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<MechanicsSettings> {
    let settings = read_story_settings_json(&pool, &story_id)?;
    let dice_mode = settings
        .get("dice_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("classifier")
        .to_string();
    let attributes_enabled = settings
        .get("attributes_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    Ok(MechanicsSettings {
        dice_mode: DiceMode::from_str_or_default(&dice_mode)
            .as_str()
            .to_string(),
        attributes_enabled,
    })
}

#[tauri::command]
pub fn save_story_mechanics_settings(
    pool: State<Pool>,
    story_id: String,
    dice_mode: String,
    attributes_enabled: bool,
) -> AppResult<()> {
    let mut settings = read_story_settings_json(&pool, &story_id)?;
    let dice_mode = DiceMode::from_str_or_default(&dice_mode).as_str();
    settings["dice_mode"] = json!(dice_mode);
    settings["attributes_enabled"] = json!(attributes_enabled);
    let conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![settings.to_string(), now, story_id],
    )?;
    Ok(())
}

fn row_to_entity_attribute(row: &rusqlite::Row) -> rusqlite::Result<EntityAttributeValue> {
    Ok(EntityAttributeValue {
        entity_id: row.get(0)?,
        attribute_id: row.get(1)?,
        canonical_name: row.get(2)?,
        value: row.get(3)?,
        min: row.get(4)?,
        max: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn list_entity_attributes_sync(
    conn: &rusqlite::Connection,
    entity_id: &str,
) -> AppResult<Vec<EntityAttributeValue>> {
    let mut stmt = conn.prepare(
        "SELECT entity_attributes.entity_id, entity_attributes.attribute_id, attribute_registry.canonical_name,
                entity_attributes.value, attribute_registry.min, attribute_registry.max, entity_attributes.updated_at
         FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id
         WHERE entity_attributes.entity_id = ?1
         ORDER BY attribute_registry.canonical_name ASC",
    )?;
    let rows = stmt.query_map([entity_id], row_to_entity_attribute)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[tauri::command]
pub fn list_entity_attributes(
    pool: State<Pool>,
    entity_id: String,
) -> AppResult<Vec<EntityAttributeValue>> {
    let conn = pool.get()?;
    list_entity_attributes_sync(&conn, &entity_id)
}

#[tauri::command]
pub fn list_attribute_registry(pool: State<Pool>) -> AppResult<Vec<AttributeRegistryEntry>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at
         FROM attribute_registry ORDER BY canonical_name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
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
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn row_to_roll(row: &rusqlite::Row) -> rusqlite::Result<Roll> {
    Ok(Roll {
        id: row.get(0)?,
        passage_id: row.get(1)?,
        actor_entity_id: row.get(2)?,
        target_entity_id: row.get(3)?,
        actor_attribute_id: row.get(4)?,
        target_attribute_id: row.get(5)?,
        actor_value: row.get(6)?,
        target_value: row.get(7)?,
        p_success: row.get(8)?,
        seed: row.get(9)?,
        roll: row.get(10)?,
        outcome: row.get(11)?,
        degree: row.get(12)?,
        modifiers_json: row.get(13)?,
        created_at: row.get(14)?,
    })
}

const ROLL_COLUMNS: &str = "rolls.id, rolls.passage_id, rolls.actor_entity_id, rolls.target_entity_id, rolls.actor_attribute_id, rolls.target_attribute_id,
     rolls.actor_value, rolls.target_value, rolls.p_success, rolls.seed, rolls.roll, rolls.outcome, rolls.degree, rolls.modifiers_json, rolls.created_at";

#[tauri::command]
pub fn list_rolls_for_passage(pool: State<Pool>, passage_id: String) -> AppResult<Vec<Roll>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {ROLL_COLUMNS} FROM rolls WHERE passage_id = ?1 ORDER BY created_at ASC"
    ))?;
    let rows = stmt.query_map([passage_id], row_to_roll)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Every roll for every passage in a branch, in one call — mirrors
/// `list_images_for_branch` so the frontend can mark which passages have a
/// roll without one query per passage. Enriched with names (so the
/// collapsed summary can say "Stealth (You) vs Perception (Mira)" instead of
/// bare ids) but not the full attribute snapshots — those are the lazy
/// `get_roll_detail` fetch on expand.
#[tauri::command]
pub fn list_rolls_for_branch(pool: State<Pool>, branch_id: String) -> AppResult<Vec<RollDetail>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {ROLL_COLUMNS}, actor.name, actor_attr.canonical_name, target.name, target_attr.canonical_name
         FROM rolls
         JOIN passages ON passages.id = rolls.passage_id
         JOIN entities actor ON actor.id = rolls.actor_entity_id
         LEFT JOIN attribute_registry actor_attr ON actor_attr.id = rolls.actor_attribute_id
         LEFT JOIN entities target ON target.id = rolls.target_entity_id
         LEFT JOIN attribute_registry target_attr ON target_attr.id = rolls.target_attribute_id
         WHERE passages.branch_id = ?1 ORDER BY rolls.created_at ASC"
    ))?;
    let rows = stmt.query_map([branch_id], |row| {
        Ok(RollDetail {
            roll: row_to_roll(row)?,
            actor_name: row.get(15)?,
            actor_attribute_name: row.get(16)?,
            target_name: row.get(17)?,
            target_attribute_name: row.get(18)?,
            actor_attributes: Vec::new(),
            target_attributes: Vec::new(),
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn get_entity_name(conn: &rusqlite::Connection, entity_id: &str) -> AppResult<String> {
    conn.query_row(
        "SELECT name FROM entities WHERE id = ?1",
        [entity_id],
        |r| r.get(0),
    )
    .map_err(|_| AppError::NotFound(format!("entity {entity_id} not found")))
}

fn get_attribute_name(conn: &rusqlite::Connection, attribute_id: &str) -> AppResult<String> {
    conn.query_row(
        "SELECT canonical_name FROM attribute_registry WHERE id = ?1",
        [attribute_id],
        |r| r.get(0),
    )
    .map_err(|_| AppError::NotFound(format!("attribute {attribute_id} not found")))
}

/// The transparency ("why") disclosure for one passage's roll: names,
/// attribute names, and each side's full current attribute snapshot — not
/// just the two attributes the roll itself used.
#[tauri::command]
pub fn get_roll_detail(pool: State<Pool>, passage_id: String) -> AppResult<Option<RollDetail>> {
    let conn = pool.get()?;
    let roll: Option<Roll> = conn
        .query_row(&format!("SELECT {ROLL_COLUMNS} FROM rolls WHERE passage_id = ?1 ORDER BY created_at DESC LIMIT 1"), [&passage_id], row_to_roll)
        .optional()
        .map_err(AppError::from)?;
    let Some(roll) = roll else { return Ok(None) };

    let actor_name = get_entity_name(&conn, &roll.actor_entity_id)?;
    let actor_attribute_name = match &roll.actor_attribute_id {
        Some(id) => Some(get_attribute_name(&conn, id)?),
        None => None,
    };
    let target_name = match &roll.target_entity_id {
        Some(id) => Some(get_entity_name(&conn, id)?),
        None => None,
    };
    let target_attribute_name = match &roll.target_attribute_id {
        Some(id) => Some(get_attribute_name(&conn, id)?),
        None => None,
    };

    let actor_attributes = list_entity_attributes_sync(&conn, &roll.actor_entity_id)?;
    let target_attributes = match &roll.target_entity_id {
        Some(id) => list_entity_attributes_sync(&conn, id)?,
        None => Vec::new(),
    };

    Ok(Some(RollDetail {
        roll,
        actor_name,
        actor_attribute_name,
        target_name,
        target_attribute_name,
        actor_attributes,
        target_attributes,
    }))
}
