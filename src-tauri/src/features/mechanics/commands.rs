use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::features::timeline::{model::kind, repository as timeline};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{AttributeRegistryEntry, EntityAttributeValue, Roll, RollDetail};
use super::pipeline::DiceMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MechanicsSettings {
    pub dice_mode: String,
    pub attributes_enabled: bool,
}

fn story_settings(pool: &State<Pool>, story_id: &str) -> AppResult<Value> {
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
    let settings = story_settings(&pool, &story_id)?;
    Ok(MechanicsSettings {
        dice_mode: DiceMode::from_str_or_default(
            settings
                .get("dice_mode")
                .and_then(Value::as_str)
                .unwrap_or("classifier"),
        )
        .as_str()
        .into(),
        attributes_enabled: settings
            .get("attributes_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

#[tauri::command]
pub fn save_story_mechanics_settings(
    pool: State<Pool>,
    story_id: String,
    branch_id: String,
    dice_mode: String,
    attributes_enabled: bool,
) -> AppResult<()> {
    let mut settings = story_settings(&pool, &story_id)?;
    let dice_mode = DiceMode::from_str_or_default(&dice_mode).as_str();
    settings["dice_mode"] = json!(dice_mode);
    settings["attributes_enabled"] = json!(attributes_enabled);
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
    )?;
    timeline::append_entry(&tx, &branch_id, kind::MECHANICS_SETTINGS_CHANGED, "hidden",
        Some(&format!("Mechanics settings changed: dice mode {dice_mode}, attributes enabled {attributes_enabled}.")),
        &json!({"dice_mode": dice_mode, "attributes_enabled": attributes_enabled}), None)?;
    tx.commit()?;
    Ok(())
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

fn list_entity_attributes_sync(
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
    let mut stmt = conn.prepare("SELECT id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at FROM attribute_registry ORDER BY canonical_name ASC")?;
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
    let event = timeline::append_entry(
        &tx,
        &branch_id,
        kind::ENTITY_ATTRIBUTE_CHANGED,
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
    timeline::append_entry(
        &tx,
        &branch_id,
        kind::ENTITY_ATTRIBUTE_REMOVED,
        "hidden",
        Some(&format!("User removed {name} (previously {before}).")),
        &json!({"entity_id": entity_id, "attribute_id": attribute_id, "attribute_name": name, "before": before, "source": "user"}),
        None,
    )?;
    tx.execute("DELETE FROM entity_attributes WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3", rusqlite::params![branch_id, entity_id, attribute_id])?;
    tx.commit()?;
    Ok(())
}

fn parse_roll(entry: &crate::features::timeline::model::TimelineEntry) -> Option<Roll> {
    let p = &entry.payload;
    Some(Roll {
        id: entry.id.clone(),
        entry_id: entry.target_entry_id.clone()?,
        actor_entity_id: p.get("actor_entity_id")?.as_str()?.into(),
        target_entity_id: p
            .get("target_entity_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        actor_attribute_id: p
            .get("actor_attribute_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        target_attribute_id: p
            .get("target_attribute_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        actor_value: p.get("actor_value").and_then(Value::as_f64),
        target_value: p.get("target_value").and_then(Value::as_f64),
        p_success: p.get("p_success")?.as_f64()?,
        seed: p.get("seed")?.as_i64()?,
        roll: p.get("roll")?.as_i64()?,
        outcome: p.get("outcome")?.as_str()?.into(),
        degree: p.get("degree")?.as_str()?.into(),
        modifiers_json: p
            .get("modifiers")
            .cloned()
            .unwrap_or_else(|| json!({}))
            .to_string(),
        created_at: entry.created_at.clone(),
    })
}

fn entity_name(conn: &rusqlite::Connection, branch_id: &str, id: &str) -> String {
    conn.query_row(
        "SELECT name FROM branch_entity_state WHERE branch_id = ?1 AND entity_id = ?2",
        rusqlite::params![branch_id, id],
        |r| r.get(0),
    )
    .unwrap_or_else(|_| id.to_string())
}
fn attribute_name(conn: &rusqlite::Connection, id: Option<&str>) -> Option<String> {
    id.and_then(|id| {
        conn.query_row(
            "SELECT canonical_name FROM attribute_registry WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    })
}
fn detail(
    conn: &rusqlite::Connection,
    branch_id: &str,
    roll: Roll,
    snapshots: bool,
) -> AppResult<RollDetail> {
    let actor_name = entity_name(conn, branch_id, &roll.actor_entity_id);
    let target_name = roll
        .target_entity_id
        .as_deref()
        .map(|id| entity_name(conn, branch_id, id));
    let actor_attribute_name = attribute_name(conn, roll.actor_attribute_id.as_deref());
    let target_attribute_name = attribute_name(conn, roll.target_attribute_id.as_deref());
    let actor_attributes = if snapshots {
        list_entity_attributes_sync(conn, branch_id, &roll.actor_entity_id)?
    } else {
        Vec::new()
    };
    let target_attributes = if snapshots {
        roll.target_entity_id
            .as_deref()
            .map(|id| list_entity_attributes_sync(conn, branch_id, id))
            .transpose()?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(RollDetail {
        roll,
        actor_name,
        actor_attribute_name,
        target_name,
        target_attribute_name,
        actor_attributes,
        target_attributes,
    })
}

#[tauri::command]
pub fn list_rolls_for_entry(pool: State<Pool>, entry_id: String) -> AppResult<Vec<Roll>> {
    let conn = pool.get()?;
    let base = timeline::get_entry(&conn, &entry_id)?;
    Ok(timeline::list_logical_entries(&conn, &base.branch_id)?
        .iter()
        .filter(|e| {
            e.kind == kind::MECHANICAL_RESULT && e.target_entry_id.as_deref() == Some(&entry_id)
        })
        .filter_map(parse_roll)
        .collect())
}

#[tauri::command]
pub fn list_rolls_for_branch(pool: State<Pool>, branch_id: String) -> AppResult<Vec<RollDetail>> {
    let conn = pool.get()?;
    timeline::list_logical_entries(&conn, &branch_id)?
        .iter()
        .filter(|e| e.kind == kind::MECHANICAL_RESULT)
        .filter_map(parse_roll)
        .map(|roll| detail(&conn, &branch_id, roll, false))
        .collect()
}

#[tauri::command]
pub fn get_roll_detail(pool: State<Pool>, entry_id: String) -> AppResult<Option<RollDetail>> {
    let conn = pool.get()?;
    let base = timeline::get_entry(&conn, &entry_id)?;
    let roll = timeline::list_logical_entries(&conn, &base.branch_id)?
        .iter()
        .rev()
        .find(|e| {
            e.kind == kind::MECHANICAL_RESULT && e.target_entry_id.as_deref() == Some(&entry_id)
        })
        .and_then(parse_roll);
    roll.map(|roll| detail(&conn, &base.branch_id, roll, true))
        .transpose()
}
