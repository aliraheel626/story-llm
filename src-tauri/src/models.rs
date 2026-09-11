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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub story_id: String,
    pub kind: String,
    pub name: String,
    pub card_json: String,
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
    pub entity_id: String,
    pub attribute_id: String,
    pub canonical_name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roll {
    pub id: String,
    pub passage_id: String,
    pub actor_entity_id: String,
    pub target_entity_id: Option<String>,
    pub actor_attribute_id: Option<String>,
    pub target_attribute_id: Option<String>,
    pub actor_value: Option<f64>,
    pub target_value: Option<f64>,
    pub p_success: f64,
    pub seed: i64,
    pub roll: i64,
    pub outcome: String,
    pub degree: String,
    pub modifiers_json: String,
    pub created_at: String,
}

/// The roll enriched with names and a broader attribute snapshot of both
/// entities involved — what the per-passage "why" disclosure renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollDetail {
    pub roll: Roll,
    pub actor_name: String,
    pub actor_attribute_name: Option<String>,
    pub target_name: Option<String>,
    pub target_attribute_name: Option<String>,
    pub actor_attributes: Vec<EntityAttributeValue>,
    pub target_attributes: Vec<EntityAttributeValue>,
}

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
