//! What's left of the old classify/resolve/update pipeline after the
//! narrator became a tool-calling agent (see `narration::tools`): just the
//! dice-mode setting, which now phrases an instruction to the narrator
//! instead of gating a separate deterministic stage.

use serde::{Deserialize, Serialize};

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
