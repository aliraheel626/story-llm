//! Dice rules, narrator tool adapter, and turn-local roll records.

use std::sync::Arc;

use crate::features::entities::{self, attributes};
use crate::features::ledger;
use crate::features::ledger::turn_tx::TurnTx;
use crate::prompts;
use crate::shared::error::{AppError, AppResult};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RollFactor {
    pub entity_id: String,
    pub entity_name: String,
    pub attribute_id: String,
    pub attribute_name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export_to = "../../src/shared/generated/Roll.ts"))]
pub struct RollPayload {
    pub chance_percent: u8,
    #[cfg_attr(test, ts(type = "number"))]
    pub roll: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub needed: i64,
    pub outcome: &'static str,
    pub reason: Option<String>,
    #[cfg_attr(test, ts(type = "\"default\" | \"narrator\" | \"attributes\" | null"))]
    pub chance_source: Option<&'static str>,
    #[cfg_attr(test, ts(inline))]
    pub factors: Vec<RollFactor>,
    #[cfg_attr(test, ts(type = "number"))]
    pub seed: i64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RollOutcome {
    pub chance_percent: u8,
    pub seed: i64,
    pub roll: i64,
    pub needed: i64,
    pub outcome: &'static str,
}

pub(super) fn chance_from_factors(factors: &[RollFactor]) -> u8 {
    let normalized = |factor: &RollFactor| (factor.value - factor.min) / (factor.max - factor.min);
    let actor = normalized(&factors[0]);
    let opponent = factors.get(1).map(normalized).unwrap_or(0.5);
    (50.0 + 50.0 * (actor - opponent)).round().clamp(0.0, 100.0) as u8
}

pub(super) fn resolve_roll(chance_percent: u8) -> RollOutcome {
    assert!(chance_percent <= 100, "chance must be between 0 and 100");
    let seed = rand::random::<u32>() as i64;
    let mut rng = StdRng::seed_from_u64(seed as u64);
    let roll = rng.gen_range(0..100);
    let needed = 100 - i64::from(chance_percent);
    RollOutcome {
        chance_percent,
        seed,
        roll,
        needed,
        outcome: if roll >= needed { "success" } else { "failure" },
    }
}

fn factor_reading(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    attribute_name: &str,
) -> AppResult<RollFactor> {
    let entity = entities::list_entities_sync(conn, story_id, None)?
        .into_iter()
        .find(|entity| entity.id == entity_id)
        .ok_or_else(|| AppError::NotFound(format!("entity {entity_id} not found in this story")))?;
    let registry_match = attributes::find_exact_match(conn, attribute_name)?;
    let values = attributes::list_entity_attributes_sync(conn, story_id, entity_id)?;
    let value = values
        .into_iter()
        .find(|value| {
            registry_match
                .as_ref()
                .is_some_and(|entry| entry.id == value.attribute_id)
                || (registry_match.is_none()
                    && value.canonical_name.eq_ignore_ascii_case(attribute_name))
        })
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "attribute {attribute_name} is not set on {}",
                entity.name
            ))
        })?;
    if !value.value.is_finite()
        || !value.min.is_finite()
        || !value.max.is_finite()
        || value.min >= value.max
        || value.value < value.min
        || value.value > value.max
    {
        return Err(AppError::Invalid(format!(
            "invalid value or range for {}'s {}",
            entity.name, value.canonical_name
        )));
    }
    Ok(RollFactor {
        entity_id: entity.id,
        entity_name: entity.name,
        attribute_id: value.attribute_id,
        attribute_name: value.canonical_name,
        value: value.value,
        min: value.min,
        max: value.max,
    })
}

pub(super) fn roll_check_tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::ROLL_CHECK_TOOL_NAME,
        prompts::ROLL_CHECK_DESCRIPTION,
        prompts::roll_check_schema(),
        move |args: serde_json::Value| {
            let turn = turn.clone();
            let target_entry_id = target_entry_id.clone();
            let turn_id = turn_id.clone();
            Box::pin(async move {
                let fields = args.as_object().ok_or_else(|| {
                    ToolExecutionError::invalid_args("roll_check expects an object")
                })?;
                if fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "chance_percent" | "reason" | "factors"))
                {
                    return Err(ToolExecutionError::invalid_args(
                        "roll_check accepts only chance_percent, reason, and factors",
                    ));
                }
                let explicit_chance = match args.get("chance_percent") {
                    None => None,
                    Some(value) => Some(value.as_u64().filter(|&n| n <= 100).ok_or_else(|| {
                        ToolExecutionError::invalid_args(
                            "chance_percent must be an integer from 0 to 100",
                        )
                    })? as u8),
                };
                let reason = match args.get("reason") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s)) => {
                        Some(s.trim().to_string()).filter(|s| !s.is_empty())
                    }
                    _ => return Err(ToolExecutionError::invalid_args("reason must be a string")),
                };
                let factor_args: &[serde_json::Value] = match args.get("factors") {
                    None => &[],
                    Some(serde_json::Value::Array(factors)) if factors.len() <= 2 => factors,
                    _ => {
                        return Err(ToolExecutionError::invalid_args(
                            "factors must be an array of at most two entity-attribute references",
                        ));
                    }
                };
                let mut references = Vec::with_capacity(factor_args.len());
                for factor in factor_args {
                    let fields = factor.as_object().ok_or_else(|| {
                        ToolExecutionError::invalid_args("each factor must be an object")
                    })?;
                    if fields.len() != 2 {
                        return Err(ToolExecutionError::invalid_args(
                            "each factor requires only entity_id and attribute_name",
                        ));
                    }
                    let entity_id = fields
                        .get("entity_id")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("factor entity_id is required")
                        })?;
                    let attribute_name = fields
                        .get("attribute_name")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("factor attribute_name is required")
                        })?;
                    references.push((entity_id, attribute_name));
                }
                if !references.is_empty() && explicit_chance.is_some() {
                    return Err(ToolExecutionError::invalid_args(
                        "chance_percent cannot be supplied with attribute factors",
                    ));
                }
                let (output, chance_source, factors) = turn
                    .with(|conn| {
                        let factors = references
                            .iter()
                            .map(|(id, name)| factor_reading(conn, turn.story_id(), id, name))
                            .collect::<AppResult<Vec<_>>>()?;
                        let (chance_percent, chance_source) = if factors.is_empty() {
                            (
                                explicit_chance.unwrap_or(50),
                                if explicit_chance.is_some() {
                                    "narrator"
                                } else {
                                    "default"
                                },
                            )
                        } else {
                            (chance_from_factors(&factors), "attributes")
                        };
                        let output = resolve_roll(chance_percent);
                        let content = format!(
                            "Dice-roll outcome: rolled {} with {}% chance and got {}.",
                            output.roll, output.chance_percent, output.outcome,
                        );
                        let payload = serde_json::to_value(RollPayload {
                            chance_percent,
                            roll: output.roll,
                            needed: output.needed,
                            outcome: output.outcome,
                            reason: reason.clone(),
                            chance_source: Some(chance_source),
                            factors: factors.clone(),
                            seed: output.seed,
                        })
                        .map_err(|error| AppError::Other(error.to_string()))?;
                        ledger::repository::append_entry(
                            conn,
                            turn.story_id(),
                            ledger::model::kind::DICEROLL,
                            "hidden",
                            Some(&content),
                            &payload,
                            Some(&target_entry_id),
                            Some(&turn_id),
                        )?;
                        Ok((output, chance_source, factors))
                    })
                    .await
                    .map_err(|error| match error {
                        AppError::NotFound(_) | AppError::Invalid(_) => {
                            ToolExecutionError::invalid_args(error.to_string())
                        }
                        _ => ToolExecutionError::other(error.to_string()),
                    })?;

                Ok(ToolOutput::json(json!({
                    "chance_percent": output.chance_percent, "roll": output.roll,
                    "needed": output.needed, "outcome": output.outcome,
                    "reason": reason, "seed": output.seed,
                    "chance_source": chance_source, "factors": factors,
                })))
            })
        },
    )
}

pub(super) fn roll_check_label(args: &serde_json::Value) -> String {
    format!(
        "Rolling for {}…",
        args.get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "a check".into())
    )
}

#[cfg(test)]
mod turn_tests {
    use super::*;
    use crate::features::ledger::{
        repository::append_entry,
        turn_tx::{TurnGate, TurnTx},
    };
    use crate::shared::db::Pool;

    fn fixture() -> (Pool, Arc<TurnTx>, String, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let story_id = uuid::Uuid::new_v4().to_string();
        conn.execute("INSERT INTO stories(id, title, created_at, updated_at, settings_json) VALUES (?1, 't', 'now', 'now', '{}')", [&story_id]).unwrap();
        let turn_id = crate::features::ledger::turns::create_turn(&conn, &story_id).unwrap();
        let target = append_entry(
            &conn,
            &story_id,
            ledger::model::kind::NARRATION,
            "visible",
            Some("scene"),
            &json!({}),
            None,
            Some(&turn_id),
        )
        .unwrap()
        .id;
        drop(conn);
        let turn = TurnTx::begin(&pool, &TurnGate::default(), &story_id).unwrap();
        (pool, turn, target, turn_id)
    }

    #[test]
    fn export_bindings() {
        use ts_rs::{Config, TS};
        RollPayload::export_all(&Config::new()).expect("failed to export roll bindings");
    }

    #[test]
    fn payload_serializes_with_exact_current_shape() {
        let payload = serde_json::to_value(RollPayload {
            chance_percent: 60,
            roll: 72,
            needed: 40,
            outcome: "success",
            reason: Some("Sneak past the guard".into()),
            chance_source: Some("attributes"),
            factors: vec![RollFactor {
                entity_id: "player".into(),
                entity_name: "You".into(),
                attribute_id: "stealth".into(),
                attribute_name: "Stealth".into(),
                value: 8.0,
                min: 0.0,
                max: 10.0,
            }],
            seed: 42,
        })
        .unwrap();
        assert_eq!(
            payload,
            json!({
                "chance_percent":60,"roll":72,"needed":40,"outcome":"success",
                "reason":"Sneak past the guard","chance_source":"attributes",
                "factors":[{"entity_id":"player","entity_name":"You","attribute_id":"stealth",
                    "attribute_name":"Stealth","value":8.0,"min":0.0,"max":10.0}],"seed":42,
            })
        );
        assert_eq!(payload.as_object().unwrap().len(), 8);
    }

    #[test]
    fn chance_roll_seed_replays_the_draw() {
        for chance in [0, 1, 25, 50, 75, 99, 100] {
            let result = resolve_roll(chance);
            let mut rng = StdRng::seed_from_u64(result.seed as u64);
            assert_eq!(rng.gen_range(0..100), result.roll);
            assert_eq!(result.outcome == "success", result.roll >= result.needed);
        }
    }

    #[tokio::test]
    async fn roll_is_written_in_turn_with_exact_payload_and_target() {
        let (pool, turn, target, turn_id) = fixture();
        let roll = roll_check_tool(turn.clone(), target.clone(), turn_id.clone());
        for args in [
            json!({"chance_percent":-1}),
            json!({"chance_percent":101}),
            json!({"factors":"not array"}),
            json!({"chance_percent":55.2}),
        ] {
            assert!(roll.execute(args).await.is_err());
        }
        let out = roll
            .execute(json!({"chance_percent":35,"reason":" escaping a ghoul "}))
            .await
            .unwrap();
        let out = out.as_json().unwrap();
        assert_eq!(out["reason"], json!("escaping a ghoul"));
        assert_eq!(out["chance_source"], json!("narrator"));
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM ledger_entries WHERE kind='diceroll'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        turn.with(|conn| {
            let (content, raw, entry, tid): (String, String, String, String) = conn.query_row(
                "SELECT content, payload_json, target_entry_id, turn_id FROM ledger_entries WHERE kind='diceroll'",
                [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            assert_eq!((entry, tid), (target.clone(), turn_id.clone()));
            assert_eq!(content, format!("Dice-roll outcome: rolled {} with 35% chance and got {}.", out["roll"], out["outcome"].as_str().unwrap()));
            let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(payload.as_object().unwrap().len(), 8);
            for key in ["chance_percent", "roll", "needed", "outcome", "reason", "chance_source", "factors", "seed"] {
                assert_eq!(payload[key], out[key]);
            }
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM ledger_entries WHERE kind='diceroll'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn factor_reads_current_turn_attribute_and_alias() {
        let (_pool, turn, target, turn_id) = fixture();
        let entity =
            super::super::tools::create_entity_tool(turn.clone(), target.clone(), turn_id.clone())
                .execute(json!({"kind":"character","name":"You"}))
                .await
                .unwrap();
        let id = entity.as_json().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        super::super::tools::adjust_entity_attribute_tool(
            turn.clone(),
            target.clone(),
            turn_id.clone(),
            String::new(),
        )
        .execute(json!({"entity_id":id,"attribute":"Stealth","delta":3}))
        .await
        .unwrap();
        let roll = roll_check_tool(turn.clone(), target, turn_id);
        let result = roll
            .execute(json!({"factors":[{"entity_id":id,"attribute_name":"Stealth"}]}))
            .await
            .unwrap();
        assert_eq!(result.as_json().unwrap()["chance_percent"], json!(65));
        assert_eq!(result.as_json().unwrap()["factors"][0]["value"], json!(8.0));
        turn.with(|conn| {
            let stealth = attributes::find_exact_match(conn, "Stealth")?.unwrap();
            attributes::add_alias(conn, &stealth.id, "Sneaking")
        })
        .await
        .unwrap();
        let alias = roll
            .execute(json!({"factors":[{"entity_id":id,"attribute_name":"Sneaking"}]}))
            .await
            .unwrap();
        assert_eq!(
            alias.as_json().unwrap()["factors"][0]["attribute_name"],
            json!("Stealth")
        );
        assert!(roll
            .execute(json!({"factors":[{"entity_id":"missing","attribute_name":"Stealth"}]}))
            .await
            .is_err());
        turn.rollback().await.unwrap();
    }
}
