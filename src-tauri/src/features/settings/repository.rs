use serde_json::json;
use tauri::AppHandle;

use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::model::{ImageModelSettings, TextModelSettings, DEFAULT_IMAGE_STYLE};
use super::secrets::has_api_key;

const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";

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
                .filter(|p| matches!(*p, "openrouter" | "nous_portal" | "ollama"))
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

    let has_api_key = has_api_key(app, &provider)?;

    Ok(TextModelSettings {
        provider,
        model,
        has_api_key,
        context_window,
    })
}

pub(super) fn write_text_model_settings(
    pool: &Pool,
    provider: &str,
    model: &str,
    context_window: usize,
) -> AppResult<()> {
    let conn = pool.get()?;
    let value = json!({ "provider": provider, "model": model, "context_window": context_window })
        .to_string();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_TEXT_MODEL, value],
    )?;

    Ok(())
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

    let (model, enabled, style) = stored
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
            (model, enabled, style)
        })
        .unwrap_or_else(|| {
            (
                crate::features::images::openrouter::DEFAULT_IMAGE_MODEL.to_string(),
                true,
                DEFAULT_IMAGE_STYLE.to_string(),
            )
        });

    let has_api_key = has_api_key(app, "openrouter")?;

    Ok(ImageModelSettings {
        model,
        enabled,
        style,
        has_api_key,
    })
}

pub(super) fn write_image_model_settings(
    pool: &Pool,
    model: String,
    enabled: bool,
    style: String,
) -> AppResult<()> {
    let conn = pool.get()?;
    let style = if style.trim().is_empty() {
        DEFAULT_IMAGE_STYLE.to_string()
    } else {
        style.trim().to_string()
    };
    let value = json!({ "model": model, "enabled": enabled, "style": style }).to_string();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_IMAGE_MODEL, value],
    )?;
    Ok(())
}
