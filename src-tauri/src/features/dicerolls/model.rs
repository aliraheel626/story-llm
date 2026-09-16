use serde::{Deserialize, Serialize};

use crate::features::entities::model::EntityAttributeValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiceMode {
    Always,
    Classifier,
    Never,
}

impl DiceMode {
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::Classifier,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Classifier => "classifier",
            Self::Never => "never",
        }
    }
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
