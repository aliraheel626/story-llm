use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextModelSettings {
    pub provider: String,
    pub model: String,
    pub has_api_key: bool,
    pub context_window: usize,
    #[serde(default)]
    pub supports_images: bool,
}

pub const DEFAULT_IMAGE_STYLE: &str = "Digital painting, atmospheric scene illustration.";
pub const DEFAULT_TEXT_PROVIDER: &str = "openrouter";
pub const DEFAULT_TEXT_MODEL: &str = "x-ai/grok-4.7";
pub const DEFAULT_TEXT_CONTEXT_WINDOW: usize = 500_000;
pub const DEFAULT_TEXT_SUPPORTS_IMAGES: bool = true;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageModelSettings {
    pub model: String,
    pub enabled: bool,
    /// A style prefix folded into every generated prompt (e.g. "anime",
    /// "photorealistic") — see `commands::images::compose_image_prompt`.
    pub style: String,
    /// Whether each generated image gets a caption.
    pub captions_enabled: bool,
    /// Vision model that writes the captions.
    pub caption_model: String,
    /// Images reuse the OpenRouter key set in the Text Model panel — there is
    /// only one provider (OpenRouter) for both text and images.
    pub has_api_key: bool,
}
