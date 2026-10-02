use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities::{self, attributes};
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;

use super::to_tool_error;

pub const NAME: &str = "get_entities";
pub const DESCRIPTION: &str =
    "List known entities (characters, objects, locations) and their current attribute values. \
     Use this to check who or what is present before narrating or changing entity state.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "description": "Filter by kind: character, object, location, relationship, or campaign."},
            "name": {"type": "string", "description": "Filter to an exact (case-insensitive) name match."}
        }
    })
}

fn label(_args: &Value) -> String {
    "Checking who's here…".to_string()
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    catalog::normal_mode(availability) && availability.settings.get_entities
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
    _target_entry_id: String,
    _turn_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        NAME,
        DESCRIPTION,
        schema(),
        move |args: serde_json::Value| {
            let turn = turn.clone();
            Box::pin(async move {
                let kind = args.get("kind").and_then(|v| v.as_str());
                let name = args.get("name").and_then(|v| v.as_str());
                let out = turn
                    .with(|conn| {
                        let mut entities =
                            entities::list_entities_sync(conn, turn.story_id(), kind)?;
                        if let Some(name) = name {
                            entities.retain(|entity| entity.name.eq_ignore_ascii_case(name));
                        }
                        let ids = entities
                            .iter()
                            .map(|entity| entity.id.as_str())
                            .collect::<Vec<_>>();
                        let mut attributes = attributes::list_entity_attributes_for_entities_sync(
                            conn,
                            turn.story_id(),
                            &ids,
                        )?;
                        Ok(entities
                            .into_iter()
                            .map(|entity| {
                                let values = attributes.remove(&entity.id).unwrap_or_default();
                                let snapshot = values
                                    .into_iter()
                                    .map(|value| {
                                        json!({
                                            "name": value.canonical_name, "value": value.value,
                                            "min": value.min, "max": value.max,
                                        })
                                    })
                                    .collect::<Vec<_>>();
                                json!({"id": entity.id, "kind": entity.kind, "name": entity.name,
                            "appearance_anchor": entity.appearance_anchor, "attributes": snapshot})
                            })
                            .collect::<Vec<_>>())
                    })
                    .await
                    .map_err(to_tool_error)?;
                Ok(ToolOutput::json(json!({"entities": out})))
            })
        },
    )
}
