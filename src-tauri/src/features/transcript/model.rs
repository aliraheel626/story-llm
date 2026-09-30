use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryImage {
    pub id: String,
    pub entry_id: String,
    pub prompt: String,
    #[serde(default)]
    pub caption: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptEntry {
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

impl TranscriptEntry {
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
pub struct TranscriptSnapshot {
    pub visible: Vec<TranscriptEntry>,
    pub hidden: Vec<TranscriptEntry>,
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
    pub const IMAGE_CAPTIONED: &str = "image_captioned";
    pub const CONTEXT_SUMMARY: &str = "context_summary";
    pub const CONTEXT_NOTE_UPDATED: &str = "context_note_updated";

    // CONTENT_EDITED is applied to its target, CONTEXT_SUMMARY is the compaction
    // boundary; the old dice-settings event is not a transcript event.
    pub const RECORD_KINDS: &[&str] = &[
        DICEROLL,
        IMAGE_GENERATED,
        IMAGE_CAPTIONED,
        TOOL_CALL,
        ENTITY_CREATED,
        ENTITY_QUERIED,
        ENTITY_UPDATED,
        ENTITY_DELETED,
        ENTITY_ATTRIBUTE_CHANGED,
        ENTITY_ATTRIBUTE_REMOVED,
        CONTEXT_NOTE_UPDATED,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn story_image_caption_defaults_to_none_and_round_trips() {
        let mut value = json!({"id":"asset","entry_id":"entry","prompt":"scene","created_at":"now"});
        let image: StoryImage = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(image.caption, None);

        value["caption"] = json!("A lantern lights the stone corridor.");
        let image: StoryImage = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            image.caption.as_deref(),
            Some("A lantern lights the stone corridor.")
        );
        assert_eq!(serde_json::to_value(image).unwrap(), value);
    }

    #[test]
    fn role_and_input_mode_follow_story_entry_defaults() {
        let mut entry = TranscriptEntry {
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
