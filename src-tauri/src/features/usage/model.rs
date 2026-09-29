use serde::Serialize;

use crate::ai::{CallUsage, TextModelConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind {
    Narration,
    Summary,
    Title,
    Image,
}

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
pub struct UsageRecord {
    pub kind: UsageKind,
    pub provider: String,
    pub model: String,
    pub usage: CallUsage,
    pub turn_id: Option<String>,
    pub image_asset_id: Option<String>,
    pub duration_ms: Option<u64>,
}

impl UsageRecord {
    pub fn text(kind: UsageKind, config: &TextModelConfig, usage: CallUsage) -> Self {
        Self {
            kind,
            provider: config.provider.clone(),
            model: config.model.clone(),
            usage,
            turn_id: None,
            image_asset_id: None,
            duration_ms: None,
        }
    }

    pub fn image(model: &str, cost_usd: Option<f64>, asset_id: Option<String>, duration_ms: Option<u64>) -> Self {
        Self {
            kind: UsageKind::Image,
            provider: "openrouter".into(),
            model: model.into(),
            usage: CallUsage {
                cost_usd,
                ..CallUsage::default()
            },
            turn_id: None,
            image_asset_id: asset_id,
            duration_ms,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StoryUsage {
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

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TurnCost {
    pub turn_id: String,
    pub text_cost_usd: f64,
    pub image_cost_usd: f64,
    pub total_cost_usd: f64,
    pub earlier_attempts_cost_usd: f64,
    pub unpriced_calls: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageCost {
    pub asset_id: String,
    pub turn_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StoryCostBreakdown {
    pub turns: Vec<TurnCost>,
    pub images: Vec<ImageCost>,
}
