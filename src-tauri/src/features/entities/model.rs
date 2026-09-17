use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub story_id: String,
    pub kind: String,
    pub name: String,
    pub appearance_anchor: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeRegistryEntry {
    pub id: String,
    pub canonical_name: String,
    pub aliases_json: String,
    pub entity_kinds_json: String,
    pub min: f64,
    pub max: f64,
    pub category: String,
    pub is_user_created: bool,
    pub created_in_story_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAttributeValue {
    pub story_id: String,
    pub entity_id: String,
    pub attribute_id: String,
    pub canonical_name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub updated_at: String,
    pub source: String,
}
