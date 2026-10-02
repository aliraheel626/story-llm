use std::sync::Arc;

use rig_agent::tool::PortableDynamicTool;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::features::images::model::ImageRequest;
use crate::features::stories::settings::NarratorToolSettings;
use crate::features::turn::TurnTx;

use super::tools;

pub struct ToolAvailability<'a> {
    pub settings: &'a NarratorToolSettings,
    pub image_enabled: bool,
    pub illustrate: bool,
}

pub struct ToolDeps {
    pub turn: Option<Arc<TurnTx>>,
    pub target_entry_id: Option<String>,
    pub turn_id: Option<String>,
    pub embedding_api_key: String,
    pub image_requests: Arc<Mutex<Vec<ImageRequest>>>,
}

pub struct ToolSpec {
    pub name: &'static str,
    pub instruction: Option<&'static str>,
    pub needs_turn: bool,
    pub enabled: for<'a> fn(&ToolAvailability<'a>) -> bool,
    pub build: fn(&ToolDeps) -> PortableDynamicTool,
    pub label: fn(&Value) -> String,
}

pub(super) fn turn(deps: &ToolDeps) -> (Arc<TurnTx>, String, String) {
    (
        deps.turn
            .as_ref()
            .expect("narrator tool requires turn transaction")
            .clone(),
        deps.target_entry_id
            .as_ref()
            .expect("narrator tool requires target entry")
            .clone(),
        deps.turn_id
            .as_ref()
            .expect("narrator tool requires turn id")
            .clone(),
    )
}

pub(super) fn normal_mode(availability: &ToolAvailability<'_>) -> bool {
    !availability.illustrate
}

pub static TOOLS: &[ToolSpec] = &[
    tools::roll_check::SPEC,
    tools::save_character::SPEC,
    tools::save_relationship::SPEC,
    tools::illustrate_scene::SPEC,
];

pub fn enabled(availability: &ToolAvailability<'_>) -> Vec<&'static ToolSpec> {
    TOOLS
        .iter()
        .filter(|spec| (spec.enabled)(availability))
        .collect()
}
