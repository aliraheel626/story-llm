use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Story {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub settings_json: String,
    pub default_branch_id: Option<String>,
}
