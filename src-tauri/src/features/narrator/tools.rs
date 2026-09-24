//! Narrator tool adapters, schemas, activity labels, and image requests.

use std::sync::Arc;

use rig_agent::tool::{DynamicTool, PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::json;
use tokio::sync::Mutex;

use crate::features::images::model::ImageRequest;
use crate::features::stories::settings::NarratorToolSettings;
use crate::prompts;
use crate::shared::error::AppError;

use super::staging::{chance_from_factors, resolve_or_stage_attribute, TurnStaging};

fn to_tool_error(e: AppError) -> ToolExecutionError {
    ToolExecutionError::other(e.to_string())
}

/// `roll_check` + args → a friendly, generic activity label for the frontend
/// (e.g. "Rolling for an escape…") — kept next to the tool definitions since it
/// needs to know each tool's argument shape.
pub fn friendly_tool_label(tool_name: &str, args_json: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    let str_arg = |key: &str| args.get(key).and_then(|v| v.as_str()).map(str::to_string);
    match tool_name {
        "roll_check" => format!(
            "Rolling for {}…",
            str_arg("reason").unwrap_or_else(|| "a check".into())
        ),
        "get_entities" => "Checking who's here…".to_string(),
        "create_entity" => format!(
            "Introducing {}…",
            str_arg("name").unwrap_or_else(|| "someone new".into())
        ),
        "update_entity" => "Updating an entity…".to_string(),
        "adjust_entity_attribute" => {
            format!(
                "Adjusting {}…",
                str_arg("attribute").unwrap_or_else(|| "an attribute".into())
            )
        }
        "illustrate_scene" => "Sketching the scene…".to_string(),
        other => format!("Running {other}…"),
    }
}

pub fn illustrate_scene_tool(image_requests: Arc<Mutex<Vec<ImageRequest>>>) -> PortableDynamicTool {
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

pub fn narrator_image_tools(enabled: bool) -> (Vec<DynamicTool>, Arc<Mutex<Vec<ImageRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let tools = if enabled {
        vec![DynamicTool::from_portable(illustrate_scene_tool(
            requests.clone(),
        ))]
    } else {
        Vec::new()
    };
    (tools, requests)
}

fn roll_check_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
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
                let factor_args: &[serde_json::Value] =
                    match args.get("factors") {
                        None => &[],
                        Some(serde_json::Value::Array(factors)) if factors.len() <= 2 => factors,
                        _ => return Err(ToolExecutionError::invalid_args(
                            "factors must be an array of at most two entity-attribute references",
                        )),
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
                    factors.push(
                        staging
                            .roll_factor(entity_id, attribute_name)
                            .map_err(|error| ToolExecutionError::invalid_args(error.to_string()))?,
                    );
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
                let output = staging.stage_roll(
                    chance_percent,
                    reason.clone(),
                    chance_source,
                    factors.clone(),
                );

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

fn get_entities_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::GET_ENTITIES_TOOL_NAME,
        prompts::GET_ENTITIES_DESCRIPTION,
        prompts::get_entities_schema(),
        move |args: serde_json::Value| {
            let staging = staging.clone();
            Box::pin(async move {
                let kind = args.get("kind").and_then(|v| v.as_str());
                let name = args.get("name").and_then(|v| v.as_str());
                let staging = staging.lock().await;
                let entities = staging
                    .effective_entities(kind, name)
                    .map_err(to_tool_error)?;
                let entity_ids = entities
                    .iter()
                    .map(|entity| entity.id.clone())
                    .collect::<Vec<_>>();
                let entity_id_refs = entity_ids.iter().map(String::as_str).collect::<Vec<_>>();
                let mut attributes_by_entity = staging
                    .attribute_snapshot_for_entities(&entity_id_refs)
                    .map_err(to_tool_error)?;
                let mut out = Vec::new();
                for entity in entities {
                    let attributes = attributes_by_entity.remove(&entity.id).unwrap_or_default();
                    out.push(json!({
                        "id": entity.id, "kind": entity.kind, "name": entity.name,
                        "appearance_anchor": entity.appearance_anchor, "attributes": attributes,
                    }));
                }
                Ok(ToolOutput::json(json!({"entities": out})))
            })
        },
    )
}

fn create_entity_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::CREATE_ENTITY_TOOL_NAME,
        prompts::CREATE_ENTITY_DESCRIPTION,
        prompts::create_entity_schema(),
        move |args: serde_json::Value| {
            let staging = staging.clone();
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

                let mut staging = staging.lock().await;
                let (entity, created) = staging
                    .resolve_or_stage_entity(kind, name, appearance_anchor)
                    .map_err(to_tool_error)?;
                Ok(ToolOutput::json(json!({
                    "id": entity.id, "kind": entity.kind, "name": entity.name, "created": created,
                })))
            })
        },
    )
}

fn update_entity_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::UPDATE_ENTITY_TOOL_NAME,
        prompts::UPDATE_ENTITY_DESCRIPTION,
        prompts::update_entity_schema(),
        move |args: serde_json::Value| {
            let staging = staging.clone();
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

                let mut staging = staging.lock().await;
                if staging
                    .find_effective_entity(&id)
                    .map_err(to_tool_error)?
                    .is_none()
                {
                    return Err(ToolExecutionError::invalid_args(format!(
                        "no such entity: {id}"
                    )));
                }
                staging.stage_entity_update(id.clone(), name.clone(), appearance_anchor.clone());
                Ok(ToolOutput::json(
                    json!({"id": id, "name": name, "appearance_anchor": appearance_anchor}),
                ))
            })
        },
    )
}

fn adjust_entity_attribute_tool(
    staging: Arc<Mutex<TurnStaging>>,
    embedding_api_key: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        prompts::ADJUST_ENTITY_ATTRIBUTE_TOOL_NAME,
        prompts::ADJUST_ENTITY_ATTRIBUTE_DESCRIPTION,
        prompts::adjust_entity_attribute_schema(),
        move |args: serde_json::Value| {
            let staging = staging.clone();
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

                let entity = {
                    let staging = staging.lock().await;
                    let entity = staging
                        .find_effective_entity(&entity_id)
                        .map_err(to_tool_error)?
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args(format!("no such entity: {entity_id}"))
                        })?;
                    entity
                };

                let attribute = resolve_or_stage_attribute(
                    &staging,
                    &embedding_api_key,
                    &attribute_name,
                    &entity.kind,
                )
                .await
                .map_err(to_tool_error)?;

                let mut staging = staging.lock().await;
                let (before, after, applied) = staging
                    .stage_attribute_delta(&entity.id, attribute, delta, reason, dramatic)
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

/// The enabled tools for one turn. Illustration is built separately because
/// it also requires a configured global image service and API key.
pub fn narrator_portable_tools_for_settings(
    staging: Arc<Mutex<TurnStaging>>,
    embedding_api_key: String,
    settings: &NarratorToolSettings,
) -> Vec<PortableDynamicTool> {
    let mut tools = Vec::new();
    if settings.roll_check {
        tools.push(roll_check_tool(staging.clone()));
    }
    if settings.get_entities {
        tools.push(get_entities_tool(staging.clone()));
    }
    if settings.create_entity {
        tools.push(create_entity_tool(staging.clone()));
    }
    if settings.update_entity {
        tools.push(update_entity_tool(staging.clone()));
    }
    if settings.adjust_entity_attribute {
        tools.push(adjust_entity_attribute_tool(staging, embedding_api_key));
    }
    tools
}

pub fn narrator_tools_for_settings(
    staging: Arc<Mutex<TurnStaging>>,
    embedding_api_key: String,
    settings: &NarratorToolSettings,
) -> Vec<DynamicTool> {
    narrator_portable_tools_for_settings(staging, embedding_api_key, settings)
        .into_iter()
        .map(DynamicTool::from_portable)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::staging::PendingOp;
    use super::*;
    use crate::features::entities::{self, attributes, model::AttributeRegistryEntry};
    use crate::features::ledger;
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

    /// Tool tests use seeded attribute names unless they deliberately
    /// exercise the no-candidate mint path, so no embedding call needs a
    /// real key.
    fn test_embedding_api_key() -> String {
        String::new()
    }

    /// The portable tool set, so each tool's real body (arg parsing, error
    /// mapping, staging) can be executed without Rig's private dispatch.
    fn portable_tools(staging: Arc<Mutex<TurnStaging>>) -> Vec<PortableDynamicTool> {
        narrator_portable_tools_for_settings(
            staging,
            test_embedding_api_key(),
            &NarratorToolSettings::default(),
        )
    }

    fn tool_named<'a>(tools: &'a [PortableDynamicTool], name: &str) -> &'a PortableDynamicTool {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool named {name}"))
    }

    /// Commits staged ops against a real narration entry the way
    /// `append_narration_entry` does — one transaction, no model involved.
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
    fn image_tools_include_only_illustration_when_enabled() {
        let (enabled, _) = narrator_image_tools(true);
        let (disabled, _) = narrator_image_tools(false);

        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name(), "illustrate_scene");
        assert!(disabled.is_empty());
    }

    #[test]
    fn tools_are_independently_available() {
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id)));
        let cases = [
            (
                "get_entities",
                NarratorToolSettings {
                    get_entities: true,
                    ..none()
                },
            ),
            (
                "create_entity",
                NarratorToolSettings {
                    create_entity: true,
                    ..none()
                },
            ),
            (
                "update_entity",
                NarratorToolSettings {
                    update_entity: true,
                    ..none()
                },
            ),
            (
                "adjust_entity_attribute",
                NarratorToolSettings {
                    adjust_entity_attribute: true,
                    ..none()
                },
            ),
            (
                "roll_check",
                NarratorToolSettings {
                    roll_check: true,
                    ..none()
                },
            ),
        ];
        for (expected, settings) in cases {
            let tools = narrator_portable_tools_for_settings(
                staging.clone(),
                test_embedding_api_key(),
                &settings,
            );
            assert_eq!(
                tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
                vec![expected]
            );
        }
        assert!(narrator_portable_tools_for_settings(
            staging.clone(),
            test_embedding_api_key(),
            &none()
        )
        .is_empty());
        let image_only = NarratorToolSettings {
            illustrate_scene: true,
            ..none()
        };
        assert!(narrator_portable_tools_for_settings(
            staging.clone(),
            test_embedding_api_key(),
            &image_only
        )
        .is_empty());
        let tools = narrator_portable_tools_for_settings(
            staging,
            test_embedding_api_key(),
            &NarratorToolSettings::default(),
        );
        assert_eq!(tools.len(), 5);
        assert_eq!(tools[0].name(), "roll_check");
    }

    fn none() -> NarratorToolSettings {
        NarratorToolSettings {
            get_entities: false,
            create_entity: false,
            update_entity: false,
            adjust_entity_attribute: false,
            roll_check: false,
            illustrate_scene: false,
        }
    }

    #[test]
    fn friendly_labels_describe_each_tool_and_degrade_safely() {
        assert_eq!(
            friendly_tool_label("roll_check", r#"{"reason":"escaping"}"#),
            "Rolling for escaping…"
        );
        assert_eq!(
            friendly_tool_label("get_entities", "{}"),
            "Checking who's here…"
        );
        assert_eq!(
            friendly_tool_label("create_entity", r#"{"name":"Mira"}"#),
            "Introducing Mira…"
        );
        assert_eq!(
            friendly_tool_label("adjust_entity_attribute", r#"{"attribute":"Trust"}"#),
            "Adjusting Trust…"
        );
        assert_eq!(
            friendly_tool_label("illustrate_scene", "{}"),
            "Sketching the scene…"
        );
        // Missing args and malformed JSON must not panic — they are only
        // labels for a transient UI line.
        assert_eq!(
            friendly_tool_label("roll_check", "{}"),
            "Rolling for a check…"
        );
        assert_eq!(
            friendly_tool_label("roll_check", "not json"),
            "Rolling for a check…"
        );
        assert_eq!(
            friendly_tool_label("mystery_tool", "{}"),
            "Running mystery_tool…"
        );
    }

    #[tokio::test]
    async fn illustrate_scene_tool_queues_requests_and_rejects_empty_descriptions() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let tool = illustrate_scene_tool(requests.clone());

        let output = tool
            .execute(json!({
                "description": "  Mira stands beneath a lightning-split sky.  ",
                "character_ids": ["mira", "watcher"]
            }))
            .await
            .unwrap();
        assert_eq!(output.as_json().unwrap()["queued"], json!(true));

        let requests_guard = requests.lock().await;
        assert_eq!(requests_guard.len(), 1);
        assert_eq!(
            requests_guard[0].description,
            "Mira stands beneath a lightning-split sky."
        );
        assert_eq!(requests_guard[0].character_ids, ["mira", "watcher"]);
        drop(requests_guard);

        let error = tool
            .execute(json!({"description": "   "}))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("description is required"),
            "{error}"
        );
        assert_eq!(requests.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn create_entity_tool_is_idempotent_by_name_and_reports_staging() {
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id)));
        let tools = portable_tools(staging.clone());

        let first = tool_named(&tools, "create_entity")
            .execute(
                json!({"kind": "character", "name": "Mira", "appearance_anchor": "silver hair"}),
            )
            .await
            .unwrap();
        let first = first.as_json().unwrap();
        assert_eq!(first["created"], json!(true));
        assert_eq!(first["name"], json!("Mira"));

        // Same name, different case: the second call must find the staged
        // entity rather than staging a duplicate.
        let second = tool_named(&tools, "create_entity")
            .execute(json!({"kind": "character", "name": "mira"}))
            .await
            .unwrap();
        let second = second.as_json().unwrap();
        assert_eq!(second["created"], json!(false));
        assert_eq!(second["id"], first["id"]);
        assert_eq!(staging.lock().await.pending.len(), 1);

        // A missing required argument is a recoverable error, not a panic.
        let err = tool_named(&tools, "create_entity")
            .execute(json!({"kind": "character", "name": "   "}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("name is required"), "{err}");
    }

    #[tokio::test]
    async fn update_entity_tool_rejects_unknown_ids_and_overlays_renames() {
        let (pool, story_id) = setup();
        let mut staging = TurnStaging::new(pool, story_id);
        let (mira, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let err = tool_named(&tools, "update_entity")
            .execute(json!({"id": "does-not-exist", "name": "Nobody"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no such entity"), "{err}");

        let out = tool_named(&tools, "update_entity")
            .execute(json!({"id": mira.id, "name": "Mira the Bold"}))
            .await
            .unwrap();
        assert_eq!(out.as_json().unwrap()["name"], json!("Mira the Bold"));

        // The overlay a later tool call sees reflects the staged rename.
        let staging = staging.lock().await;
        assert_eq!(
            staging
                .find_effective_entity(&mira.id)
                .unwrap()
                .unwrap()
                .name,
            "Mira the Bold"
        );
        assert!(staging
            .effective_entities(Some("character"), Some("mira the bold"))
            .unwrap()
            .iter()
            .any(|e| e.id == mira.id));
    }

    #[tokio::test]
    async fn adjust_attribute_tool_previews_clamped_delta_and_rejects_unknown_entity() {
        let (pool, story_id) = setup();
        let stealth = find_attribute(&pool.get().unwrap(), "Stealth");
        let mut staging = TurnStaging::new(pool, story_id);
        let (mira, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let err = tool_named(&tools, "adjust_entity_attribute")
            .execute(
                json!({"entity_id": "ghost", "attribute": "Stealth", "delta": 1.0, "reason": "x"}),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no such entity"), "{err}");

        // Stealth is a seeded 0-10 character attribute, so a non-dramatic
        // +9 is rate-limited to +3 off its 5.0 midpoint.
        let out = tool_named(&tools, "adjust_entity_attribute")
            .execute(
                json!({"entity_id": mira.id, "attribute": "Stealth", "delta": 9.0, "reason": "sneaking"}),
            )
            .await
            .unwrap();
        let out = out.as_json().unwrap();
        assert_eq!(out["before"], json!(5.0));
        assert_eq!(out["after"], json!(8.0));

        let staging = staging.lock().await;
        let staged = staging
            .pending
            .iter()
            .find_map(|op| match op {
                PendingOp::AdjustAttribute {
                    delta,
                    cause,
                    dramatic,
                    ..
                } => Some((*delta, cause.clone(), *dramatic)),
                _ => None,
            })
            .unwrap();
        assert_eq!(staged, (9.0, "sneaking".to_string(), false));
        assert_eq!(
            staging
                .effective_attribute_state(&mira.id, &stealth)
                .unwrap()
                .0,
            8.0
        );
    }

    #[tokio::test]
    async fn roll_check_stages_only_chance_and_reason_without_entity_side_effects() {
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let tools = portable_tools(staging.clone());
        let out = tool_named(&tools, "roll_check")
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
        assert!(matches!(staging.pending.as_slice(), [PendingOp::Roll(_)]));
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
        assert_eq!(payload.as_object().unwrap().len(), 7);
        assert_eq!(payload["chance_percent"], json!(35));
        assert_eq!(payload["roll"], out["roll"]);
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
        let tools = portable_tools(staging.clone());
        let roll = tool_named(&tools, "roll_check");
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
        )
        .unwrap();
        let stealth = find_attribute(&conn, "Stealth");
        let perception = find_attribute(&conn, "Perception");
        attributes::set_entity_attribute_sync(&conn, &story_id, "actor", &stealth.id, 8.0).unwrap();
        attributes::set_entity_attribute_sync(&conn, &story_id, "guard", &perception.id, 6.0)
            .unwrap();
        drop(conn);

        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let tools = portable_tools(staging.clone());
        let roll = tool_named(&tools, "roll_check");
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
        let staging = Arc::new(Mutex::new(staging));
        let roll = roll_check_tool(staging);
        let result = roll
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
                resolve_or_stage_attribute(turn, &test_embedding_api_key(), name, "artifact")
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
        let roll = roll_check_tool(second.clone());
        let result = roll
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

    #[tokio::test]
    async fn get_entities_tool_reports_staged_entities_with_attributes() {
        let (pool, story_id) = setup();
        let mut staging = TurnStaging::new(pool, story_id);
        staging
            .resolve_or_stage_entity("location", "The Drowned Keep", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let out = tool_named(&tools, "get_entities")
            .execute(json!({"kind": "location"}))
            .await
            .unwrap();
        let entities = out.as_json().unwrap()["entities"].clone();
        let entities = entities.as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["name"], json!("The Drowned Keep"));
        assert_eq!(entities[0]["kind"], json!("location"));
        assert_eq!(entities[0]["attributes"], json!([]));
        assert_eq!(staging.lock().await.pending.len(), 1);
    }

    #[tokio::test]
    async fn committed_tool_delta_does_not_override_a_user_set_attribute() {
        let (pool, story_id) = setup();
        let stealth = find_attribute(&pool.get().unwrap(), "Stealth");
        let mut staging = TurnStaging::new(pool.clone(), story_id.clone());
        let (mira, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();

        // Land the entity, then set a value the player owns explicitly.
        persist_staging(&pool, &story_id, &staging);
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO entity_attributes (story_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
             VALUES (?1, ?2, ?3, 7.0, 'user', 'now', NULL)",
            rusqlite::params![story_id, mira.id, stealth.id],
        )
        .unwrap();

        // Even if a delta was staged before the lock became visible, every
        // preview path must mirror commit's no-op behavior.
        let mut stale_preview = TurnStaging::new(pool.clone(), story_id.clone());
        stale_preview.pending.push(PendingOp::AdjustAttribute {
            entity_id: mira.id.clone(),
            attribute: stealth.clone(),
            delta: 5.0,
            cause: "stale preview".into(),
            dramatic: true,
        });
        assert_eq!(
            stale_preview
                .effective_attribute_state(&mira.id, &stealth)
                .unwrap()
                .0,
            7.0
        );
        let snapshot = stale_preview
            .attribute_snapshot_for_entities(&[&mira.id])
            .unwrap();
        assert_eq!(snapshot[&mira.id][0]["value"], json!(7.0));

        let staged = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let tools = portable_tools(staged.clone());
        let preview = tool_named(&tools, "adjust_entity_attribute")
            .execute(json!({
                "entity_id": mira.id,
                "attribute": "Stealth",
                "delta": 5.0,
                "dramatic": true,
                "reason": "the narrator decided so",
            }))
            .await
            .unwrap();
        let preview = preview.as_json().unwrap();
        assert_eq!(preview["before"], json!(7.0));
        assert_eq!(preview["after"], json!(7.0));
        assert_eq!(preview["applied"], json!(false));
        assert_eq!(preview["reason"], json!("locked to a player-set value"));

        let staged = staged.lock().await;
        assert!(!staged
            .pending
            .iter()
            .any(|op| matches!(op, PendingOp::AdjustAttribute { .. })));
        persist_staging(&pool, &story_id, &staged);

        // The narrator context promises the model that user overrides win; an
        // inferred tool delta must not quietly overwrite this.
        let (value, source): (f64, String) = conn
            .query_row(
                "SELECT value, source FROM entity_attributes WHERE entity_id = ?1 AND attribute_id = ?2",
                rusqlite::params![mira.id, stealth.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(value, 7.0);
        assert_eq!(source, "user");
    }
}
