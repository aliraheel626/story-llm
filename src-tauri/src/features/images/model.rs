use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryImage {
    pub id: String,
    pub passage_id: String,
    pub path: String,
    pub prompt: String,
    pub seed: Option<i64>,
    pub provider: String,
    pub created_at: String,
}
