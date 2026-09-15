use serde::{Deserialize, Serialize};

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
    pub branch_id: String,
    pub entity_id: String,
    pub attribute_id: String,
    pub canonical_name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub updated_at: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roll {
    pub id: String,
    pub entry_id: String,
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
