use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, State};
use tauri_plugin_store::StoreExt;

use crate::db::Pool;
use crate::error::{AppError, AppResult};

const SECRETS_STORE: &str = "secrets.json";
const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextModelSettings {
    pub provider: String,
    pub model: String,
    pub has_api_key: bool,
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

    let (provider, model) = stored
        .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
        .map(|v| {
            let provider = v.get("provider").and_then(|p| p.as_str()).unwrap_or("openrouter").to_string();
            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
            (provider, model)
        })
        .unwrap_or_else(|| ("openrouter".to_string(), String::new()));

    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    let has_api_key = store.get(api_key_store_key(&provider)).is_some();

    Ok(TextModelSettings { provider, model, has_api_key })
}

#[tauri::command]
pub fn save_text_model_settings(
    app: AppHandle,
    pool: State<Pool>,
    provider: String,
    model: String,
    api_key: Option<String>,
) -> AppResult<()> {
    let conn = pool.get()?;
    let value = json!({ "provider": provider, "model": model }).to_string();
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

pub const DEFAULT_IMAGE_STYLE: &str = "Digital painting, atmospheric scene illustration.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageModelSettings {
    pub model: String,
    pub enabled: bool,
    /// A style prefix folded into every generated prompt (e.g. "anime",
    /// "photorealistic") — see `crate::images::DEFAULT_IMAGE_MODEL`'s prompt
    /// composition in `commands::images::build_image_prompt`.
    pub style: String,
    /// OpenRouter render tier (`512`/`1K`/`2K`/`4K`) — lower renders faster.
    /// Empty means "Auto": the parameter is omitted so the model chooses.
    pub resolution: String,
    /// Images reuse the OpenRouter key set in the Text Model panel — there is
    /// only one provider (OpenRouter) for both text and images.
    pub has_api_key: bool,
}

#[tauri::command]
pub fn get_image_model_settings(app: AppHandle, pool: State<Pool>) -> AppResult<ImageModelSettings> {
    let conn = pool.get()?;
    let stored: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [SETTINGS_KEY_IMAGE_MODEL], |row| row.get(0))
        .ok();

    let (model, enabled, style, resolution) = stored
        .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
        .map(|v| {
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(crate::images::DEFAULT_IMAGE_MODEL)
                .to_string();
            let enabled = v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true);
            let style = v.get("style").and_then(|s| s.as_str()).unwrap_or(DEFAULT_IMAGE_STYLE).to_string();
            let resolution = v
                .get("resolution")
                .and_then(|r| r.as_str())
                .unwrap_or(crate::images::DEFAULT_IMAGE_RESOLUTION)
                .to_string();
            (model, enabled, style, resolution)
        })
        .unwrap_or_else(|| {
            (
                crate::images::DEFAULT_IMAGE_MODEL.to_string(),
                true,
                DEFAULT_IMAGE_STYLE.to_string(),
                crate::images::DEFAULT_IMAGE_RESOLUTION.to_string(),
            )
        });

    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    let has_api_key = store.get(api_key_store_key("openrouter")).is_some();

    Ok(ImageModelSettings { model, enabled, style, resolution, has_api_key })
}

#[tauri::command]
pub fn save_image_model_settings(pool: State<Pool>, model: String, enabled: bool, style: String, resolution: String) -> AppResult<()> {
    let conn = pool.get()?;
    let style = if style.trim().is_empty() { DEFAULT_IMAGE_STYLE.to_string() } else { style.trim().to_string() };
    // Empty means "let the model choose" — kept as-is, not coerced to a tier.
    let resolution = resolution.trim().to_string();
    let value = json!({ "model": model, "enabled": enabled, "style": style, "resolution": resolution }).to_string();
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
