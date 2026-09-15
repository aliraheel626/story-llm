use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Passage {
    pub id: String,
    pub branch_id: String,
    pub seq: i64,
    pub role: String,
    pub input_mode: String,
    pub content: String,
    pub thoughts: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageVariant {
    pub id: String,
    pub passage_id: String,
    pub content: String,
    pub is_selected: bool,
    pub created_at: String,
}
