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
    let context_window = match provider.as_str() {
        "nous_portal" => fetch_context_window(
            &format!("{}/models", crate::ai::NOUS_PORTAL_BASE_URL),
            ("data", "id"),
            &model,
            Some(crate::ai::NOUS_PORTAL_USER_AGENT),
        )
        .await
        .unwrap_or(NOUS_PORTAL_DEFAULT_CONTEXT_WINDOW),
        // Ollama serves a model with the context it was loaded with, not the
        // model's maximum, and silently truncates longer prompts. `/api/ps`
        // reports that loaded size, but only while the model is loaded.
        "ollama" => fetch_context_window(
            &format!(
                "{}/api/ps",
                crate::ai::OLLAMA_BASE_URL.trim_end_matches("/v1")
            ),
            ("models", "name"),
            &model,
            None,
        )
        .await
        .unwrap_or(32_768),
        _ => fetch_context_window(
            "https://openrouter.ai/api/v1/models",
            ("data", "id"),
            &model,
            None,
        )
        .await
        .unwrap_or(32_768),
    };
    let pool = pool.clone();
    let provider_for_write = provider.clone();
    blocking(move || {
        repository::write_text_model_settings(&pool, &provider_for_write, &model, context_window)
    })
    .await?;

    if let Some(key) = api_key {
        secrets::write_api_key(app, &provider, &key)?;
    }

    Ok(())
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
        .and_then(|item| item.get("context_length"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .ok_or_else(|| AppError::NotFound(format!("context metadata for model {model} not found")))
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
    })
}
