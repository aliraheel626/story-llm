use std::sync::Arc;

use rig_agent::tool::DynamicTool;
use tauri::AppHandle;
use tokio::sync::Mutex;

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{images, settings, stories};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::injection::{self, ContextPlan};
use super::model::NarratorPurpose;
use super::staging::TurnStaging;
use super::tools;

pub struct NarratorInputs<'a> {
    pub app: &'a AppHandle,
    pub settings_pool: &'a Pool,
    pub world_pool: &'a Pool,
    pub story_id: &'a str,
    pub transcript: Vec<HistoryTurn>,
    pub before_seq: Option<i64>,
    pub purpose: NarratorPurpose,
}

pub struct Prepared {
    pub(super) app: AppHandle,
    pub(super) world_pool: Pool,
    pub(super) story_id: String,
    pub(super) config: TextModelConfig,
    pub(super) transcript: Vec<HistoryTurn>,
    pub(super) context: ContextPlan,
    pub(super) tools: Vec<DynamicTool>,
    pub(super) stop_after_tool_result: bool,
    pub(super) reasoning_effort: Option<String>,
    pub(super) before_seq: Option<i64>,
    pub(super) staging: Option<Arc<Mutex<TurnStaging>>>,
    pub(super) image_requests: Arc<Mutex<Vec<images::model::ImageRequest>>>,
}

pub fn prepare(inputs: NarratorInputs<'_>) -> AppResult<Prepared> {
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
    let tool_settings =
        stories::settings::read_story_narrator_tools(inputs.settings_pool, inputs.story_id)?;
    let reasoning_effort =
        stories::settings::read_story_reasoning_effort(inputs.settings_pool, inputs.story_id)?;
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
    let (image_tools, image_requests) = tools::narrator_image_tools(image_enabled);
    let world_tools_enabled = !illustrate
        && (tool_settings.get_entities
            || tool_settings.create_entity
            || tool_settings.update_entity
            || tool_settings.adjust_entity_attribute
            || tool_settings.roll_check);
    let staging = if world_tools_enabled || !image_tools.is_empty() {
        Some(Arc::new(Mutex::new(TurnStaging::new(
            inputs.world_pool.clone(),
            inputs.story_id.to_string(),
        ))))
    } else {
        None
    };
    let mut tools = if world_tools_enabled {
        let staging = staging.as_ref().expect("world tools require staging");
        let embedding_api_key =
            settings::read_api_key(inputs.app, "openrouter").unwrap_or_default();
        tools::narrator_tools_for_settings(staging.clone(), embedding_api_key, &tool_settings)
    } else {
        Vec::new()
    };
    tools.extend(image_tools);
    let context = injection::build_message_context(&injection::Inputs {
        pool: inputs.world_pool,
        settings_pool: inputs.settings_pool,
        story_id: inputs.story_id,
        history: &inputs.transcript,
        config: &config,
        tool_settings: if illustrate {
            None
        } else {
            Some(&tool_settings)
        },
        image_enabled,
    })?;
    Ok(Prepared {
        app: inputs.app.clone(),
        world_pool: inputs.world_pool.clone(),
        story_id: inputs.story_id.to_string(),
        config,
        transcript: inputs.transcript,
        context,
        tools,
        stop_after_tool_result: illustrate,
        reasoning_effort,
        before_seq: inputs.before_seq,
        staging,
        image_requests,
    })
}
