use std::sync::Arc;

use rig_agent::tool::PortableDynamicTool;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::features::images::model::ImageRequest;
use crate::features::ledger::turn_tx::TurnTx;
use crate::features::stories::settings::NarratorToolSettings;
use crate::prompts;

use super::dice;
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

fn turn(deps: &ToolDeps) -> (Arc<TurnTx>, String, String) {
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

fn normal_mode(availability: &ToolAvailability<'_>) -> bool {
    !availability.illustrate
}

fn roll_check_enabled(availability: &ToolAvailability<'_>) -> bool {
    normal_mode(availability) && availability.settings.roll_check
}

fn get_entities_enabled(availability: &ToolAvailability<'_>) -> bool {
    normal_mode(availability) && availability.settings.get_entities
}

fn create_entity_enabled(availability: &ToolAvailability<'_>) -> bool {
    normal_mode(availability) && availability.settings.create_entity
}

fn update_entity_enabled(availability: &ToolAvailability<'_>) -> bool {
    normal_mode(availability) && availability.settings.update_entity
}

fn adjust_entity_attribute_enabled(availability: &ToolAvailability<'_>) -> bool {
    normal_mode(availability) && availability.settings.adjust_entity_attribute
}

fn illustrate_scene_enabled(availability: &ToolAvailability<'_>) -> bool {
    availability.image_enabled
}

fn build_roll_check(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = turn(deps);
    dice::roll_check_tool(turn, target, turn_id)
}

fn build_get_entities(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = turn(deps);
    tools::get_entities_tool(turn, target, turn_id)
}

fn build_create_entity(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = turn(deps);
    tools::create_entity_tool(turn, target, turn_id)
}

fn build_update_entity(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = turn(deps);
    tools::update_entity_tool(turn, target, turn_id)
}

fn build_adjust_entity_attribute(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = turn(deps);
    tools::adjust_entity_attribute_tool(turn, target, turn_id, deps.embedding_api_key.clone())
}

fn build_illustrate_scene(deps: &ToolDeps) -> PortableDynamicTool {
    tools::illustrate_scene_tool(deps.image_requests.clone())
}

pub static TOOLS: [ToolSpec; 6] = [
    ToolSpec {
        name: prompts::ROLL_CHECK_TOOL_NAME,
        instruction: Some(prompts::ROLL_CHECK_AVAILABLE_INSTRUCTION),
        needs_turn: true,
        enabled: roll_check_enabled,
        build: build_roll_check,
        label: dice::roll_check_label,
    },
    ToolSpec {
        name: prompts::GET_ENTITIES_TOOL_NAME,
        instruction: None,
        needs_turn: true,
        enabled: get_entities_enabled,
        build: build_get_entities,
        label: tools::get_entities_label,
    },
    ToolSpec {
        name: prompts::CREATE_ENTITY_TOOL_NAME,
        instruction: None,
        needs_turn: true,
        enabled: create_entity_enabled,
        build: build_create_entity,
        label: tools::create_entity_label,
    },
    ToolSpec {
        name: prompts::UPDATE_ENTITY_TOOL_NAME,
        instruction: None,
        needs_turn: true,
        enabled: update_entity_enabled,
        build: build_update_entity,
        label: tools::update_entity_label,
    },
    ToolSpec {
        name: prompts::ADJUST_ENTITY_ATTRIBUTE_TOOL_NAME,
        instruction: None,
        needs_turn: true,
        enabled: adjust_entity_attribute_enabled,
        build: build_adjust_entity_attribute,
        label: tools::adjust_entity_attribute_label,
    },
    ToolSpec {
        name: prompts::ILLUSTRATE_SCENE_TOOL_NAME,
        instruction: Some(prompts::IMAGE_TOOL_AVAILABLE_INSTRUCTION),
        needs_turn: false,
        enabled: illustrate_scene_enabled,
        build: build_illustrate_scene,
        label: tools::illustrate_scene_label,
    },
];

pub fn enabled(availability: &ToolAvailability<'_>) -> Vec<&'static ToolSpec> {
    TOOLS
        .iter()
        .filter(|spec| (spec.enabled)(availability))
        .collect()
}
