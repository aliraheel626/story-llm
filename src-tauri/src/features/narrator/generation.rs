use std::sync::Arc;

use rig_agent::tool::DynamicTool;
use tauri::AppHandle;
use tokio::sync::Mutex;

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{
    context::{self, ContextPlan},
    entities, images, settings, stories,
    turn::TurnTx,
};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::catalog::{self, ToolAvailability, ToolDeps};
use super::model::NarratorPurpose;

fn entity_snapshot(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<context::injection::EntityDisplay>> {
    let entities = entities::list_entities_sync(conn, story_id, None)?;
    let entity_ids = entities
        .iter()
        .map(|entity| entity.id.as_str())
        .collect::<Vec<_>>();
    let mut attrs = entities::attributes::list_entity_attributes_for_entities_sync(
        conn,
        story_id,
        &entity_ids,
    )?;
    Ok(entities
        .into_iter()
        .map(|entity| context::injection::EntityDisplay {
            attributes: attrs
                .remove(&entity.id)
                .unwrap_or_default()
                .into_iter()
                .map(|attribute| context::injection::AttributeDisplay {
                    canonical_name: attribute.canonical_name,
                    value: attribute.value,
                })
                .collect(),
            id: entity.id,
            name: entity.name,
            kind: entity.kind,
            appearance_anchor: entity.appearance_anchor,
        })
        .collect())
}

pub struct NarratorInputs<'a> {
    pub app: &'a AppHandle,
    pub settings_pool: &'a Pool,
    pub turn: Arc<TurnTx>,
    pub story_id: &'a str,
    pub transcript: Vec<HistoryTurn>,
    pub purpose: NarratorPurpose,
    pub target_entry_id: String,
    pub turn_id: String,
}

pub struct Prepared {
    pub(super) app: AppHandle,
    pub(super) turn: Arc<TurnTx>,
    pub(super) story_id: String,
    pub(super) target_entry_id: String,
    pub(super) turn_id: String,
    pub(super) config: TextModelConfig,
    pub(super) transcript: Vec<HistoryTurn>,
    pub(super) context: ContextPlan,
    pub(super) tools: Vec<DynamicTool>,
    pub(super) stop_after_tool_result: bool,
    pub(super) reasoning_effort: Option<String>,
    pub(super) image_requests: Arc<Mutex<Vec<images::model::ImageRequest>>>,
}

pub async fn prepare(inputs: NarratorInputs<'_>) -> AppResult<Prepared> {
    match inputs.transcript.last() {
        None => {
            return Err(AppError::Invalid(
                "the narration history has no action turn".into(),
            ))
        }
        Some(turn) if !turn.is_player => {
            return Err(AppError::Invalid(
                "the narration history does not end with an action turn".into(),
            ));
        }
        _ => {}
    }

    let config = settings::resolve_text_model(inputs.app, inputs.settings_pool)?;
    let (tool_settings, reasoning_effort) = inputs
        .turn
        .with(|conn| {
            Ok((
                stories::settings::read_story_narrator_tools_conn(conn, inputs.story_id)?,
                stories::settings::read_story_reasoning_effort_conn(conn, inputs.story_id)?,
            ))
        })
        .await?;
    let reasoning_effort = (!reasoning_effort.is_empty()).then_some(reasoning_effort);
    let image_settings = settings::read_image_model_settings(inputs.app, inputs.settings_pool)?;
    let image_enabled =
        image_settings.enabled && image_settings.has_api_key && tool_settings.illustrate_scene;
    let illustrate = matches!(inputs.purpose, NarratorPurpose::Illustrate);
    if illustrate && !image_enabled {
        return Err(AppError::Invalid(
            "image generation is disabled or has no API key".into(),
        ));
    }
    let enabled_tools = catalog::enabled(&ToolAvailability {
        settings: &tool_settings,
        image_enabled,
        illustrate,
    });
    let world_tools_enabled = enabled_tools.iter().any(|spec| spec.needs_turn);
    let embedding_api_key = if world_tools_enabled {
        settings::read_api_key(inputs.app, "openrouter").unwrap_or_default()
    } else {
        String::new()
    };
    let image_requests = Arc::new(Mutex::new(Vec::new()));
    let deps = ToolDeps {
        turn: Some(Arc::clone(&inputs.turn)),
        target_entry_id: Some(inputs.target_entry_id.clone()),
        turn_id: Some(inputs.turn_id.clone()),
        embedding_api_key,
        image_requests: image_requests.clone(),
    };
    let tools = enabled_tools
        .iter()
        .map(|spec| DynamicTool::from_portable((spec.build)(&deps)))
        .collect();
    let entity_mode =
        settings::read_context_injection_settings(inputs.settings_pool)?.entity_context_mode;
    let descriptions = enabled_tools
        .iter()
        .map(|spec| context::injection::ToolDescription {
            name: spec.name,
            instruction: spec.instruction,
        })
        .collect::<Vec<_>>();
    let context = inputs
        .turn
        .with(|conn| {
            let entities = if entity_mode == "none" {
                Vec::new()
            } else {
                entity_snapshot(conn, inputs.story_id)?
            };
            context::build_message_context(&context::injection::Inputs {
                conn,
                story_id: inputs.story_id,
                history: &inputs.transcript,
                config: &config,
                entity_mode: &entity_mode,
                entities: &entities,
                tools: &descriptions,
            })
        })
        .await?;
    Ok(Prepared {
        app: inputs.app.clone(),
        turn: inputs.turn,
        story_id: inputs.story_id.to_string(),
        target_entry_id: inputs.target_entry_id,
        turn_id: inputs.turn_id,
        config,
        transcript: inputs.transcript,
        context,
        tools,
        stop_after_tool_result: illustrate,
        reasoning_effort,
        image_requests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn entity_snapshot_reads_uncommitted_attributes_on_turn_connection() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        ).unwrap();
        drop(conn);
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            entities::create_entity_with_id_sync(
                conn,
                "alice",
                "s",
                "character",
                "Alice",
                Some("blue coat"),
                "test",
                None,
                None,
            )?;
            let attribute_id: String = conn.query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Accuracy'",
                [],
                |row| row.get(0),
            )?;
            entities::attributes::set_entity_attribute_sync(
                conn,
                "s",
                "alice",
                &attribute_id,
                7.0,
            )?;
            let snapshot = entity_snapshot(conn, "s")?;
            assert_eq!(snapshot.len(), 1);
            assert_eq!(snapshot[0].id, "alice");
            assert_eq!(snapshot[0].name, "Alice");
            assert_eq!(snapshot[0].appearance_anchor.as_deref(), Some("blue coat"));
            assert_eq!(snapshot[0].attributes.len(), 1);
            assert_eq!(snapshot[0].attributes[0].canonical_name, "Accuracy");
            assert_eq!(snapshot[0].attributes[0].value, 7.0);
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
    }
}
