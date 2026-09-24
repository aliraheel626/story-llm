use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerEntry {
    pub id: String,
    pub story_id: String,
    pub seq: i64,
    pub kind: String,
    pub visibility: String,
    pub content: Option<String>,
    pub payload: Value,
    pub target_entry_id: Option<String>,
    pub turn_id: Option<String>,
    pub created_at: String,
}

impl LedgerEntry {
    #[allow(dead_code)]
    pub fn role(&self) -> &str {
        if self.kind == kind::PLAYER_MESSAGE {
            "player"
        } else {
            "narrator"
        }
    }

    #[cfg(test)]
    pub fn input_mode(&self) -> &str {
        self.payload
            .get("input_mode")
            .and_then(Value::as_str)
            .unwrap_or("generated")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerSnapshot {
    pub visible: Vec<LedgerEntry>,
    pub hidden: Vec<LedgerEntry>,
    pub turns: Vec<TurnSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnSummary {
    pub id: String,
    pub status: String,
}

pub mod kind {
    pub const PLAYER_MESSAGE: &str = "player_message";
    pub const NARRATION: &str = "narration";
    pub const CONTENT_EDITED: &str = "content_edited";
    pub const DICEROLL: &str = "diceroll";
    pub const TOOL_CALL: &str = "tool_call";
    pub const ENTITY_CREATED: &str = "entity_created";
    pub const ENTITY_QUERIED: &str = "entity_queried";
    pub const ENTITY_UPDATED: &str = "entity_updated";
    pub const ENTITY_DELETED: &str = "entity_deleted";
    pub const ENTITY_ATTRIBUTE_CHANGED: &str = "entity_attribute_changed";
    pub const ENTITY_ATTRIBUTE_REMOVED: &str = "entity_attribute_removed";
    pub const IMAGE_GENERATED: &str = "image_generated";
    pub const DICEROLL_SETTINGS_CHANGED: &str = "diceroll_settings_changed";
    pub const CONTEXT_SUMMARY: &str = "context_summary";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn role_and_input_mode_follow_story_entry_defaults() {
        let mut entry = LedgerEntry {
            id: "entry".into(),
            story_id: "story".into(),
            seq: 0,
            kind: kind::PLAYER_MESSAGE.into(),
            visibility: "visible".into(),
            content: Some("action".into()),
            payload: json!({"input_mode":"do"}),
            target_entry_id: None,
            turn_id: None,
            created_at: "now".into(),
        };
        assert_eq!(entry.role(), "player");
        assert_eq!(entry.input_mode(), "do");

        entry.kind = kind::NARRATION.into();
        entry.payload = json!({"input_mode":42});
        assert_eq!(entry.role(), "narrator");
        assert_eq!(entry.input_mode(), "generated");
        entry.payload = json!({});
        assert_eq!(entry.input_mode(), "generated");
    }
}
