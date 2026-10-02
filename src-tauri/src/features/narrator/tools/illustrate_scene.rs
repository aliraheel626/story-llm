use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::features::images::model::ImageRequest;
use crate::features::narrator::catalog::{ToolAvailability, ToolDeps, ToolSpec};

use super::shared::{object_args, required_string};
use super::to_tool_error;

pub const NAME: &str = "illustrate_scene";
pub const DESCRIPTION: &str =
    "Generate a scene image. Always call this when the player sends <see>, whatever the \
     subject: that is an explicit request, so never skip it or answer in prose. Call it at most \
     once per turn. When you choose \
     to illustrate on your own, use it sparingly: reserve it for a genuinely striking visual \
     moment (a new place revealed, a character's first appearance, a dramatic turn worth \
     seeing). Write a vivid, concrete visual description of the subject as it appears in the \
     story: subject, setting, composition, lighting. Do not mention art style or medium; \
     that's applied separately.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "description": {"type": "string", "description": "A vivid, concrete visual description of the scene's subject, setting, composition, and lighting."},
            "characters": {"type": "array", "items": {"type": "string"}, "description": "Names of characters visible in the scene."}
        },
        "required": ["description"],
        "additionalProperties": false
    })
}

fn label(_args: &Value) -> String {
    "Sketching the scene…".to_string()
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    availability.image_enabled
}

fn build(deps: &ToolDeps) -> PortableDynamicTool {
    tool(deps.image_requests.clone())
}

pub const SPEC: ToolSpec = ToolSpec {
    name: NAME,
    needs_turn: false,
    enabled,
    build,
    label,
};

pub(super) fn tool(image_requests: Arc<Mutex<Vec<ImageRequest>>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        NAME,
        DESCRIPTION,
        schema(),
        move |args: serde_json::Value| {
            let image_requests = image_requests.clone();
            Box::pin(async move {
                let fields = object_args(&args, &["description", "characters"], NAME).map_err(to_tool_error)?;
                let description = required_string(fields, "description").map_err(to_tool_error)?;
                let character_ids = match fields.get("characters") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("characters must be an array")
                        })?
                        .iter()
                        .map(|id| {
                            id.as_str().map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).ok_or_else(|| {
                                ToolExecutionError::invalid_args(
                                    "characters must contain only non-empty strings",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };

                let mut requests = image_requests.lock().await;
                if !requests.is_empty() {
                    return Ok(ToolOutput::json(json!({
                        "queued": false,
                        "reason": "a scene image is already queued for this turn; only one image per turn",
                    })));
                }
                requests.push(ImageRequest {
                    description,
                    character_ids,
                });
                Ok(ToolOutput::json(json!({"queued": true})))
            })
        },
    )
}

#[cfg(test)]
mod turn_tests {
    use super::tool as illustrate_scene_tool;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn illustrate_scene_queues_one_image_per_turn() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let tool = illustrate_scene_tool(requests.clone());
        let first = tool
            .execute(json!({"description": "a lighthouse at dusk"}))
            .await
            .unwrap();
        assert_eq!(first.as_json().unwrap()["queued"], json!(true));
        let second = tool
            .execute(json!({"description": "the harbor below"}))
            .await
            .unwrap();
        assert_eq!(second.as_json().unwrap()["queued"], json!(false));
        let queued = requests.lock().await;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].description, "a lighthouse at dusk");
    }

    #[tokio::test]
    async fn character_names_are_queued_in_internal_ids_and_arguments_are_strict() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let tool = illustrate_scene_tool(requests.clone());
        for args in [
            json!({"description":false}), json!({"description":"scene","characters":null}),
            json!({"description":"scene","characters":[7]}),
            json!({"description":"scene","characters":[" "]}),
            json!({"description":"scene","character_ids":["mira"]}),
            json!({"description":"scene","unknown":true}),
        ] {
            assert!(tool.execute(args).await.is_err());
        }
        tool.execute(json!({"description":"  lighthouse  ","characters":[" mira ","Unknown"]})).await.unwrap();
        let queued = requests.lock().await;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].description, "lighthouse");
        assert_eq!(queued[0].character_ids, ["mira", "Unknown"]);
        assert_eq!(super::schema()["additionalProperties"], false);
    }
}
