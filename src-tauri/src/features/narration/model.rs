use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveStoryEntry {
    pub id: String,
    pub story_id: String,
    pub seq: i64,
    pub role: String,
    pub input_mode: String,
    pub content: String,
    pub thoughts: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
}
