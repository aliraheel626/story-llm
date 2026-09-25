use std::sync::Arc;

use rig_agent::tool::DynamicTool;
use tauri::AppHandle;
use tokio::sync::Mutex;

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{images, ledger::turn_tx::TurnTx, settings, stories};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::catalog::{self, ToolAvailability, ToolDeps};
use super::injection::{self, ContextPlan};
use super::model::NarratorPurpose;

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
    let context = injection::build_message_context(&injection::Inputs {
        turn: &inputs.turn,
        settings_pool: inputs.settings_pool,
        story_id: inputs.story_id,
        history: &inputs.transcript,
        config: &config,
        tool_specs: &enabled_tools,
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
