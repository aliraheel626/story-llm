pub mod cascade;
pub mod model;
pub mod projections;
pub mod reducer;
pub mod repository;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::features::narration::staging::RollFactor;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};
use model::{kind, LedgerSnapshot};

#[tauri::command]
pub fn list_ledger_entries(pool: State<Pool>, story_id: String) -> AppResult<LedgerSnapshot> {
    let conn = pool.get()?;
    let entries = repository::list_logical_entries(&conn, &story_id)?;
    Ok(LedgerSnapshot {
        visible: reducer::active_visible_entries(&entries),
        hidden: entries
            .into_iter()
            .filter(|entry| entry.visibility == "hidden")
            .collect(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roll {
    pub id: String,
    pub entry_id: String,
    pub chance_percent: u8,
    pub roll: i64,
    pub outcome: String,
    pub reason: Option<String>,
    pub chance_source: Option<String>,
    pub factors: Vec<RollFactor>,
    pub seed: i64,
    pub created_at: String,
}

fn parse_roll(entry: &model::LedgerEntry) -> Option<Roll> {
    let p = &entry.payload;
    let chance_percent = u8::try_from(p.get("chance_percent")?.as_u64()?).ok()?;
    let roll = p.get("roll")?.as_i64()?;
    let outcome = p.get("outcome")?.as_str()?;
    let chance_source = p.get("chance_source").and_then(Value::as_str);
    let factors: Vec<RollFactor> = p
        .get("factors")
        .map(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_else(|| Some(Vec::new()))?;
    if chance_percent > 100
        || !(0..100).contains(&roll)
        || !matches!(outcome, "success" | "failure")
        || (outcome == "success") != (roll >= 100 - i64::from(chance_percent))
        || factors.len() > 2
        || factors.iter().any(|factor| {
            !factor.value.is_finite()
                || !factor.min.is_finite()
                || !factor.max.is_finite()
                || factor.min >= factor.max
                || factor.value < factor.min
                || factor.value > factor.max
        })
        || chance_source.is_some_and(|source| match source {
            "default" => !(factors.is_empty() && chance_percent == 50),
            "narrator" => !factors.is_empty(),
            "attributes" => factors.is_empty(),
            _ => true,
        })
    {
        return None;
    }
    Some(Roll {
        id: entry.id.clone(),
        entry_id: entry.target_entry_id.clone()?,
        chance_percent,
        roll,
        outcome: outcome.into(),
        reason: p.get("reason").and_then(Value::as_str).map(str::to_string),
        chance_source: chance_source.map(str::to_string),
        factors,
        seed: p.get("seed")?.as_i64()?,
        created_at: entry.created_at.clone(),
    })
}

#[tauri::command]
pub fn list_rolls_for_story(pool: State<Pool>, story_id: String) -> AppResult<Vec<Roll>> {
    let conn = pool.get()?;
    Ok(repository::list_logical_entries(&conn, &story_id)?
        .iter()
        .filter(|entry| entry.kind == kind::DICEROLL)
        .filter_map(parse_roll)
        .collect())
}

#[tauri::command]
pub fn list_roll_details_for_entry(
    pool: State<Pool>,
    story_id: String,
    entry_id: String,
) -> AppResult<Vec<Roll>> {
    let conn = pool.get()?;
    list_roll_details_for_entry_in_conn(&conn, &story_id, &entry_id)
}

fn list_roll_details_for_entry_in_conn(
    conn: &rusqlite::Connection,
    story_id: &str,
    entry_id: &str,
) -> AppResult<Vec<Roll>> {
    let base = repository::get_entry(conn, entry_id)?;
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
            repository::row_to_entry,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries.iter().filter_map(parse_roll).collect())
}

#[cfg(test)]
mod roll_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roll_details_are_scoped_and_use_only_chance_payload() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('story', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let anchor = repository::append_entry(
            &conn,
            "story",
            kind::NARRATION,
            "visible",
            Some("anchor"),
            &json!({}),
            None,
        )
        .unwrap();
        let kept = repository::append_entry(
            &conn,
            "story",
            kind::DICEROLL,
            "hidden",
            Some("roll"),
            &json!({"chance_percent":40,"roll":60,"outcome":"success","reason":"a door","seed":42}),
            Some(&anchor.id),
        )
        .unwrap();
        repository::append_entry(
            &conn, "story", kind::DICEROLL, "hidden", Some("old roll"),
            &json!({"actor_entity_id":"actor","p_success":0.5,"roll":60,"outcome":"success","seed":42}),
            Some(&anchor.id),
        ).unwrap();
        let rolls = list_roll_details_for_entry_in_conn(&conn, "story", &anchor.id).unwrap();
        assert_eq!(rolls.len(), 1);
        assert_eq!(rolls[0].id, kept.id);
        assert_eq!(rolls[0].reason.as_deref(), Some("a door"));
        assert_eq!(rolls[0].chance_source, None);
        assert!(rolls[0].factors.is_empty());
        assert!(matches!(
            list_roll_details_for_entry_in_conn(&conn, "other", &anchor.id),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn roll_reader_preserves_attribute_snapshots() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES ('s','Story','now','now','{}')", []).unwrap();
        let anchor = repository::append_entry(
            &conn,
            "s",
            kind::NARRATION,
            "visible",
            Some("scene"),
            &json!({}),
            None,
        )
        .unwrap();
        repository::append_entry(&conn, "s", kind::DICEROLL, "hidden", Some("roll"),
            &json!({"chance_percent":60,"roll":70,"outcome":"success","reason":"sneak","chance_source":"attributes","seed":42,
                "factors":[{"entity_id":"player","entity_name":"You","attribute_id":"stealth","attribute_name":"Stealth","value":8.0,"min":0.0,"max":10.0},
                           {"entity_id":"guard","entity_name":"Guard","attribute_id":"perception","attribute_name":"Perception","value":6.0,"min":0.0,"max":10.0}]}), Some(&anchor.id)).unwrap();
        let rolls = list_roll_details_for_entry_in_conn(&conn, "s", &anchor.id).unwrap();
        assert_eq!(rolls.len(), 1);
        assert_eq!(rolls[0].chance_source.as_deref(), Some("attributes"));
        assert_eq!(rolls[0].factors.len(), 2);
        assert_eq!(rolls[0].factors[0].entity_name, "You");
        assert_eq!(rolls[0].factors[1].value, 6.0);
    }
}
