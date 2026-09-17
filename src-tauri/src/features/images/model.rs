use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub description: String,
    pub character_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryImage {
    pub id: String,
    pub entry_id: String,
    pub path: String,
    pub prompt: String,
    pub created_at: String,
}
