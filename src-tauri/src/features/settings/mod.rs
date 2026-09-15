use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, State};
use tauri_plugin_store::StoreExt;

use crate::ai::{TextModelConfig, TextProviderKind};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

const SECRETS_STORE: &str = "secrets.json";
const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextModelSettings {
    pub provider: String,
    pub model: String,
    pub has_api_key: bool,
    pub context_window: usize,
}

fn api_key_store_key(provider: &str) -> String {
    format!("text_model.{provider}.api_key")
}

#[tauri::command]
pub fn get_text_model_settings(app: AppHandle, pool: State<Pool>) -> AppResult<TextModelSettings> {
    read_text_model_settings(&app, pool.inner())
}

/// Plain-`&Pool` variant of `get_text_model_settings` for callers that aren't
/// Tauri commands (e.g. the narrator's config resolution and the auto-titler).
pub fn read_text_model_settings(app: &AppHandle, pool: &Pool) -> AppResult<TextModelSettings> {
    let conn = pool.get()?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_TEXT_MODEL],
            |row| row.get(0),
        )
        .ok();

    let (provider, model, context_window) = stored
        .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
        .map(|v| {
            let provider = v
                .get("provider")
                .and_then(|p| p.as_str())
                .unwrap_or("openrouter")
                .to_string();
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let context_window = v
                .get("context_window")
                .and_then(|n| n.as_u64())
                .unwrap_or(32_768) as usize;
            (provider, model, context_window)
        })
        .unwrap_or_else(|| ("openrouter".to_string(), String::new(), 32_768));

    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    let has_api_key = store.get(api_key_store_key(&provider)).is_some();

    Ok(TextModelSettings {
        provider,
        model,
        has_api_key,
        context_window,
    })
}

#[tauri::command]
pub async fn save_text_model_settings(
    app: AppHandle,
    pool: State<'_, Pool>,
    provider: String,
    model: String,
    api_key: Option<String>,
) -> AppResult<()> {
    let context_window = fetch_openrouter_context_window(&model)
        .await
        .unwrap_or(32_768);
    let conn = pool.get()?;
    let value = json!({ "provider": provider, "model": model, "context_window": context_window })
        .to_string();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_TEXT_MODEL, value],
    )?;

    if let Some(key) = api_key {
        let store = app
            .store(SECRETS_STORE)
            .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
        let store_key = api_key_store_key(&provider);
        if key.is_empty() {
            store.delete(store_key);
        } else {
            store.set(store_key, json!(key));
        }
        store
            .save()
            .map_err(|e| AppError::Other(format!("failed to persist secrets store: {e}")))?;
    }

    Ok(())
}

/// Best-effort, fetched once at save time and persisted — not a live cache.
/// A failed fetch (or a model id that doesn't exactly match OpenRouter's
/// listing) is stored as the same 32,768 fallback as a real value, so
/// re-saving the model is the only way to pick up a corrected number.
async fn fetch_openrouter_context_window(model: &str) -> AppResult<usize> {
    let response: serde_json::Value = reqwest::Client::new()
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
        .map_err(|e| AppError::Other(format!("model metadata request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Other(format!("model metadata request failed: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Other(format!("invalid model metadata: {e}")))?;
    response
        .get("data")
        .and_then(|v| v.as_array())
        .and_then(|models| {
            models
                .iter()
                .find(|item| item.get("id").and_then(|v| v.as_str()) == Some(model))
        })
        .and_then(|item| item.get("context_length"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .ok_or_else(|| AppError::NotFound(format!("context metadata for model {model} not found")))
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
    /// Whether the narrator decides on its own that a passage is worth
    /// illustrating (see `commands::images::maybe_auto_image`), rather than
    /// images only ever coming from the player's "See" composer mode.
    pub narrator_images: bool,
}

#[tauri::command]
pub fn get_image_model_settings(
    app: AppHandle,
    pool: State<Pool>,
) -> AppResult<ImageModelSettings> {
    read_image_model_settings(&app, pool.inner())
}

/// Plain-`&Pool` variant of `get_image_model_settings` for callers that aren't
/// Tauri commands (the narrator-driven image path runs in a spawned task).
pub fn read_image_model_settings(app: &AppHandle, pool: &Pool) -> AppResult<ImageModelSettings> {
    let conn = pool.get()?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_IMAGE_MODEL],
            |row| row.get(0),
        )
        .ok();

    let (model, enabled, style, narrator_images) = stored
        .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
        .map(|v| {
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(crate::features::images::openrouter::DEFAULT_IMAGE_MODEL)
                .to_string();
            let enabled = v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true);
            let style = v
                .get("style")
                .and_then(|s| s.as_str())
                .unwrap_or(DEFAULT_IMAGE_STYLE)
                .to_string();
            let narrator_images = v
                .get("narrator_images")
                .and_then(|n| n.as_bool())
                .unwrap_or(true);
            (model, enabled, style, narrator_images)
        })
        .unwrap_or_else(|| {
            (
                crate::features::images::openrouter::DEFAULT_IMAGE_MODEL.to_string(),
                true,
                DEFAULT_IMAGE_STYLE.to_string(),
                true,
            )
        });

    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    let has_api_key = store.get(api_key_store_key("openrouter")).is_some();

    Ok(ImageModelSettings {
        model,
        enabled,
        style,
        has_api_key,
        narrator_images,
    })
}

#[tauri::command]
pub fn save_image_model_settings(
    pool: State<Pool>,
    model: String,
    enabled: bool,
    style: String,
    narrator_images: bool,
) -> AppResult<()> {
    let conn = pool.get()?;
    let style = if style.trim().is_empty() {
        DEFAULT_IMAGE_STYLE.to_string()
    } else {
        style.trim().to_string()
    };
    let value = json!({ "model": model, "enabled": enabled, "style": style, "narrator_images": narrator_images }).to_string();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_IMAGE_MODEL, value],
    )?;
    Ok(())
}

pub fn read_api_key(app: &AppHandle, provider: &str) -> AppResult<String> {
    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    store
        .get(api_key_store_key(provider))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| AppError::Invalid(format!("no API key set for provider '{provider}'")))
}

/// Resolves persisted settings and the provider secret into the configuration
/// consumed by the shared text-AI gateway.
pub fn resolve_text_model(app: &AppHandle, pool: &Pool) -> AppResult<TextModelConfig> {
    let settings = read_text_model_settings(app, pool)?;
    if settings.model.trim().is_empty() {
        return Err(AppError::Invalid(
            "no text model configured yet — set one in the Text Model panel".into(),
        ));
    }
    let api_key = read_api_key(app, &settings.provider)?;
    let provider = match settings.provider.as_str() {
        "openrouter" => TextProviderKind::OpenRouter,
        other => return Err(AppError::Invalid(format!("unsupported provider: {other}"))),
    };
    Ok(TextModelConfig {
        provider,
        model: settings.model,
        api_key,
        context_window: settings.context_window,
    })
}
