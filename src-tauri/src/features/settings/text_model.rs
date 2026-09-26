use tauri::AppHandle;

use crate::ai::TextModelConfig;
use crate::shared::db::{blocking, Pool};
use crate::shared::error::{AppError, AppResult};

use super::{repository, secrets};

pub(super) async fn save_text_model_settings(
    app: &AppHandle,
    pool: &Pool,
    provider: String,
    model: String,
    api_key: Option<String>,
) -> AppResult<()> {
    if !matches!(provider.as_str(), "openrouter" | "nous_portal" | "ollama") {
        return Err(AppError::Invalid(format!(
            "unsupported text model provider: {provider}"
        )));
    }
    let (context_window, supports_images) = model_capabilities(&provider, &model).await;
    let pool = pool.clone();
    let provider_for_write = provider.clone();
    blocking(move || {
        repository::write_text_model_settings(
            &pool,
            &provider_for_write,
            &model,
            context_window,
            supports_images,
        )
    })
    .await?;

    if let Some(key) = api_key {
        secrets::write_api_key(app, &provider, &key)?;
    }

    Ok(())
}

/// The served context window and whether the model accepts image input.
async fn model_capabilities(provider: &str, model: &str) -> (usize, bool) {
    let (window, support) = model_capabilities_with_status(provider, model).await;
    (window, support.unwrap_or(false))
}

/// `None` means the capability lookup failed, not that the model is text-only.
async fn model_capabilities_with_status(provider: &str, model: &str) -> (usize, Option<bool>) {
    match provider {
        "nous_portal" => (
            fetch_context_window(
                &format!("{}/models", crate::ai::NOUS_PORTAL_BASE_URL),
                ("data", "id"),
                model,
                Some(crate::ai::NOUS_PORTAL_USER_AGENT),
            )
            .await
            .unwrap_or(NOUS_PORTAL_DEFAULT_CONTEXT_WINDOW),
            Some(false),
        ),
        // Ollama serves a model with the context it was loaded with, not the
        // model's maximum, and silently truncates longer prompts. `/api/ps`
        // reports that loaded size, but only while the model is loaded.
        "ollama" => {
            let base = crate::ai::OLLAMA_BASE_URL.trim_end_matches("/v1");
            let window =
                fetch_context_window(&format!("{base}/api/ps"), ("models", "name"), model, None)
                    .await
                    .unwrap_or(32_768);
            let supports_images = reqwest::Client::new()
                .post(format!("{base}/api/show"))
                .json(&serde_json::json!({"model":model}))
                .send()
                .await
                .ok()
                .and_then(|response| response.error_for_status().ok());
            let supports_images = match supports_images {
                Some(response) => response
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|value| ollama_image_support(&value)),
                None => None,
            };
            (window, supports_images)
        }
        _ => {
            let item = fetch_model(
                "https://openrouter.ai/api/v1/models",
                ("data", "id"),
                model,
                None,
            )
            .await
            .ok();
            let window = item
                .as_ref()
                .and_then(|value| value.get("context_length"))
                .and_then(|value| value.as_u64())
                .map(|n| n as usize)
                .unwrap_or(32_768);
            let supports_images = item.as_ref().and_then(openrouter_image_support);
            (window, supports_images)
        }
    }
}

fn confirmed_image_support(support: Option<bool>) -> AppResult<bool> {
    support.ok_or_else(|| AppError::Other("model image capability metadata unavailable".into()))
}

/// Settings saved before image support was recorded have no `supports_images`.
/// Look it up once in the background without delaying app startup.
pub async fn refresh_missing_capabilities(pool: Pool) -> bool {
    let result: AppResult<bool> = async {
        let read_pool = pool.clone();
        let stored = blocking(move || repository::stored_text_model_row(&read_pool)).await?;
        let Some(stored) = stored.filter(repository::needs_capability_refresh) else {
            return Ok(false);
        };
        let provider = stored
            .get("provider")
            .and_then(|value| value.as_str())
            .unwrap_or("openrouter")
            .to_string();
        let model = stored
            .get("model")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if model.is_empty() {
            return Ok(false);
        }
        let (_, support) = model_capabilities_with_status(&provider, &model).await;
        let supports_images = confirmed_image_support(support)?;
        blocking(move || repository::write_missing_image_support(&pool, &stored, supports_images))
            .await
    }
    .await;
    match result {
        Ok(updated) => updated,
        Err(error) => {
            log::warn!("could not refresh saved text model image support: {error}");
            false
        }
    }
}

/// Best-effort, fetched once at save time and persisted — not a live cache.
/// A failed fetch (or a model id that doesn't exactly match the provider's
/// listing) is stored as the same fallback as a real value, so re-saving the
/// model is the only way to pick up a corrected number. `list_field` names the
/// response's model list and `id_field` the id within each listed model.
async fn fetch_context_window(
    models_url: &str,
    (list_field, id_field): (&str, &str),
    model: &str,
    user_agent: Option<&str>,
) -> AppResult<usize> {
    fetch_model(models_url, (list_field, id_field), model, user_agent)
        .await?
        .get("context_length")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .ok_or_else(|| AppError::NotFound(format!("context metadata for model {model} not found")))
}

fn openrouter_image_support(value: &serde_json::Value) -> Option<bool> {
    let modalities = value
        .pointer("/architecture/input_modalities")
        .and_then(|v| v.as_array())
        ?;
    if modalities.iter().any(|v| !v.is_string()) {
        return None;
    }
    Some(modalities.iter().any(|v| v.as_str() == Some("image")))
}

fn ollama_image_support(value: &serde_json::Value) -> Option<bool> {
    let capabilities = value
        .get("capabilities")
        .and_then(|v| v.as_array())
        ?;
    if capabilities.iter().any(|v| !v.is_string()) {
        return None;
    }
    Some(capabilities.iter().any(|v| v.as_str() == Some("vision")))
}

async fn fetch_model(
    models_url: &str,
    (list_field, id_field): (&str, &str),
    model: &str,
    user_agent: Option<&str>,
) -> AppResult<serde_json::Value> {
    let mut request = reqwest::Client::new().get(models_url);
    if let Some(user_agent) = user_agent {
        request = request.header(reqwest::header::USER_AGENT, user_agent);
    }
    let response: serde_json::Value = request
        .send()
        .await
        .map_err(|e| AppError::Other(format!("model metadata request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Other(format!("model metadata request failed: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Other(format!("invalid model metadata: {e}")))?;
    response
        .get(list_field)
        .and_then(|v| v.as_array())
        .and_then(|models| {
            models
                .iter()
                .find(|item| item.get(id_field).and_then(|v| v.as_str()) == Some(model))
        })
        .cloned()
        .ok_or_else(|| AppError::NotFound(format!("metadata for model {model} not found")))
}

/// Hermes 4 (70B and 405B) both document a 131,072-token context window;
/// used when Nous Portal's own listing doesn't carry a `context_length`
/// field for the model (unlike OpenRouter, Nous Portal's `/v1/models` is not
/// guaranteed to include one — it's plain OpenAI-compatible, and real
/// OpenAI's own listing omits this field too).
const NOUS_PORTAL_DEFAULT_CONTEXT_WINDOW: usize = 131_072;

/// Resolves persisted settings and the provider secret into the configuration
/// consumed by the shared text-AI gateway.
pub fn resolve_text_model(app: &AppHandle, pool: &Pool) -> AppResult<TextModelConfig> {
    let settings = repository::read_text_model_settings(app, pool)?;
    if settings.model.trim().is_empty() {
        return Err(AppError::Invalid(
            "no text model configured yet — set one in the Text Model panel".into(),
        ));
    }
    let api_key = if settings.provider == "ollama" {
        crate::ai::OLLAMA_API_KEY.to_string()
    } else {
        secrets::read_api_key(app, &settings.provider)?
    };
    Ok(TextModelConfig {
        provider: settings.provider,
        model: settings.model,
        api_key,
        context_window: settings.context_window,
        supports_images: settings.supports_images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_metadata_parses_independently_of_context_length() {
        assert_eq!(openrouter_image_support(&json!({"architecture":{"input_modalities":["text","image"]}})), Some(true));
        assert_eq!(openrouter_image_support(&json!({"architecture":{"input_modalities":["text"]},"context_length":8192})), Some(false));
        assert_eq!(openrouter_image_support(&json!({"architecture":{"input_modalities":"image"}})), None);
        assert_eq!(openrouter_image_support(&json!({"architecture":{}})), None);
        assert_eq!(openrouter_image_support(&json!({"architecture":{"input_modalities":["text",7]}})), None);
        assert_eq!(ollama_image_support(&json!({"capabilities":["completion","vision"]})), Some(true));
        assert_eq!(ollama_image_support(&json!({"capabilities":[]})), Some(false));
        assert_eq!(ollama_image_support(&json!({"capabilities":null})), None);
        assert_eq!(ollama_image_support(&json!({})), None);
        assert_eq!(ollama_image_support(&json!({"capabilities":["completion",7]})), None);
    }

    #[test]
    fn unavailable_metadata_does_not_become_text_only() {
        assert!(confirmed_image_support(None).is_err());
        assert!(confirmed_image_support(openrouter_image_support(&json!({"architecture":{}}))).is_err());
        assert!(confirmed_image_support(ollama_image_support(&json!({}))).is_err());
        assert!(!confirmed_image_support(Some(false)).unwrap());
        assert!(confirmed_image_support(Some(true)).unwrap());
    }
}
