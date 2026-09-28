//! OpenRouter's dedicated Image API (`POST /api/v1/images`). Rig has no
//! `image_generation` support for the OpenRouter provider (verified against
//! its source — only `openai` has that submodule), so this is hand-rolled
//! directly on reqwest rather than forced through a mismatched Rig trait.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::shared::error::{AppError, AppResult};

const IMAGES_ENDPOINT: &str = "https://openrouter.ai/api/v1/images";
pub const DEFAULT_IMAGE_MODEL: &str = "google/gemini-3.1-flash-image-preview";

pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
    pub cost_usd: Option<f64>,
}

/// No `resolution` is sent: the model's own default is its lowest supported
/// tier on every model we probed (Gemini and Grok bottom out at 1K, FLUX
/// renders 1024² regardless), and it is also the fastest.
#[derive(Serialize)]
struct ImageRequestBody<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct ImageResponseBody {
    data: Option<Vec<ImageDataEntry>>,
    #[serde(default)]
    usage: Option<ImageUsage>,
    #[serde(default)]
    error: Option<ImageResponseError>,
}

#[derive(Deserialize)]
struct ImageUsage {
    #[serde(default)]
    cost: Option<f64>,
}

#[derive(Deserialize)]
struct ImageResponseError {
    message: String,
}

#[derive(Deserialize)]
struct ImageDataEntry {
    b64_json: Option<String>,
    media_type: Option<String>,
}

pub async fn generate_image(api_key: &str, model: &str, prompt: &str) -> AppResult<GeneratedImage> {
    let client = reqwest::Client::new();
    let res = client
        .post(IMAGES_ENDPOINT)
        .bearer_auth(api_key)
        .json(&ImageRequestBody { model, prompt })
        .send()
        .await
        .map_err(|e| AppError::Other(format!("image request failed: {e}")))?;

    let status = res.status();
    let body: ImageResponseBody = res
        .json()
        .await
        .map_err(|e| AppError::Other(format!("image response was not valid JSON: {e}")))?;

    if !status.is_success() {
        let message = body
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(AppError::Other(format!(
            "image generation failed: {message}"
        )));
    }

    let entry = body
        .data
        .and_then(|mut d| {
            if d.is_empty() {
                None
            } else {
                Some(d.remove(0))
            }
        })
        .ok_or_else(|| AppError::Other("image generation returned no images".into()))?;

    let b64 = entry
        .b64_json
        .ok_or_else(|| AppError::Other("image generation response had no image data".into()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| AppError::Other(format!("failed to decode image data: {e}")))?;
    let media_type = entry.media_type.unwrap_or_else(|| "image/png".to_string());

    Ok(GeneratedImage {
        bytes,
        media_type,
        cost_usd: body.usage.and_then(|usage| usage.cost),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_response_cost_is_optional() {
        let with_usage: ImageResponseBody = serde_json::from_value(serde_json::json!({
            "created":1,"data":[{"b64_json":"iVBORw==","media_type":"image/png"}],
            "usage":{"prompt_tokens":10,"completion_tokens":1290,"total_tokens":1300,"cost":0.039}
        }))
        .unwrap();
        assert_eq!(with_usage.usage.and_then(|usage| usage.cost), Some(0.039));
        let without_usage: ImageResponseBody = serde_json::from_value(serde_json::json!({
            "created":1,"data":[{"b64_json":"iVBORw==","media_type":"image/png"}]
        }))
        .unwrap();
        assert!(without_usage.usage.is_none());
    }
}
