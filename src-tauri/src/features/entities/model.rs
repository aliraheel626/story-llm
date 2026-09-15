use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub story_id: String,
    pub branch_id: String,
    pub kind: String,
    pub name: String,
    pub appearance_anchor: Option<String>,
    pub created_at: String,
}
