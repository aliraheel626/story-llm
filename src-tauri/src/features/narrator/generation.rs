use std::sync::Arc;

use rig_agent::tool::DynamicTool;
use tauri::AppHandle;
use tokio::sync::Mutex;

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{images, settings, stories};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::catalog::{self, ToolAvailability, ToolDeps};
use super::injection::{self, ContextPlan};
use super::model::NarratorPurpose;
use super::staging::TurnStaging;

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
    let enabled_tools = catalog::enabled(&ToolAvailability {
        settings: &tool_settings,
        image_enabled,
        illustrate,
    });
    let world_tools_enabled = enabled_tools.iter().any(|spec| spec.needs_staging);
    let staging = if enabled_tools.is_empty() {
        None
    } else {
        Some(Arc::new(Mutex::new(TurnStaging::new(
            inputs.world_pool.clone(),
            inputs.story_id.to_string(),
        ))))
    };
    let embedding_api_key = if world_tools_enabled {
        settings::read_api_key(inputs.app, "openrouter").unwrap_or_default()
    } else {
        String::new()
    };
    let image_requests = Arc::new(Mutex::new(Vec::new()));
    let deps = ToolDeps {
        staging: staging.clone(),
        embedding_api_key,
        image_requests: image_requests.clone(),
    };
    let tools = enabled_tools
        .iter()
        .map(|spec| DynamicTool::from_portable((spec.build)(&deps)))
        .collect();
    let context = injection::build_message_context(&injection::Inputs {
        pool: inputs.world_pool,
        settings_pool: inputs.settings_pool,
        story_id: inputs.story_id,
        history: &inputs.transcript,
        config: &config,
        tool_specs: &enabled_tools,
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
