use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::shared::error::{AppError, AppResult};

use super::model::kind;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TranscriptItem {
    pub key: String,
    pub group: String,
    pub label: String,
    pub enabled: bool,
}

const FIXED_ITEMS: &[(&str, &str, &str, bool)] = &[
    ("action.do", "Player actions", "Do", true),
    ("action.say", "Player actions", "Say", true),
    ("action.story", "Player actions", "Story", true),
    ("action.guide", "Player actions", "Guide", true),
    ("action.continue", "Player actions", "Continue", true),
    ("action.see", "Player actions", "See", false),
    ("narration", "Narration", "Narration text", true),
    (
        "narration.thoughts",
        "Narration",
        "Thoughts (reasoning)",
        false,
    ),
];

// These are the only record kinds whose history can be independently toggled.
// content_edited is folded into its target; context_summary is the compaction
// boundary and disabling it loses history; old dice-settings events have never
// been sent to the narrator.
fn record_label(kind: &str) -> (&'static str, bool) {
    match kind {
        kind::DICEROLL => ("Dicerolls", true),
        kind::IMAGE_GENERATED => ("Image prompts", false),
        kind::IMAGE_CAPTIONED => ("Image captions", true),
        kind::TOOL_CALL => ("Tool calls", false),
        kind::ENTITY_CREATED => ("Entity created", true),
        kind::ENTITY_UPDATED => ("Entity updated", true),
        kind::ENTITY_DELETED => ("Entity deleted", true),
        kind::ENTITY_ATTRIBUTE_CHANGED => ("Attribute changed", true),
        kind::ENTITY_ATTRIBUTE_REMOVED => ("Attribute removed", true),
        kind::CONTEXT_NOTE_UPDATED => ("Author's note updates", false),
        _ => ("Unknown record", false),
    }
}

fn catalog() -> Vec<TranscriptItem> {
    let mut items: Vec<_> = FIXED_ITEMS
        .iter()
        .map(|&(key, group, label, enabled)| TranscriptItem {
            key: key.into(),
            group: group.into(),
            label: label.into(),
            enabled,
        })
        .collect();
    items.extend(kind::RECORD_KINDS.iter().map(|kind| {
        let (label, enabled) = record_label(kind);
        TranscriptItem {
            key: format!("record.{kind}"),
            group: "Records".into(),
            label: label.into(),
            enabled,
        }
    }));
    items.push(TranscriptItem {
        key: "images".into(),
        group: "Images".into(),
        label: "The images themselves".into(),
        enabled: false,
    });
    items
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptSettings {
    pub include: BTreeMap<String, bool>,
}

impl Default for TranscriptSettings {
    fn default() -> Self {
        Self {
            include: catalog()
                .into_iter()
                .map(|item| (item.key, item.enabled))
                .collect(),
        }
    }
}

impl TranscriptSettings {
    pub fn includes(&self, key: &str) -> bool {
        self.include.get(key).copied().unwrap_or(false)
    }

    pub fn items(&self) -> Vec<TranscriptItem> {
        catalog()
            .into_iter()
            .map(|mut item| {
                item.enabled = self.includes(&item.key);
                item
            })
            .collect()
    }

    pub fn from_stored(value: Option<&Value>) -> Self {
        let mut settings = Self::default();
        for (key, include) in &mut settings.include {
            if let Some(value) = value
                .and_then(|v| v.get("include"))
                .and_then(|v| v.get(key))
                .and_then(Value::as_bool)
            {
                *include = value;
            }
        }
        settings
    }

    pub fn validate_keys(include: &BTreeMap<String, bool>) -> AppResult<()> {
        let defaults = Self::default();
        for key in include.keys() {
            if !defaults.include.contains_key(key) {
                return Err(AppError::Invalid(format!("unknown context item: {key}")));
            }
        }
        Ok(())
    }
}
