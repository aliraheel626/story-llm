use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelineEntry {
    pub id: String,
    pub story_id: String,
    pub seq: i64,
    pub kind: String,
    pub visibility: String,
    pub content: Option<String>,
    pub payload: Value,
    pub target_entry_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineSnapshot {
    pub visible: Vec<TimelineEntry>,
    pub hidden: Vec<TimelineEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrationVariant {
    pub id: String,
    pub entry_id: String,
    pub content: String,
    pub is_selected: bool,
    pub created_at: String,
    /// That revision's own reasoning, for the composer's activity panel.
    /// Display only — never fed back to the model as context.
    pub thoughts: Option<String>,
}

pub mod kind {
    pub const PLAYER_MESSAGE: &str = "player_message";
    pub const NARRATION: &str = "narration";
    pub const NARRATION_VARIANT: &str = "narration_variant";
    pub const NARRATION_SELECTED: &str = "narration_selected";
    pub const CONTENT_EDITED: &str = "content_edited";
    pub const DICEROLL: &str = "diceroll";
    pub const ENTITY_CREATED: &str = "entity_created";
    pub const ENTITY_QUERIED: &str = "entity_queried";
    pub const ENTITY_UPDATED: &str = "entity_updated";
    pub const ENTITY_DELETED: &str = "entity_deleted";
    pub const ENTITY_ATTRIBUTE_CHANGED: &str = "entity_attribute_changed";
    pub const ENTITY_ATTRIBUTE_REMOVED: &str = "entity_attribute_removed";
    pub const IMAGE_GENERATED: &str = "image_generated";
    pub const CONTEXT_NOTE_UPDATED: &str = "context_note_updated";
    pub const DICEROLL_SETTINGS_CHANGED: &str = "diceroll_settings_changed";
    pub const CONTEXT_SUMMARY: &str = "context_summary";
}
