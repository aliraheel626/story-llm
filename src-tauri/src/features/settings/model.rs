use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextModelSettings {
    pub provider: String,
    pub model: String,
    pub has_api_key: bool,
    pub context_window: usize,
}

pub const DEFAULT_IMAGE_STYLE: &str = "Digital painting, atmospheric scene illustration.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageModelSettings {
    pub model: String,
    pub enabled: bool,
    /// A style prefix folded into every generated prompt (e.g. "anime",
    /// "photorealistic") — see `commands::images::compose_image_prompt`.
    pub style: String,
    /// Images reuse the OpenRouter key set in the Text Model panel — there is
    /// only one provider (OpenRouter) for both text and images.
    pub has_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInjectionSettings {
    pub entity_context_mode: String,
}

impl Default for ContextInjectionSettings {
    fn default() -> Self {
        Self {
            entity_context_mode: "all".to_string(),
        }
    }
}
