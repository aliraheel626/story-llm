use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities::{self, attributes, registry};
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;

use super::to_tool_error;

pub const NAME: &str = "adjust_entity_attribute";
pub const DESCRIPTION: &str =
    "Change an entity's attribute by a delta implied by what just happened (an injury, growing \
     trust, a depleted resource). Most changes are minor; only set dramatic for a genuinely \
     major, story-changing swing.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entity_id": {"type": "string", "description": "A known entity id from available context or an enabled lookup/create tool. The player character is named \"You\"."},
            "attribute": {"type": "string", "description": "Attribute name, e.g. \"Trust\"."},
            "delta": {"type": "number", "description": "Positive or negative change, on the attribute's own scale."},
            "dramatic": {"type": "boolean", "description": "True only for a major, story-changing swing."},
            "reason": {"type": "string", "description": "Why this changed, for the audit log."}
        },
        "required": ["entity_id", "attribute", "delta", "reason"]
    })
}

fn label(args: &Value) -> String {
    let attribute = args
        .get("attribute")
        .and_then(|value| value.as_str())
        .unwrap_or("an attribute");
    format!("Adjusting {attribute}…")
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    catalog::normal_mode(availability) && availability.settings.adjust_entity_attribute
}

fn build(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = catalog::turn(deps);
    tool(turn, target, turn_id, deps.embedding_api_key.clone())
}

pub const SPEC: ToolSpec = ToolSpec {
    name: NAME,
    instruction: None,
    needs_turn: true,
    enabled,
    build,
    label,
};

pub(super) fn tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
    embedding_api_key: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        NAME,
        DESCRIPTION,
        schema(),
        move |args: serde_json::Value| {
            let turn = turn.clone();
            let target_entry_id = target_entry_id.clone();
            let turn_id = turn_id.clone();
            let embedding_api_key = embedding_api_key.clone();
            Box::pin(async move {
                let entity_id = args
                    .get("entity_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolExecutionError::invalid_args("entity_id is required"))?
                    .to_string();
                let attribute_name = args
                    .get("attribute")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("attribute is required"))?
                    .to_string();
                let delta = args
                    .get("delta")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| ToolExecutionError::invalid_args("delta is required"))?;
                let dramatic = args
                    .get("dramatic")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let reason = args
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("narration")
                    .to_string();

                let entity = turn
                    .with(|conn| {
                        Ok(entities::list_entities_sync(conn, turn.story_id(), None)?
                            .into_iter()
                            .find(|entity| entity.id == entity_id))
                    })
                    .await
                    .map_err(to_tool_error)?
                    .ok_or_else(|| {
                        ToolExecutionError::invalid_args(format!("no such entity: {entity_id}"))
                    })?;
                let resolution = registry::resolve_attribute_in_turn(
                    &turn,
                    &embedding_api_key,
                    &attribute_name,
                    &entity.kind,
                )
                .await
                .map_err(to_tool_error)?;
                let (before, after, applied) = turn
                    .with(|conn| {
                        let attribute = if let Some(exact) =
                            registry::find_exact_match(conn, &attribute_name)?
                        {
                            exact
                        } else {
                            match resolution {
                                registry::AttributeResolution::Existing(attribute) => attribute,
                                registry::AttributeResolution::AddAlias { attribute, alias } => {
                                    registry::add_alias(conn, &attribute.id, &alias)?;
                                    attribute
                                }
                                registry::AttributeResolution::Mint(attribute) => {
                                    let id = registry::insert_minted_attribute(conn, &attribute)?;
                                    registry::find_attribute_by_id(conn, &id)?
                                }
                            }
                        };
                        let (before, source) = attributes::peek_entity_attribute(
                            conn,
                            turn.story_id(),
                            &entity.id,
                            &attribute,
                        )?;
                        if source.as_deref() == Some("user") {
                            return Ok((before, before, false));
                        }
                        let result = attributes::apply_attribute_delta(
                            conn,
                            turn.story_id(),
                            &entity.id,
                            &attribute,
                            delta,
                            &reason,
                            &target_entry_id,
                            dramatic,
                            Some(&turn_id),
                        )?;
                        Ok((result.0, result.1, true))
                    })
                    .await
                    .map_err(to_tool_error)?;
                if !applied {
                    return Ok(ToolOutput::json(json!({
                        "before": before,
                        "after": before,
                        "applied": false,
                        "reason": "locked to a player-set value",
                    })));
                }
                Ok(ToolOutput::json(
                    json!({"before": before, "after": after, "applied": true}),
                ))
            })
        },
    )
}

#[cfg(test)]
mod turn_tests {
    use super::super::{
        create_entity::tool as create_entity_tool, get_entities::tool as get_entities_tool,
        roll_check::tool as roll_check_tool, test_support::fixture,
    };
    use super::tool as adjust_entity_attribute_tool;
    use crate::features::entities::{attributes, registry};
    use serde_json::json;

    #[tokio::test]
    async fn attribute_tool_applies_delta_and_honors_player_lock() {
        let (_pool, turn, target, turn_id) = fixture();
        let id = create_entity_tool(turn.clone(), target.clone(), turn_id.clone())
            .execute(json!({"kind":"character","name":"Mira"}))
            .await
            .unwrap()
            .as_json()
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let adjust = adjust_entity_attribute_tool(
            turn.clone(),
            target.clone(),
            turn_id.clone(),
            String::new(),
        );
        let output = adjust
            .execute(json!({"entity_id":id,"attribute":"Stealth","delta":9,"reason":"sneaking"}))
            .await
            .unwrap();
        assert_eq!(
            output.as_json().unwrap(),
            &json!({"before":5.0,"after":8.0,"applied":true})
        );
        let snapshot = get_entities_tool(turn.clone(), target.clone(), turn_id.clone())
            .execute(json!({"name":"Mira"}))
            .await
            .unwrap();
        assert_eq!(
            snapshot.as_json().unwrap()["entities"][0]["attributes"][0],
            json!({"name":"Stealth","value":8.0,"min":0.0,"max":10.0})
        );
        turn.with(|conn| {
            let attribute = attributes::find_exact_match(conn, "Stealth")?.unwrap();
            let entry: (String, String, String) = conn.query_row(
                "SELECT payload_json, target_entry_id, turn_id FROM transcript_entries WHERE kind = 'entity_attribute_changed'",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            assert_eq!(serde_json::from_str::<serde_json::Value>(&entry.0).unwrap()["cause"], json!("sneaking"));
            assert_eq!((entry.1, entry.2), (target.clone(), turn_id.clone()));
            conn.execute("UPDATE entity_attributes SET value = 7, source = 'user' WHERE entity_id = ?1 AND attribute_id = ?2",
                rusqlite::params![id, attribute.id])?;
            Ok(())
        }).await.unwrap();
        let locked = adjust
            .execute(json!({"entity_id":id,"attribute":"Stealth","delta":5}))
            .await
            .unwrap();
        assert_eq!(
            locked.as_json().unwrap(),
            &json!({"before":7.0,"after":7.0,"applied":false,"reason":"locked to a player-set value"})
        );
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn minted_attribute_is_immediately_available_to_later_tools() {
        let (pool, turn, target, turn_id) = fixture();
        let id = create_entity_tool(turn.clone(), target.clone(), turn_id.clone())
            .execute(json!({"kind":"artifact","name":"Prism"}))
            .await
            .unwrap()
            .as_json()
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let output = adjust_entity_attribute_tool(
            turn.clone(),
            target.clone(),
            turn_id.clone(),
            String::new(),
        )
        .execute(json!({"entity_id":id,"attribute":"Resonance","delta":1.0}))
        .await
        .unwrap();
        assert_eq!(output.as_json().unwrap()["after"], json!(6.0));
        let roll = roll_check_tool(turn.clone(), target, turn_id)
            .execute(json!({"factors":[{"entity_id":id,"attribute_name":"Resonance"}]}))
            .await
            .unwrap();
        assert_eq!(roll.as_json().unwrap()["factors"][0]["value"], json!(6.0));
        turn.with(|conn| {
            assert!(registry::find_exact_match(conn, "Resonance")?.is_some());
            Ok(())
        })
        .await
        .unwrap();
        assert!(
            registry::find_exact_match(&pool.get().unwrap(), "Resonance")
                .unwrap()
                .is_none()
        );
        turn.rollback().await.unwrap();
    }
}
