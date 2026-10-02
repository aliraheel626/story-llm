use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities;
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;

use super::to_tool_error;

pub const NAME: &str = "create_entity";
pub const DESCRIPTION: &str =
    "Introduce a new entity (character, object, or location) the story just established. \
     Idempotent by name — calling this for an entity that already exists just returns it.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "description": "character, object, location, relationship, or campaign."},
            "name": {"type": "string", "description": "The entity's name, exactly as it should appear in the story."},
            "appearance_anchor": {"type": "string", "description": "A short, stable visual description to keep the entity consistent."}
        },
        "required": ["kind", "name"]
    })
}

fn label(args: &Value) -> String {
    let name = args
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("someone new");
    format!("Introducing {name}…")
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    catalog::normal_mode(availability) && availability.settings.create_entity
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
                let kind = args
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("kind is required"))?;
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("name is required"))?;
                let appearance_anchor = args.get("appearance_anchor").and_then(|v| v.as_str());

                let (entity, created) = turn
                    .with(|conn| {
                        if let Some(existing) =
                            entities::list_entities_sync(conn, turn.story_id(), Some(kind))?
                                .into_iter()
                                .find(|entity| entity.name.eq_ignore_ascii_case(name))
                        {
                            return Ok((existing, false));
                        }
                        let entity = entities::create_entity_with_id_sync(
                            conn,
                            &uuid::Uuid::new_v4().to_string(),
                            turn.story_id(),
                            kind,
                            name,
                            appearance_anchor,
                            "narrator_tool",
                            Some(&target_entry_id),
                            Some(&turn_id),
                        )?;
                        Ok((entity, true))
                    })
                    .await
                    .map_err(to_tool_error)?;
                Ok(ToolOutput::json(json!({
                    "id": entity.id, "kind": entity.kind, "name": entity.name, "created": created,
                })))
            })
        },
    )
}

#[cfg(test)]
mod turn_tests {
    use super::super::{
        get_entities::tool as get_entities_tool, test_support::fixture,
        update_entity::tool as update_entity_tool,
    };
    use super::tool as create_entity_tool;
    use serde_json::json;

    #[tokio::test]
    async fn entity_tools_write_and_read_on_the_turn_connection() {
        let (pool, turn, target, turn_id) = fixture();
        let create = create_entity_tool(turn.clone(), target.clone(), turn_id.clone());
        let first = create
            .execute(json!({"kind":"character", "name":"Mira", "appearance_anchor":"silver hair"}))
            .await
            .unwrap();
        let id = first.as_json().unwrap()["id"].as_str().unwrap().to_owned();
        assert_eq!(first.as_json().unwrap()["created"], json!(true));
        let second = create
            .execute(json!({"kind":"character", "name":"mira"}))
            .await
            .unwrap();
        assert_eq!(second.as_json().unwrap()["created"], json!(false));
        assert_eq!(second.as_json().unwrap()["id"], json!(id));
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM entities", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let update = update_entity_tool(turn.clone(), target.clone(), turn_id.clone());
        assert!(update
            .execute(json!({"id":"missing", "name":"Nobody"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("no such entity"));
        update
            .execute(json!({"id":id,"name":"Mira the Bold"}))
            .await
            .unwrap();
        let output = get_entities_tool(turn.clone(), target.clone(), turn_id.clone())
            .execute(json!({"kind":"character","name":"mira the bold"}))
            .await
            .unwrap();
        assert_eq!(
            output.as_json().unwrap()["entities"][0]["name"],
            json!("Mira the Bold")
        );
        turn.with(|conn| {
            let mut stmt = conn.prepare("SELECT kind, target_entry_id, turn_id FROM transcript_entries WHERE kind IN ('entity_created', 'entity_updated') ORDER BY seq")?;
            let events = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(events.len(), 2);
            assert!(events.iter().all(|(_, entry, tid)| entry == &target && tid == &turn_id));
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM entities", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
