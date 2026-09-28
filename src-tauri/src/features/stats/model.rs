use serde::Serialize;

use crate::ai::{CallUsage, TextModelConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Wired into turn recording in S3.
pub enum UsageKind {
    Narration,
    Summary,
    Title,
    Image,
}

#[allow(dead_code)]
impl UsageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Narration => "narration",
            Self::Summary => "summary",
            Self::Title => "title",
            Self::Image => "image",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct UsageRecord {
    pub kind: UsageKind,
    pub provider: String,
    pub model: String,
    pub usage: CallUsage,
}

#[allow(dead_code)]
impl UsageRecord {
    pub fn text(kind: UsageKind, config: &TextModelConfig, usage: CallUsage) -> Self {
        Self {
            kind,
            provider: config.provider.clone(),
            model: config.model.clone(),
            usage,
        }
    }

    pub fn image(model: &str, cost_usd: Option<f64>) -> Self {
        Self {
            kind: UsageKind::Image,
            provider: "openrouter".into(),
            model: model.into(),
            usage: CallUsage {
                cost_usd,
                ..CallUsage::default()
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StoryStats {
    pub text_cost_usd: f64,
    pub image_cost_usd: f64,
    pub total_cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub image_count: i64,
    pub unpriced_calls: i64,
    pub since: Option<String>,
}
