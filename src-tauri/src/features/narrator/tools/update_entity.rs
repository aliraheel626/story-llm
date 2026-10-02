use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities;
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;

use super::to_tool_error;

pub const NAME: &str = "update_entity";
pub const DESCRIPTION: &str =
    "Rename an entity or update its appearance description. Use a known entity id; look it up first when the lookup tool is available.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string", "description": "Entity id from get_entities/create_entity."},
            "name": {"type": "string", "description": "The entity's (possibly unchanged) name."},
            "appearance_anchor": {"type": "string", "description": "The entity's (possibly unchanged) appearance description."}
        },
        "required": ["id", "name"]
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
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolExecutionError::invalid_args("id is required"))?
                    .to_string();
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("name is required"))?
                    .to_string();
                let appearance_anchor = args
                    .get("appearance_anchor")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);

                let updated = turn
                    .with(|conn| {
                        if !entities::list_entities_sync(conn, turn.story_id(), None)?
                            .iter()
                            .any(|entity| entity.id == id)
                        {
                            return Ok(false);
                        }
                        entities::update_entity_sync(
                            conn,
                            turn.story_id(),
                            &id,
                            &name,
                            appearance_anchor.as_deref(),
                            "narrator_tool",
                            Some(&target_entry_id),
                            Some(&turn_id),
                        )?;
                        Ok(true)
                    })
                    .await
                    .map_err(to_tool_error)?;
                if !updated {
                    return Err(ToolExecutionError::invalid_args(format!(
                        "no such entity: {id}"
                    )));
                }
                Ok(ToolOutput::json(
                    json!({"id": id, "name": name, "appearance_anchor": appearance_anchor}),
                ))
            })
        },
    )
}
