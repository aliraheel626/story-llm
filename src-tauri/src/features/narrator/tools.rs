//! Narrator tool adapters, schemas, activity labels, and image requests.

use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::json;
use tokio::sync::Mutex;

use crate::features::entities::{self, attributes, registry};
use crate::features::images::model::ImageRequest;
use crate::features::ledger::turn_tx::TurnTx;
use crate::prompts;
use crate::shared::error::AppError;

use super::catalog;

fn to_tool_error(e: AppError) -> ToolExecutionError {
    ToolExecutionError::other(e.to_string())
}

/// Tool name + args → a friendly, generic activity label for the frontend.
pub fn friendly_tool_label(tool_name: &str, args_json: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    catalog::TOOLS
        .iter()
        .find(|spec| spec.name == tool_name)
        .map_or_else(
            || format!("Running {tool_name}…"),
            |spec| (spec.label)(&args),
        )
}

pub(super) fn get_entities_label(_args: &serde_json::Value) -> String {
    "Checking who's here…".to_string()
}

pub(super) fn create_entity_label(args: &serde_json::Value) -> String {
    let name = args
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("someone new");
    format!("Introducing {name}…")
}

pub(super) fn update_entity_label(_args: &serde_json::Value) -> String {
    "Updating an entity…".to_string()
}

pub(super) fn adjust_entity_attribute_label(args: &serde_json::Value) -> String {
    let attribute = args
        .get("attribute")
        .and_then(|value| value.as_str())
        .unwrap_or("an attribute");
    format!("Adjusting {attribute}…")
}

pub(super) fn illustrate_scene_label(_args: &serde_json::Value) -> String {
    "Sketching the scene…".to_string()
}

pub(super) fn illustrate_scene_tool(
    image_requests: Arc<Mutex<Vec<ImageRequest>>>,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::ILLUSTRATE_SCENE_TOOL_NAME,
        prompts::ILLUSTRATE_SCENE_DESCRIPTION,
        prompts::illustrate_scene_schema(),
        move |args: serde_json::Value| {
            let image_requests = image_requests.clone();
            Box::pin(async move {
                let description = args
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("description is required"))?
                    .to_string();
                let character_ids = match args.get("character_ids") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("character_ids must be an array")
                        })?
                        .iter()
                        .map(|id| {
                            id.as_str().map(str::to_string).ok_or_else(|| {
                                ToolExecutionError::invalid_args(
                                    "character_ids must contain only strings",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };

                image_requests.lock().await.push(ImageRequest {
                    description,
                    character_ids,
                });
                Ok(ToolOutput::json(json!({"queued": true})))
            })
        },
    )
}

pub(super) fn get_entities_tool(
    turn: Arc<TurnTx>,
    _target_entry_id: String,
    _turn_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::GET_ENTITIES_TOOL_NAME,
        prompts::GET_ENTITIES_DESCRIPTION,
        prompts::get_entities_schema(),
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

pub(super) fn create_entity_tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::CREATE_ENTITY_TOOL_NAME,
        prompts::CREATE_ENTITY_DESCRIPTION,
        prompts::create_entity_schema(),
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

pub(super) fn update_entity_tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::UPDATE_ENTITY_TOOL_NAME,
        prompts::UPDATE_ENTITY_DESCRIPTION,
        prompts::update_entity_schema(),
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

pub(super) fn adjust_entity_attribute_tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
    embedding_api_key: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::ADJUST_ENTITY_ATTRIBUTE_TOOL_NAME,
        prompts::ADJUST_ENTITY_ATTRIBUTE_DESCRIPTION,
        prompts::adjust_entity_attribute_schema(),
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
    use super::super::catalog::{self, ToolAvailability, ToolDeps};
    use super::*;
    use crate::features::ledger::{
        repository::append_entry,
        turn_tx::{TurnGate, TurnTx},
    };
    use crate::features::stories::settings::NarratorToolSettings;
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
            crate::features::ledger::model::kind::NARRATION,
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
    fn labels_and_catalog_keep_existing_contract() {
        assert_eq!(
            friendly_tool_label("create_entity", r#"{"name":"Mira"}"#),
            "Introducing Mira…"
        );
        assert_eq!(
            friendly_tool_label("roll_check", r#"{"reason":"escaping"}"#),
            "Rolling for escaping…"
        );
        assert_eq!(friendly_tool_label("unknown", "{}"), "Running unknown…");
        let settings = NarratorToolSettings::default();
        let specs = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: false,
            illustrate: false,
        });
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "roll_check");
        let (_pool, turn, target, turn_id) = fixture();
        let deps = ToolDeps {
            turn: Some(turn),
            target_entry_id: Some(target),
            turn_id: Some(turn_id),
            embedding_api_key: String::new(),
            image_requests: Arc::new(Mutex::new(Vec::new())),
        };
        assert_eq!((specs[0].build)(&deps).name(), "roll_check");
    }

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
            let mut stmt = conn.prepare("SELECT kind, target_entry_id, turn_id FROM ledger_entries WHERE kind IN ('entity_created', 'entity_updated') ORDER BY seq")?;
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
                "SELECT payload_json, target_entry_id, turn_id FROM ledger_entries WHERE kind = 'entity_attribute_changed'",
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
        let roll = super::super::dice::roll_check_tool(turn.clone(), target, turn_id)
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
