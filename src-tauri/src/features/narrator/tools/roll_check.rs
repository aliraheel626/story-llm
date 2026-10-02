use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities::attributes;
use crate::features::narrator::{
    catalog::{self, ToolAvailability, ToolDeps, ToolSpec},
    dice::{chance_from_factors, resolve_roll, RollFactor, RollPayload},
};
use crate::features::transcript;
use crate::features::turn::TurnTx;
use crate::shared::error::{AppError, AppResult};

use super::shared::{object_args, resolve_entity};

pub const NAME: &str = "roll_check";
pub const DESCRIPTION: &str =
    "Resolve a genuinely uncertain action: call this before narrating the result, and do not \
     roll routine or certain actions. With zero factors, chance_percent is optional and \
     defaults to 50. For one factor, name the acting entity and attribute_name; for two, \
     put the acting pair first and the opposing pair second. The backend reads stored attribute \
     values, normalizes each by its registered min/max, and calculates chance_percent as \
     round(50 + 50 * (actor_normalized - opponent_normalized)); a single factor faces a neutral \
     opponent at 0.5. Do not pass chance_percent with factors, and do not invent attribute \
     values. The tool returns the draw and success or failure.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "chance_percent": {"type": "integer", "minimum": 0, "maximum": 100, "description": "Optional narrator-estimated chance when no factors are given; defaults to 50%. Must be omitted when factors are present."},
            "reason": {"type": "string", "description": "A short description of the uncertain action and why it needs a roll."},
            "factors": {
                "type": "array", "maxItems": 2,
                "description": "Zero, one acting, or two acting-then-opposing registered entity attributes. Values are fetched by the backend; never supply numbers here.",
                "items": {
                    "type": "object",
                    "properties": {
                        "entity": {"type": "string", "description": "A character name, or a relationship as 'Mira → You'. An id is also accepted."},
                        "attribute_name": {"type": "string", "description": "Name of an attribute currently set on the entity."}
                    },
                    "required": ["entity", "attribute_name"],
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

fn label(args: &Value) -> String {
    format!(
        "Rolling for {}…",
        args.get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "a check".into())
    )
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    catalog::normal_mode(availability) && availability.settings.roll_check
}

fn build(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = catalog::turn(deps);
    tool(turn, target, turn_id)
}

pub const SPEC: ToolSpec = ToolSpec {
    name: NAME,
    needs_turn: true,
    enabled,
    build,
    label,
};

fn factor_reading(
    conn: &rusqlite::Connection,
    story_id: &str,
    reference: &str,
    attribute_name: &str,
) -> AppResult<RollFactor> {
    let entity = resolve_entity(conn, story_id, reference)?;
    let registry_match = attributes::find_exact_match(conn, attribute_name)?;
    let values = attributes::list_entity_attributes_sync(conn, story_id, &entity.id)?;
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

pub(super) fn tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        NAME,
        DESCRIPTION,
        schema(),
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
                    None => None,
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
                    let fields = object_args(factor, &["entity", "attribute_name"], "factor")
                        .map_err(super::to_tool_error)?;
                    if fields.len() != 2 {
                        return Err(ToolExecutionError::invalid_args(
                            "each factor requires only entity and attribute_name",
                        ));
                    }
                    let entity = fields
                        .get("entity")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("factor entity is required")
                        })?;
                    let attribute_name = fields
                        .get("attribute_name")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("factor attribute_name is required")
                        })?;
                    references.push((entity, attribute_name));
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
                        transcript::repository::append_entry(
                            conn,
                            turn.story_id(),
                            transcript::model::kind::DICEROLL,
                            "hidden",
                            Some(&content),
                            &payload,
                            Some(&target_entry_id),
                            Some(&turn_id),
                        )?;
                        Ok((output, chance_source, factors))
                    })
                    .await
                    .map_err(super::to_tool_error)?;

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

#[cfg(test)]
mod turn_tests {
    use super::super::{
        save_character::tool as save_character_tool, test_support::fixture,
        save_relationship::tool as save_relationship_tool,
    };
    use super::schema as roll_check_schema;
    use super::tool as roll_check_tool;
    use crate::features::entities::attributes;
    use serde_json::json;

    #[test]
    fn roll_check_schema_accepts_optional_chance_or_two_attribute_references() {
        let schema = roll_check_schema();
        assert!(schema.get("required").is_none());
        assert_eq!(schema["properties"]["chance_percent"]["minimum"], 0);
        assert_eq!(schema["properties"]["chance_percent"]["maximum"], 100);
        assert_eq!(schema["properties"]["factors"]["maxItems"], 2);
        assert_eq!(
            schema["properties"]["factors"]["items"]["required"],
            json!(["entity", "attribute_name"])
        );
        assert!(schema["properties"].get("attribute").is_none());
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
                    "SELECT COUNT(*) FROM transcript_entries WHERE kind='diceroll'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        turn.with(|conn| {
            let (content, raw, entry, tid): (String, String, String, String) = conn.query_row(
                "SELECT content, payload_json, target_entry_id, turn_id FROM transcript_entries WHERE kind='diceroll'",
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
                    "SELECT COUNT(*) FROM transcript_entries WHERE kind='diceroll'",
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
        save_character_tool(turn.clone(), target.clone(), turn_id.clone(), String::new())
            .execute(json!({"name":"You","stats":[{"attribute":"Stealth","delta":3,"reason":"training"}]}))
            .await.unwrap();
        let id = turn.with(|conn| Ok(super::resolve_entity(conn, turn.story_id(), "You")?.id))
            .await.unwrap();
        let roll = roll_check_tool(turn.clone(), target, turn_id);
        let result = roll
            .execute(json!({"factors":[{"entity":"you","attribute_name":"Stealth"}]}))
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
            .execute(json!({"factors":[{"entity":id,"attribute_name":"Sneaking"}]}))
            .await
            .unwrap();
        assert_eq!(
            alias.as_json().unwrap()["factors"][0]["attribute_name"],
            json!("Stealth")
        );
        assert!(roll
            .execute(json!({"factors":[{"entity":"missing","attribute_name":"Stealth"}]}))
            .await
            .is_err());
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn names_and_relationship_arrows_resolve_to_stored_factor_ids() {
        let (_pool, turn, target, turn_id) = fixture();
        let save = save_character_tool(turn.clone(), target.clone(), turn_id.clone(), String::new());
        save.execute(json!({"name":"Mira","stats":[{"attribute":"Stealth","delta":3,"reason":"practice"}]})).await.unwrap();
        save.execute(json!({"name":"Varro"})).await.unwrap();
        let mira_id = turn.with(|conn| Ok(super::resolve_entity(conn, turn.story_id(), "Mira")?.id))
            .await.unwrap();
        save_relationship_tool(turn.clone(), target.clone(), turn_id.clone(), String::new())
            .execute(json!({"from":"Mira","to":"Varro","label":"rivals",
                "stats":[{"attribute":"Affection","delta":2,"reason":"a truce"}]})).await.unwrap();
        let relationship_id = turn.with(|conn| Ok(super::resolve_entity(conn, turn.story_id(), "Mira → Varro")?.id))
            .await.unwrap();
        let roll = roll_check_tool(turn.clone(), target, turn_id);
        let character = roll.execute(json!({"factors":[{"entity":"MIRA","attribute_name":"Stealth"}]})).await.unwrap();
        assert_eq!(character.as_json().unwrap()["factors"][0]["entity_id"], mira_id);
        assert_eq!(character.as_json().unwrap()["factors"][0]["value"], 8.0);
        for reference in ["Mira → Varro", "mira -> varro"] {
            let relation = roll.execute(json!({"factors":[{"entity":reference,"attribute_name":"Affection"}]})).await.unwrap();
            assert_eq!(relation.as_json().unwrap()["factors"][0]["entity_id"], relationship_id);
            assert_eq!(relation.as_json().unwrap()["factors"][0]["value"], 2.0);
        }
        assert!(roll.execute(json!({"factors":[{"entity":"Varro -> Mira","attribute_name":"Affection"}]})).await.is_err());
        assert!(roll.execute(json!({"factors":[{"entity":"Mira","attribute_name":"Stealth","extra":1}]})).await.is_err());
        assert!(roll.execute(json!({"reason":null})).await.is_err());
        turn.rollback().await.unwrap();
    }
}
