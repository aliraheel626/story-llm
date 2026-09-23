use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::features::ledger::{model::kind, repository as ledger};
use crate::shared::db::{with_transaction, Pool};
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

fn story_settings(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Value> {
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
    read_story_diceroll_settings(pool.inner(), &story_id)
}

pub(crate) fn read_story_diceroll_settings(
    pool: &Pool,
    story_id: &str,
) -> AppResult<DicerollSettings> {
    let conn = pool.get()?;
    let settings = story_settings(&conn, story_id)?;
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
            .unwrap_or(false),
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
    dice_mode: String,
    attributes_enabled: bool,
    reasoning_effort: Option<String>,
) -> AppResult<()> {
    let dice_mode = DiceMode::from_str_or_default(&dice_mode).as_str();
    // Unrecognized levels are rejected rather than stored, so the story never
    // carries an effort the provider would refuse.
    let reasoning_effort = reasoning_effort
        .as_deref()
        .and_then(normalize_reasoning_effort)
        .map(str::to_string);
    with_transaction(pool.inner(), |tx| {
        let mut settings = story_settings(tx, &story_id)?;
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
        tx.execute(
            "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
        )?;
        ledger::append_entry(tx, &story_id, kind::DICEROLL_SETTINGS_CHANGED, "hidden",
            Some(&format!("Dice-roll settings changed: dice mode {dice_mode}, attributes enabled {attributes_enabled}, reasoning effort {}.",
                reasoning_effort.as_deref().unwrap_or("model default"))),
            &json!({"dice_mode": dice_mode, "attributes_enabled": attributes_enabled, "reasoning_effort": reasoning_effort}), None)?;
        Ok(())
    })
}

fn parse_roll(entry: &crate::features::ledger::model::LedgerEntry) -> Option<Roll> {
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

fn entity_name(conn: &rusqlite::Connection, story_id: &str, id: &str) -> String {
    conn.query_row(
        "SELECT name FROM story_entity_state WHERE story_id = ?1 AND entity_id = ?2",
        rusqlite::params![story_id, id],
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
    story_id: &str,
    roll: Roll,
    snapshots: bool,
) -> AppResult<RollDetail> {
    let actor_name = entity_name(conn, story_id, &roll.actor_entity_id);
    let target_name = roll
        .target_entity_id
        .as_deref()
        .map(|id| entity_name(conn, story_id, id));
    let actor_attribute_name = attribute_name(conn, roll.actor_attribute_id.as_deref());
    let target_attribute_name = attribute_name(conn, roll.target_attribute_id.as_deref());
    let actor_attributes = if snapshots {
        list_entity_attributes_sync(conn, story_id, &roll.actor_entity_id)?
    } else {
        Vec::new()
    };
    let target_attributes = if snapshots {
        roll.target_entity_id
            .as_deref()
            .map(|id| list_entity_attributes_sync(conn, story_id, id))
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
pub fn list_rolls_for_story(pool: State<Pool>, story_id: String) -> AppResult<Vec<RollDetail>> {
    let conn = pool.get()?;
    ledger::list_logical_entries(&conn, &story_id)?
        .iter()
        .filter(|e| e.kind == kind::DICEROLL)
        .filter_map(parse_roll)
        .map(|roll| detail(&conn, &story_id, roll, false))
        .collect()
}

#[tauri::command]
pub fn list_roll_details_for_entry(
    pool: State<Pool>,
    story_id: String,
    entry_id: String,
) -> AppResult<Vec<RollDetail>> {
    let conn = pool.get()?;
    list_roll_details_for_entry_in_conn(&conn, &story_id, &entry_id)
}

fn list_roll_details_for_entry_in_conn(
    conn: &rusqlite::Connection,
    story_id: &str,
    entry_id: &str,
) -> AppResult<Vec<RollDetail>> {
    let base = ledger::get_entry(conn, entry_id)?;
    if base.story_id != story_id {
        return Err(AppError::NotFound(format!(
            "ledger entry {entry_id} not found in story {story_id}"
        )));
    }
    let mut stmt = conn.prepare(
        "SELECT id, story_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at
         FROM ledger_entries WHERE target_entry_id = ?1 AND story_id = ?2 AND kind = ?3 ORDER BY seq ASC",
    )?;
    let entries = stmt
        .query_map(
            rusqlite::params![entry_id, story_id, kind::DICEROLL],
            ledger::row_to_entry,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    entries
        .into_iter()
        .filter_map(|entry| parse_roll(&entry))
        .map(|roll| detail(conn, story_id, roll, true))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        list_roll_details_for_entry_in_conn, normalize_reasoning_effort,
        read_story_diceroll_settings,
    };
    use crate::features::ledger::{model::kind, repository as ledger};
    use crate::shared::error::AppError;
    use serde_json::json;

    fn setup_story() -> crate::shared::db::Pool {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories
             (id, title, created_at, updated_at, settings_json)
             VALUES ('story', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        drop(conn);
        pool
    }

    fn append_test_roll(
        conn: &rusqlite::Connection,
        story_id: &str,
        target_entry_id: &str,
        marker: i64,
    ) -> crate::features::ledger::model::LedgerEntry {
        ledger::append_entry(
            conn,
            story_id,
            kind::DICEROLL,
            "hidden",
            Some("test roll"),
            &json!({
                "actor_entity_id": "actor",
                "p_success": 0.5,
                "seed": marker,
                "roll": marker,
                "outcome": "success",
                "degree": "marginal"
            }),
            Some(target_entry_id),
        )
        .unwrap()
    }

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

    #[test]
    fn fresh_stories_default_dice_rolls_to_disabled() {
        let pool = setup_story();
        let settings = read_story_diceroll_settings(&pool, "story").unwrap();
        assert!(!settings.attributes_enabled);
    }

    #[test]
    fn roll_details_reject_unknown_entry_id() {
        let pool = setup_story();
        let conn = pool.get().unwrap();
        let error = list_roll_details_for_entry_in_conn(&conn, "story", "missing").unwrap_err();
        assert!(matches!(
            error,
            AppError::NotFound(message) if message == "ledger entry missing not found"
        ));
    }

    #[test]
    fn roll_details_are_scoped_to_the_story() {
        let pool = setup_story();
        let conn = pool.get().unwrap();
        let anchor = ledger::append_entry(
            &conn,
            "story",
            kind::NARRATION,
            "visible",
            Some("anchor"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let kept = append_test_roll(&conn, "story", &anchor.id, 60);

        let details = list_roll_details_for_entry_in_conn(&conn, "story", &anchor.id).unwrap();
        let ids = details
            .iter()
            .map(|detail| detail.roll.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![kept.id.as_str()]);
    }
}
