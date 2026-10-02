use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities::{model::CHARACTER, repository};
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;
use crate::shared::error::AppError;

use super::shared::{nullable_field, object_args, required_string};
use super::to_tool_error;

pub const NAME: &str = "update_entity";
pub const DESCRIPTION: &str =
    "Rename a character or update its appearance description. Use a known character id.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string", "description": "A known character id."},
            "name": {"type": "string", "description": "The entity's (possibly unchanged) name."},
            "appearance_anchor": {"type": ["string", "null"], "description": "The character's appearance description. Omit to keep it; null clears it."}
        },
        "required": ["id", "name"],
        "additionalProperties": false
    })
}

fn label(_args: &Value) -> String {
    "Updating an entity…".to_string()
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    catalog::normal_mode(availability) && availability.settings.update_entity
}

fn build(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = catalog::turn(deps);
    tool(turn, target, turn_id)
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
                let fields = object_args(&args, &["id", "name", "appearance_anchor"], NAME).map_err(to_tool_error)?;
                let id = required_string(fields, "id").map_err(to_tool_error)?;
                let name = required_string(fields, "name").map_err(to_tool_error)?;
                let appearance_anchor = nullable_field(fields, "appearance_anchor").map_err(to_tool_error)?;

                let updated = turn
                    .with_savepoint(|conn| {
                        let before = repository::load_entity_raw(conn, turn.story_id(), &id)?
                            .ok_or_else(|| AppError::Invalid(format!("no such entity: {id}")))?;
                        if before.kind != CHARACTER {
                            return Err(AppError::Invalid("update_entity only updates characters".into()));
                        }
                        let anchor = appearance_anchor.clone().unwrap_or(before.appearance_anchor);
                        repository::update_entity_sync(
                            conn,
                            turn.story_id(),
                            &id,
                            &name,
                            anchor.as_deref(),
                            "narrator_tool",
                            Some(&target_entry_id),
                            Some(&turn_id),
                        )
                    })
                    .await
                    .map_err(to_tool_error)?;
                Ok(ToolOutput::json(
                    json!({"id": id, "name": updated.name, "appearance_anchor": updated.appearance_anchor}),
                ))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{create_entity::tool as create_entity_tool, test_support::fixture};

    #[tokio::test]
    async fn nullable_appearance_preserves_omitted_fields_and_rejects_bad_arguments() {
        let (_pool, turn, target, turn_id) = fixture();
        let created = create_entity_tool(turn.clone(), target.clone(), turn_id.clone())
            .execute(json!({"kind":"character","name":"Mira","appearance_anchor":"silver hair"})).await.unwrap();
        let id = created.as_json().unwrap()["id"].as_str().unwrap();
        let update = tool(turn.clone(), target, turn_id);
        let unchanged = update.execute(json!({"id":id,"name":"Mira"})).await.unwrap();
        assert_eq!(unchanged.as_json().unwrap()["appearance_anchor"], "silver hair");
        for args in [
            json!({"id":7,"name":"Mira"}), json!({"id":id,"name":null}),
            json!({"id":id,"name":"Mira","appearance_anchor":false}),
            json!({"id":id,"name":"Mira","unknown":true}),
        ] {
            assert!(update.execute(args).await.is_err());
        }
        let cleared = update.execute(json!({"id":id,"name":"Mira","appearance_anchor":null})).await.unwrap();
        assert_eq!(cleared.as_json().unwrap()["appearance_anchor"], Value::Null);
        assert_eq!(schema()["additionalProperties"], false);
        turn.rollback().await.unwrap();
    }
}
