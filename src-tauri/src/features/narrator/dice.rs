//! Dice rules, narrator tool adapter, and staged roll records.

use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use crate::features::entities::model::AttributeRegistryEntry;
use crate::features::ledger;
use crate::prompts;
use crate::shared::error::{AppError, AppResult};

use super::staging::{AttributeReading, StagedRecord, TurnStaging};

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

impl From<AttributeReading> for RollFactor {
    fn from(reading: AttributeReading) -> Self {
        Self {
            entity_id: reading.entity_id,
            entity_name: reading.entity_name,
            attribute_id: reading.attribute_id,
            attribute_name: reading.attribute_name,
            value: reading.value,
            min: reading.min,
            max: reading.max,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RollOutcome {
    pub chance_percent: u8,
    pub seed: i64,
    pub roll: i64,
    pub needed: i64,
    pub outcome: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct PendingRoll {
    pub output: RollOutcome,
    pub reason: Option<String>,
    pub chance_source: &'static str,
    pub factors: Vec<RollFactor>,
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

impl StagedRecord for PendingRoll {
    fn finalize(
        &self,
        remap: &dyn Fn(&str) -> AppResult<Option<AttributeRegistryEntry>>,
    ) -> AppResult<(&'static str, String, serde_json::Value)> {
        let mut pending = self.clone();
        for factor in &mut pending.factors {
            if let Some(canonical) = remap(&factor.attribute_id)? {
                if factor.min != canonical.min || factor.max != canonical.max {
                    return Err(AppError::Invalid(format!(
                        "{}'s attribute range changed while rolling; retry this turn",
                        factor.entity_name
                    )));
                }
                factor.attribute_id = canonical.id;
                factor.attribute_name = canonical.canonical_name;
            }
        }
        let content = format!(
            "Dice-roll outcome: rolled {} with {}% chance and got {}.",
            pending.output.roll, pending.output.chance_percent, pending.output.outcome
        );
        let payload = serde_json::to_value(RollPayload {
            chance_percent: pending.output.chance_percent,
            roll: pending.output.roll,
            needed: pending.output.needed,
            outcome: pending.output.outcome,
            reason: pending.reason,
            chance_source: Some(pending.chance_source),
            factors: pending.factors,
            seed: pending.output.seed,
        })
        .map_err(|error| AppError::Other(error.to_string()))?;
        Ok((ledger::model::kind::DICEROLL, content, payload))
    }
}

pub(super) fn roll_check_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::ROLL_CHECK_TOOL_NAME,
        prompts::ROLL_CHECK_DESCRIPTION,
        prompts::roll_check_schema(),
        move |args: serde_json::Value| {
            let staging = staging.clone();
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
                let mut staging = staging.lock().await;
                let mut factors = Vec::with_capacity(factor_args.len());
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
                    factors.push(RollFactor::from(
                        staging
                            .attribute_reading(entity_id, attribute_name)
                            .map_err(|error| ToolExecutionError::invalid_args(error.to_string()))?,
                    ));
                }
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
                    if explicit_chance.is_some() {
                        return Err(ToolExecutionError::invalid_args(
                            "chance_percent cannot be supplied with attribute factors",
                        ));
                    }
                    (chance_from_factors(&factors), "attributes")
                };
                let output = resolve_roll(chance_percent);
                staging.stage_record(PendingRoll {
                    output,
                    reason: reason.clone(),
                    chance_source,
                    factors: factors.clone(),
                });

                Ok(ToolOutput::json(json!({
                    "chance_percent": chance_percent, "roll": output.roll,
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
mod tests {
    use super::super::staging::PendingOp;
    use super::*;
    use crate::features::entities::{self, attributes, model::AttributeRegistryEntry};
    use crate::features::ledger::repository::append_entry;
    use crate::shared::db::Pool;
    use chrono::Utc;
    use uuid::Uuid;

    fn setup() -> (Pool, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let story_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES (?1, 't', ?2, ?2, '{}')",
            rusqlite::params![story_id, now],
        )
        .unwrap();
        (pool, story_id)
    }

    fn find_attribute(conn: &rusqlite::Connection, name: &str) -> AttributeRegistryEntry {
        conn.query_row(
            "SELECT id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at
             FROM attribute_registry WHERE canonical_name = ?1",
            [name],
            |row| {
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
            },
        )
        .unwrap()
    }

    #[test]
    fn export_bindings() {
        use ts_rs::{Config, TS};

        RollPayload::export_all(&Config::new()).expect("failed to export roll bindings");
    }

    #[test]
    fn roll_payload_serializes_with_exact_current_shape() {
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
                "chance_percent": 60,
                "roll": 72,
                "needed": 40,
                "outcome": "success",
                "reason": "Sneak past the guard",
                "chance_source": "attributes",
                "factors": [{
                    "entity_id": "player",
                    "entity_name": "You",
                    "attribute_id": "stealth",
                    "attribute_name": "Stealth",
                    "value": 8.0,
                    "min": 0.0,
                    "max": 10.0,
                }],
                "seed": 42,
            })
        );
        assert_eq!(payload.as_object().unwrap().len(), 8);
    }

    fn persist_staging(pool: &Pool, story_id: &str, staging: &TurnStaging) {
        let mut conn = pool.get().unwrap();
        let turn_id = crate::features::ledger::turns::create_turn(&conn, story_id).unwrap();
        let passage = append_entry(
            &conn,
            story_id,
            "narration",
            "visible",
            Some("scene"),
            &json!({}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        staging.commit(&tx, &passage.id, &turn_id).unwrap();
        crate::features::ledger::turns::set_status(
            &tx,
            &turn_id,
            crate::features::ledger::turns::COMPLETE,
        )
        .unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn friendly_label_describes_rolls_and_degrades_safely() {
        assert_eq!(
            super::super::tools::friendly_tool_label("roll_check", r#"{"reason":"escaping"}"#,),
            "Rolling for escaping…"
        );
        assert_eq!(
            super::super::tools::friendly_tool_label("roll_check", "{}"),
            "Rolling for a check…"
        );
        assert_eq!(
            super::super::tools::friendly_tool_label("roll_check", "not json"),
            "Rolling for a check…"
        );
    }

    #[test]
    fn chance_roll_has_exact_zero_and_hundred_percent_bounds() {
        for _ in 0..100 {
            let impossible = resolve_roll(0);
            assert!((0..100).contains(&impossible.roll));
            assert_eq!(impossible.needed, 100);
            assert_eq!(impossible.outcome, "failure");
            let certain = resolve_roll(100);
            assert!((0..100).contains(&certain.roll));
            assert_eq!(certain.needed, 0);
            assert_eq!(certain.outcome, "success");
        }
    }

    #[test]
    fn chance_roll_seed_replays_the_draw() {
        for chance in [1, 25, 50, 75, 99] {
            let result = resolve_roll(chance);
            let mut rng = StdRng::seed_from_u64(result.seed as u64);
            assert_eq!(rng.gen_range(0..100), result.roll);
            assert_eq!(result.needed, 100 - i64::from(chance));
            assert_eq!(result.outcome == "success", result.roll >= result.needed);
        }
    }

    #[tokio::test]
    async fn roll_check_stages_only_chance_and_reason_without_entity_side_effects() {
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let out = roll_check_tool(staging.clone())
            .execute(json!({"chance_percent": 35, "reason": " escaping a ghoul "}))
            .await
            .unwrap();
        let out = out.as_json().unwrap();
        assert_eq!(out["chance_percent"], json!(35));
        assert_eq!(out["chance_source"], json!("narrator"));
        assert_eq!(out["factors"], json!([]));
        assert_eq!(out["needed"], json!(65));
        assert_eq!(out["reason"], json!("escaping a ghoul"));
        assert_eq!(
            out["outcome"] == json!("success"),
            out["roll"].as_i64().unwrap() >= 65
        );

        let staging = staging.lock().await;
        assert!(matches!(staging.pending.as_slice(), [PendingOp::Record(_)]));
        assert!(staging.effective_entities(None, None).unwrap().is_empty());
        let count: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger::model::kind::DICEROLL],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        persist_staging(&pool, &story_id, &staging);
        drop(staging);

        let payload_json: String = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT payload_json FROM ledger_entries WHERE kind = ?1",
                [ledger::model::kind::DICEROLL],
                |row| row.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
        assert_eq!(payload.as_object().unwrap().len(), 8);
        assert_eq!(payload["chance_percent"], json!(35));
        assert_eq!(payload["roll"], out["roll"]);
        assert_eq!(payload["needed"], out["needed"]);
        assert_eq!(payload["outcome"], out["outcome"]);
        assert_eq!(payload["reason"], out["reason"]);
        assert_eq!(payload["chance_source"], out["chance_source"]);
        assert_eq!(payload["factors"], out["factors"]);
        assert_eq!(payload["seed"], out["seed"]);
    }

    #[tokio::test]
    async fn roll_check_rejects_invalid_chances_without_staging() {
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id)));
        let roll = roll_check_tool(staging.clone());
        for args in [
            json!({"chance_percent": -1}),
            json!({"chance_percent": 101}),
            json!({"chance_percent": 50.5}),
            json!({"chance_percent": "50"}),
            json!({"chance_percent": 50, "reason": 7}),
            json!({"value": 8}),
            json!({"factors": "You"}),
            json!({"factors": [1, 2, 3]}),
            json!({"factors": [{"entity_id": "missing", "attribute_name": "Stealth", "value": 10}]}),
        ] {
            assert!(roll.execute(args.clone()).await.is_err(), "{args}");
        }
        assert!(staging.lock().await.pending.is_empty());
        let default = roll.execute(json!({})).await.unwrap();
        assert_eq!(default.as_json().unwrap()["chance_percent"], json!(50));
        assert_eq!(
            default.as_json().unwrap()["chance_source"],
            json!("default")
        );
        for chance in [0, 100] {
            let out = roll
                .execute(json!({"chance_percent": chance}))
                .await
                .unwrap();
            assert_eq!(out.as_json().unwrap()["chance_percent"], json!(chance));
        }
    }

    #[tokio::test]
    async fn roll_check_fetches_one_or_two_authoritative_attribute_values() {
        let (pool, story_id) = setup();
        let conn = pool.get().unwrap();
        entities::create_entity_with_id_sync(
            &conn,
            "actor",
            &story_id,
            "character",
            "You",
            None,
            "test",
            None,
            None,
        )
        .unwrap();
        entities::create_entity_with_id_sync(
            &conn,
            "guard",
            &story_id,
            "character",
            "Guard",
            None,
            "test",
            None,
            None,
        )
        .unwrap();
        let stealth = find_attribute(&conn, "Stealth");
        let perception = find_attribute(&conn, "Perception");
        attributes::set_entity_attribute_sync(&conn, &story_id, "actor", &stealth.id, 8.0).unwrap();
        attributes::set_entity_attribute_sync(&conn, &story_id, "guard", &perception.id, 6.0)
            .unwrap();
        drop(conn);

        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let roll = roll_check_tool(staging.clone());
        let actor = json!({"entity_id":"actor","attribute_name":"Stealth"});
        let guard = json!({"entity_id":"guard","attribute_name":"Perception"});
        let one = roll.execute(json!({"factors":[actor]})).await.unwrap();
        let one = one.as_json().unwrap();
        assert_eq!(one["chance_percent"], json!(65));
        assert_eq!(one["chance_source"], json!("attributes"));
        assert_eq!(one["factors"][0]["entity_name"], json!("You"));
        assert_eq!(one["factors"][0]["attribute_id"], json!(stealth.id));
        assert_eq!(one["factors"][0]["value"], json!(8.0));

        let two = roll
            .execute(json!({"factors":[actor, guard],"reason":"slip past the guard"}))
            .await
            .unwrap();
        let two = two.as_json().unwrap();
        assert_eq!(two["chance_percent"], json!(60));
        assert_eq!(two["factors"][1]["entity_name"], json!("Guard"));
        assert_eq!(two["factors"][1]["attribute_name"], json!("Perception"));
        assert_eq!(two["factors"][1]["value"], json!(6.0));
        assert!(roll
            .execute(json!({"chance_percent":60,"factors":[actor]}))
            .await
            .is_err());
        assert!(roll
            .execute(json!({"factors":[{"entity_id":"guard","attribute_name":"Stealth"}]}))
            .await
            .is_err());
        assert!(roll
            .execute(json!({"factors":[{"entity_id":"other-story","attribute_name":"Stealth"}]}))
            .await
            .is_err());
        assert_eq!(staging.lock().await.pending.len(), 2);

        let staging = staging.lock().await;
        persist_staging(&pool, &story_id, &staging);
        drop(staging);
        let conn = pool.get().unwrap();
        let payload: String = conn
            .query_row(
                "SELECT payload_json FROM ledger_entries WHERE kind = ?1 ORDER BY seq DESC LIMIT 1",
                [ledger::model::kind::DICEROLL],
                |row| row.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["factors"][0]["value"], json!(8.0));
        assert_eq!(payload["factors"][1]["value"], json!(6.0));
        attributes::set_entity_attribute_sync(&conn, &story_id, "actor", &stealth.id, 2.0).unwrap();
        assert_eq!(payload["factors"][0]["value"], json!(8.0));
    }

    #[tokio::test]
    async fn roll_check_can_use_attribute_adjusted_earlier_in_the_same_turn() {
        let (pool, story_id) = setup();
        let conn = pool.get().unwrap();
        let stealth = find_attribute(&conn, "Stealth");
        drop(conn);
        let mut staging = TurnStaging::new(pool, story_id);
        let (player, _) = staging
            .resolve_or_stage_entity("character", "You", None)
            .unwrap();
        staging.pending.push(PendingOp::AdjustAttribute {
            entity_id: player.id.clone(),
            attribute: stealth,
            delta: 3.0,
            cause: "careful practice".into(),
            dramatic: false,
        });
        let result = roll_check_tool(Arc::new(Mutex::new(staging)))
            .execute(json!({"factors":[{"entity_id":player.id,"attribute_name":"Stealth"}]}))
            .await
            .unwrap();
        assert_eq!(result.as_json().unwrap()["chance_percent"], json!(65));
        assert_eq!(result.as_json().unwrap()["factors"][0]["value"], json!(8.0));
    }

    #[tokio::test]
    async fn roll_check_resolves_committed_and_staged_attribute_aliases() {
        let (pool, story_id) = setup();
        let conn = pool.get().unwrap();
        entities::create_entity_with_id_sync(
            &conn,
            "actor",
            &story_id,
            "character",
            "You",
            None,
            "test",
            None,
            None,
        )
        .unwrap();
        let stealth = find_attribute(&conn, "Stealth");
        attributes::set_entity_attribute_sync(&conn, &story_id, "actor", &stealth.id, 7.0).unwrap();
        attributes::add_alias(&conn, &stealth.id, "Sneaking").unwrap();
        drop(conn);

        let mut staging = TurnStaging::new(pool, story_id);
        staging.pending.push(PendingOp::AddAlias {
            attribute: stealth.clone(),
            alias: "Quiet Steps".into(),
        });
        let roll = roll_check_tool(Arc::new(Mutex::new(staging)));
        for alias in ["Sneaking", "Quiet Steps"] {
            let out = roll
                .execute(json!({"factors":[{"entity_id":"actor","attribute_name":alias}]}))
                .await
                .unwrap();
            assert_eq!(
                out.as_json().unwrap()["factors"][0]["attribute_id"],
                json!(stealth.id)
            );
            assert_eq!(
                out.as_json().unwrap()["factors"][0]["attribute_name"],
                json!("Stealth")
            );
            assert_eq!(out.as_json().unwrap()["factors"][0]["value"], json!(7.0));
        }
    }

    #[tokio::test]
    async fn independently_staged_mints_commit_to_one_canonical_attribute() {
        let (pool, story_id) = setup();
        let first = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let second = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let mut staged = Vec::new();
        for (turn, entity_name, name) in [
            (&first, "First Prism", "Resonance"),
            (&second, "Second Prism", "resonance"),
        ] {
            let entity = turn
                .lock()
                .await
                .resolve_or_stage_entity("artifact", entity_name, None)
                .unwrap()
                .0;
            let attribute =
                super::super::staging::resolve_or_stage_attribute(turn, "", name, "artifact")
                    .await
                    .unwrap();
            turn.lock().await.pending.push(PendingOp::AdjustAttribute {
                entity_id: entity.id.clone(),
                attribute: attribute.clone(),
                delta: 1.0,
                cause: "test".into(),
                dramatic: false,
            });
            staged.push((entity, attribute));
        }
        assert_ne!(staged[0].1.id, staged[1].1.id);
        let result = roll_check_tool(second.clone())
            .execute(json!({"factors":[{"entity_id":staged[1].0.id,"attribute_name":"resonance"}]}))
            .await
            .unwrap();
        assert_eq!(
            result.as_json().unwrap()["factors"][0]["attribute_id"],
            json!(staged[1].1.id)
        );
        assert_eq!(pool.get().unwrap().query_row(
            "SELECT COUNT(*) FROM attribute_registry WHERE lower(canonical_name) = 'resonance'",
            [], |row| row.get::<_, i64>(0),
        ).unwrap(), 0);
        {
            let staging = first.lock().await;
            persist_staging(&pool, &story_id, &staging);
        }
        {
            let staging = second.lock().await;
            persist_staging(&pool, &story_id, &staging);
        }
        let conn = pool.get().unwrap();
        let (count, distinct_ids): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT attribute_id) FROM entity_attributes WHERE entity_id IN (?1, ?2)",
            rusqlite::params![staged[0].0.id, staged[1].0.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!((count, distinct_ids), (2, 1));
        let roll_payload: String = conn
            .query_row(
                "SELECT payload_json FROM ledger_entries WHERE kind = ?1 ORDER BY seq DESC LIMIT 1",
                [ledger::model::kind::DICEROLL],
                |row| row.get(0),
            )
            .unwrap();
        let roll_payload: serde_json::Value = serde_json::from_str(&roll_payload).unwrap();
        assert_eq!(
            roll_payload["factors"][0]["attribute_id"],
            json!(staged[0].1.id)
        );
        assert_eq!(
            roll_payload["factors"][0]["attribute_name"],
            json!("Resonance")
        );
    }
}
