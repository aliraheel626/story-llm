//! Narrator tool adapters, schemas, activity labels, and image requests.

use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::json;
use tokio::sync::Mutex;

use crate::features::images::model::ImageRequest;
use crate::prompts;
use crate::shared::error::AppError;

use super::catalog;
use super::staging::{resolve_or_stage_attribute, TurnStaging};

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

pub(super) fn get_entities_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
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

pub(super) fn create_entity_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
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

pub(super) fn update_entity_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
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

pub(super) fn adjust_entity_attribute_tool(
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

#[cfg(test)]
mod tests {
    use super::super::catalog::{self, ToolAvailability, ToolDeps};
    use super::super::staging::PendingOp;
    use super::*;
    use crate::features::entities::model::AttributeRegistryEntry;
    use crate::features::ledger::repository::append_entry;
    use crate::features::stories::settings::NarratorToolSettings;
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
        portable_tools_for_availability(staging, &NarratorToolSettings::default(), false, false)
    }

    fn portable_tools_for_availability(
        staging: Arc<Mutex<TurnStaging>>,
        settings: &NarratorToolSettings,
        image_enabled: bool,
        illustrate: bool,
    ) -> Vec<PortableDynamicTool> {
        let specs = catalog::enabled(&ToolAvailability {
            settings,
            image_enabled,
            illustrate,
        });
        let deps = ToolDeps {
            staging: Some(staging),
            embedding_api_key: test_embedding_api_key(),
            image_requests: Arc::new(Mutex::new(Vec::new())),
        };
        specs.iter().map(|spec| (spec.build)(&deps)).collect()
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
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id)));
        let enabled = portable_tools_for_availability(
            staging.clone(),
            &NarratorToolSettings::default(),
            true,
            true,
        );
        let disabled =
            portable_tools_for_availability(staging, &NarratorToolSettings::default(), false, true);

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
            let tools = portable_tools_for_availability(staging.clone(), &settings, false, false);
            assert_eq!(
                tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
                vec![expected]
            );
        }
        assert!(
            portable_tools_for_availability(staging.clone(), &none(), false, false,).is_empty()
        );
        let image_only = NarratorToolSettings {
            illustrate_scene: true,
            ..none()
        };
        assert!(
            portable_tools_for_availability(staging.clone(), &image_only, false, false,).is_empty()
        );
        let tools = portable_tools_for_availability(
            staging,
            &NarratorToolSettings::default(),
            false,
            false,
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
