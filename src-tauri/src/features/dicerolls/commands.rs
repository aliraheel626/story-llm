use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::features::timeline::{model::kind, repository as timeline};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use crate::features::entities::attributes::list_entity_attributes_sync;

use super::model::{DiceMode, Roll, RollDetail};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DicerollSettings {
    pub dice_mode: String,
    pub attributes_enabled: bool,
    /// OpenRouter reasoning effort for narration, e.g. "low" or "high". `None`
    /// leaves the model at its own default.
    pub reasoning_effort: Option<String>,
}

/// The effort levels OpenRouter accepts; anything else is ignored rather than
/// forwarded, so a stale setting can't make every narration 400.
pub fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        _ => None,
    }
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
pub fn get_story_diceroll_settings(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<DicerollSettings> {
    let settings = story_settings(&pool, &story_id)?;
    Ok(DicerollSettings {
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
        reasoning_effort: settings
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .and_then(normalize_reasoning_effort)
            .map(str::to_string),
    })
}

#[tauri::command]
pub fn save_story_diceroll_settings(
    pool: State<Pool>,
    story_id: String,
    branch_id: String,
    dice_mode: String,
    attributes_enabled: bool,
    reasoning_effort: Option<String>,
) -> AppResult<()> {
    let mut settings = story_settings(&pool, &story_id)?;
    let dice_mode = DiceMode::from_str_or_default(&dice_mode).as_str();
    // Unrecognized levels are rejected rather than stored, so the story never
    // carries an effort the provider would refuse.
    let reasoning_effort = reasoning_effort
        .as_deref()
        .and_then(normalize_reasoning_effort)
        .map(str::to_string);
    settings["dice_mode"] = json!(dice_mode);
    settings["attributes_enabled"] = json!(attributes_enabled);
    match &reasoning_effort {
        Some(effort) => settings["reasoning_effort"] = json!(effort),
        None => {
            if let Some(object) = settings.as_object_mut() {
                object.remove("reasoning_effort");
            }
        }
    }
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
    )?;
    timeline::append_entry(&tx, &branch_id, kind::DICEROLL_SETTINGS_CHANGED, "hidden",
        Some(&format!("Dice-roll settings changed: dice mode {dice_mode}, attributes enabled {attributes_enabled}, reasoning effort {}.",
            reasoning_effort.as_deref().unwrap_or("model default"))),
        &json!({"dice_mode": dice_mode, "attributes_enabled": attributes_enabled, "reasoning_effort": reasoning_effort}), None)?;
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
        .filter(|e| e.kind == kind::DICEROLL && e.target_entry_id.as_deref() == Some(&entry_id))
        .filter_map(parse_roll)
        .collect())
}

#[tauri::command]
pub fn list_rolls_for_branch(pool: State<Pool>, branch_id: String) -> AppResult<Vec<RollDetail>> {
    let conn = pool.get()?;
    timeline::list_logical_entries(&conn, &branch_id)?
        .iter()
        .filter(|e| e.kind == kind::DICEROLL)
        .filter_map(parse_roll)
        .map(|roll| detail(&conn, &branch_id, roll, false))
        .collect()
}

#[tauri::command]
pub fn list_roll_details_for_entry(
    pool: State<Pool>,
    entry_id: String,
) -> AppResult<Vec<RollDetail>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, branch_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at
         FROM timeline_entries WHERE target_entry_id = ?1 AND kind = ?2 ORDER BY seq ASC",
    )?;
    let entries = stmt
        .query_map(
            rusqlite::params![entry_id, kind::DICEROLL],
            timeline::row_to_entry,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    entries
        .into_iter()
        .filter_map(|entry| {
            let branch_id = entry.branch_id.clone();
            parse_roll(&entry).map(|roll| (branch_id, roll))
        })
        .map(|(branch_id, roll)| detail(&conn, &branch_id, roll, true))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::normalize_reasoning_effort;

    #[test]
    fn reasoning_effort_accepts_only_provider_levels() {
        assert_eq!(normalize_reasoning_effort("none"), Some("none"));
        assert_eq!(normalize_reasoning_effort("MINIMAL"), Some("minimal"));
        assert_eq!(normalize_reasoning_effort(" high "), Some("high"));
        assert_eq!(normalize_reasoning_effort("xhigh"), Some("xhigh"));
        assert_eq!(normalize_reasoning_effort("max"), Some("max"));
        // Anything else is dropped rather than forwarded, so a hand-edited or
        // stale setting can't make every narration request fail.
        assert_eq!(normalize_reasoning_effort(""), None);
        assert_eq!(normalize_reasoning_effort("off"), None);
        assert_eq!(normalize_reasoning_effort("medium "), Some("medium"));
        assert_eq!(normalize_reasoning_effort("turbo"), None);
    }
}
