use serde::{Deserialize, Serialize};

use crate::shared::error::{AppError, AppResult};

pub const CHARACTER: &str = "character";
pub const RELATIONSHIP: &str = "relationship";

pub fn relationship_id(from_id: &str, to_id: &str) -> String {
    format!("relationship:{from_id}:{to_id}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityLink {
    pub from_id: String,
    pub to_id: String,
    pub label: String,
    pub direction: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterFields {
    pub known_as: Option<String>,
    pub appearance_anchor: Option<String>,
    pub gender: Option<String>,
    pub age: Option<String>,
    pub role: Option<String>,
    pub location: Option<String>,
    pub outfit: Option<String>,
}

impl CharacterFields {
    pub fn fields(&self) -> [(&'static str, &Option<String>); 7] {
        [
            ("known_as", &self.known_as),
            ("appearance_anchor", &self.appearance_anchor),
            ("gender", &self.gender),
            ("age", &self.age),
            ("role", &self.role),
            ("location", &self.location),
            ("outfit", &self.outfit),
        ]
    }

    pub fn normalize(mut self) -> AppResult<Self> {
        for (field, value) in [
            ("known_as", &mut self.known_as),
            ("appearance_anchor", &mut self.appearance_anchor),
            ("gender", &mut self.gender),
            ("age", &mut self.age),
            ("role", &mut self.role),
            ("location", &mut self.location),
            ("outfit", &mut self.outfit),
        ] {
            normalize_character_field(field, value)?;
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterPatch {
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub known_as: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub appearance_anchor: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub gender: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub age: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub role: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub location: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_character_patch_field", skip_serializing_if = "Option::is_none")]
    pub outfit: Option<Option<String>>,
}

impl CharacterPatch {
    pub fn normalize(mut self) -> AppResult<Self> {
        for (field, value) in [
            ("known_as", &mut self.known_as),
            ("appearance_anchor", &mut self.appearance_anchor),
            ("gender", &mut self.gender),
            ("age", &mut self.age),
            ("role", &mut self.role),
            ("location", &mut self.location),
            ("outfit", &mut self.outfit),
        ] {
            if let Some(value) = value {
                normalize_character_field(field, value)?;
            }
        }
        Ok(self)
    }

    pub fn apply_to(&self, before: &CharacterFields) -> CharacterFields {
        CharacterFields {
            known_as: self.known_as.clone().unwrap_or_else(|| before.known_as.clone()),
            appearance_anchor: self.appearance_anchor.clone().unwrap_or_else(|| before.appearance_anchor.clone()),
            gender: self.gender.clone().unwrap_or_else(|| before.gender.clone()),
            age: self.age.clone().unwrap_or_else(|| before.age.clone()),
            role: self.role.clone().unwrap_or_else(|| before.role.clone()),
            location: self.location.clone().unwrap_or_else(|| before.location.clone()),
            outfit: self.outfit.clone().unwrap_or_else(|| before.outfit.clone()),
        }
    }
}

fn normalize_character_field(field: &'static str, value: &mut Option<String>) -> AppResult<()> {
    let trimmed = value.as_deref().map(str::trim).filter(|value| !value.is_empty());
    if field != "appearance_anchor" && trimmed.is_some_and(|value| value.chars().count() > 200) {
        return Err(AppError::Invalid(format!("{field} must be at most 200 characters")));
    }
    *value = trimmed.map(str::to_string);
    Ok(())
}

fn deserialize_character_patch_field<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // The field default handles absence; a present null is an explicit clear.
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub story_id: String,
    pub kind: String,
    pub name: String,
    #[serde(flatten)]
    pub character: CharacterFields,
    #[serde(default)]
    pub link: Option<EntityLink>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn character_patch_json_preserves_missing_null_and_string_values() {
        let absent: CharacterPatch = serde_json::from_value(json!({})).unwrap();
        assert_eq!(absent.age, None);
        assert_eq!(absent.known_as, None);
        assert_eq!(serde_json::to_value(&absent).unwrap(), json!({}));
        let cleared: CharacterPatch = serde_json::from_value(json!({"age":null, "known_as":null})).unwrap();
        assert_eq!(cleared.age, Some(None));
        assert_eq!(cleared.known_as, Some(None));
        assert_eq!(cleared.role, None);
        let encoded = serde_json::to_value(&cleared).unwrap();
        assert_eq!(encoded, json!({"age":null, "known_as":null}));
        assert_eq!(serde_json::from_value::<CharacterPatch>(encoded).unwrap(), cleared);
        let changed: CharacterPatch = serde_json::from_value(json!({"age":"34", "known_as":"the hooded stranger"})).unwrap();
        assert_eq!(changed.age, Some(Some("34".into())));
        assert_eq!(changed.known_as, Some(Some("the hooded stranger".into())));
        let before = CharacterFields {
            age: Some("33".into()), known_as: Some("the traveler".into()),
            role: Some("smuggler".into()), ..Default::default()
        };
        assert_eq!(absent.apply_to(&before), before);
        let after = cleared.apply_to(&before);
        assert_eq!(after.age, None);
        assert_eq!(after.known_as, None);
        assert_eq!(after.role, before.role);
        assert!(serde_json::from_value::<CharacterPatch>(json!({"age":34})).is_err());
        let empty = CharacterFields::default();
        let serialized = serde_json::to_value(&empty).unwrap();
        for (field, _) in empty.fields() {
            assert!(serialized.get(field).unwrap().is_null());
        }
    }

    #[test]
    fn character_normalization_uses_scalar_limits_and_preserves_patch_clears() {
        let empty = CharacterFields::default();
        let at_limit = "\u{00e9}".repeat(200);
        let over_limit = "\u{00e9}".repeat(201);
        for (field, _) in empty.fields() {
            let patch: CharacterPatch = serde_json::from_value(json!({(field): format!(" {at_limit} ")})).unwrap();
            let normalized = patch.normalize().unwrap().apply_to(&empty);
            assert_eq!(normalized.fields().into_iter().find(|(name, _)| *name == field).unwrap().1.as_deref(), Some(at_limit.as_str()));
            let patch: CharacterPatch = serde_json::from_value(json!({(field): over_limit.as_str()})).unwrap();
            let fields: CharacterFields = serde_json::from_value(json!({(field): over_limit.as_str()})).unwrap();
            if field == "appearance_anchor" {
                assert!(patch.normalize().is_ok());
                assert!(fields.normalize().is_ok());
            } else {
                assert!(matches!(patch.normalize(), Err(AppError::Invalid(message)) if message.contains(field)));
                assert!(matches!(fields.normalize(), Err(AppError::Invalid(message)) if message.contains(field)));
            }
        }
        let patch: CharacterPatch = serde_json::from_value(json!({"known_as":" ", "age":null, "role":" innkeeper "})).unwrap();
        let normalized = patch.normalize().unwrap();
        assert_eq!(normalized.known_as, Some(None));
        assert_eq!(normalized.age, Some(None));
        assert_eq!(normalized.gender, None);
        assert_eq!(normalized.role, Some(Some("innkeeper".into())));
        assert_eq!(CharacterFields { known_as: Some(" ".into()), role: Some(" innkeeper ".into()), ..Default::default() }.normalize().unwrap(),
            CharacterFields { role: Some("innkeeper".into()), ..Default::default() });
    }
}
