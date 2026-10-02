use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities::repository;
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;

use super::shared::{apply_resolved_stats, object_args, parse_stats, required_string, resolve_entity, resolve_stats};
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
            "entity": {"type": "string", "description": "A character name, or a relationship as 'Mira → You'. An id is also accepted."},
            "attribute": {"type": "string", "description": "Attribute name, e.g. \"Trust\"."},
            "delta": {"type": "number", "description": "Positive or negative change, on the attribute's own scale."},
            "dramatic": {"type": "boolean", "description": "True only for a major, story-changing swing."},
            "reason": {"type": "string", "description": "Why this changed, for the audit log."}
        },
        "required": ["entity", "attribute", "delta", "reason"],
        "additionalProperties": false
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
                let fields = object_args(&args, &["entity", "attribute", "delta", "reason", "dramatic"], NAME)
                    .map_err(to_tool_error)?;
                let reference = required_string(fields, "entity").map_err(to_tool_error)?;
                let mut change = fields.clone();
                change.remove("entity");
                let changes = parse_stats(Some(&Value::Array(vec![Value::Object(change)])))
                    .map_err(to_tool_error)?.unwrap_or_default();
                let entity = turn.with(|conn| resolve_entity(conn, turn.story_id(), &reference))
                    .await.map_err(to_tool_error)?;
                let resolved = resolve_stats(&turn, &embedding_api_key, &entity.kind, &changes)
                    .await.map_err(to_tool_error)?;
                let results = turn.with_savepoint(|conn| {
                    let current = resolve_entity(conn, turn.story_id(), &entity.id)?;
                    let current = repository::load_entity_raw(conn, turn.story_id(), &current.id)?
                        .ok_or_else(|| crate::shared::error::AppError::Invalid(format!("no entity named '{reference}'")))?;
                    apply_resolved_stats(conn, turn.story_id(), &current.id, resolved, &target_entry_id, &turn_id)
                }).await.map_err(to_tool_error)?;
                Ok(ToolOutput::json(
                    json!({"before": results[0]["before"], "after": results[0]["after"], "applied": true}),
                ))
            })
        },
    )
}

#[cfg(test)]
mod turn_tests {
    use super::super::{
        create_entity::tool as create_entity_tool,
        roll_check::tool as roll_check_tool, test_support::fixture,
    };
    use super::tool as adjust_entity_attribute_tool;
    use crate::features::entities::{attributes, registry};
    use serde_json::json;

    #[tokio::test]
    async fn attribute_tool_applies_delta_over_a_player_set_value() {
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
            .execute(json!({"entity":"Mira","attribute":"Stealth","delta":9,"reason":"sneaking"}))
            .await
            .unwrap();
        assert_eq!(
            output.as_json().unwrap(),
            &json!({"before":5.0,"after":8.0,"applied":true})
        );
        let snapshot = turn.with(|conn| attributes::list_entity_attributes_sync(conn, turn.story_id(), &id))
            .await
            .unwrap();
        assert_eq!(
            json!({"name":snapshot[0].canonical_name,"value":snapshot[0].value,"min":snapshot[0].min,"max":snapshot[0].max}),
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
        let newer = adjust
            .execute(json!({"entity":id,"attribute":"Stealth","delta":-2,"reason":"later story events"}))
            .await
            .unwrap();
        assert_eq!(
            newer.as_json().unwrap(),
            &json!({"before":7.0,"after":5.0,"applied":true})
        );
        turn.with(|conn| {
            let stored = attributes::list_entity_attributes_sync(conn, turn.story_id(), &id)?;
            assert_eq!(stored[0].source, "inferred");
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn minted_attribute_is_immediately_available_to_later_tools() {
        let (pool, turn, target, turn_id) = fixture();
        turn.with(|conn| {
            conn.execute("DELETE FROM attribute_registry WHERE EXISTS (SELECT 1 FROM json_each(entity_kinds_json) WHERE value = 'character')", [])?;
            Ok(())
        }).await.unwrap();
        let id = create_entity_tool(turn.clone(), target.clone(), turn_id.clone())
            .execute(json!({"kind":"character","name":"Prism"}))
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
        .execute(json!({"entity":id,"attribute":"Resonance","delta":1.0,"reason":"resonating"}))
        .await
        .unwrap();
        assert_eq!(output.as_json().unwrap()["after"], json!(6.0));
        let roll = roll_check_tool(turn.clone(), target, turn_id)
            .execute(json!({"factors":[{"entity":id,"attribute_name":"Resonance"}]}))
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
