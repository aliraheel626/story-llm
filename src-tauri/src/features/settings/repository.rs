use serde_json::json;
use tauri::AppHandle;

use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{
    ContextInjectionSettings, ImageModelSettings, TextModelSettings, DEFAULT_IMAGE_STYLE,
};
use super::secrets::has_api_key;

const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";
const SETTINGS_KEY_CONTEXT_INJECTION: &str = "context_injection";

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
                .filter(|p| matches!(*p, "openrouter" | "nous_portal"))
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

pub fn read_context_injection_settings(pool: &Pool) -> AppResult<ContextInjectionSettings> {
    let conn = pool.get()?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_CONTEXT_INJECTION],
            |row| row.get(0),
        )
        .ok();
    let mut settings = stored
        .and_then(|value| serde_json::from_str::<ContextInjectionSettings>(&value).ok())
        .unwrap_or_default();
    if !matches!(
        settings.entity_context_mode.as_str(),
        "all" | "scoped" | "none"
    ) {
        settings.entity_context_mode = "all".to_string();
    }
    Ok(settings)
}

pub(super) fn write_context_injection_settings(
    pool: &Pool,
    entity_context_mode: String,
    dice_rolls_in_context: bool,
) -> AppResult<()> {
    if !matches!(entity_context_mode.as_str(), "all" | "scoped" | "none") {
        return Err(AppError::Invalid(format!(
            "invalid narrator entity context mode: {entity_context_mode}"
        )));
    }
    let conn = pool.get()?;
    let context = ContextInjectionSettings {
        entity_context_mode,
        dice_rolls_in_context,
    };
    let value = serde_json::to_string(&context).map_err(|error| {
        AppError::Other(format!(
            "failed to serialize context injection settings: {error}"
        ))
    })?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_CONTEXT_INJECTION, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_injection_defaults_and_legacy_rows_preserve_entity_mode() {
        let pool = crate::shared::db::test_pool();
        let defaults = read_context_injection_settings(&pool).unwrap();
        assert_eq!(defaults.entity_context_mode, "all");
        assert!(defaults.dice_rolls_in_context);

        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                SETTINGS_KEY_CONTEXT_INJECTION,
                r#"{"entity_context_mode":"scoped"}"#
            ],
        )
        .unwrap();
        drop(conn);
        let legacy = read_context_injection_settings(&pool).unwrap();
        assert_eq!(legacy.entity_context_mode, "scoped");
        assert!(legacy.dice_rolls_in_context);
    }

    #[test]
    fn context_settings_survive_restart_without_recreating_legacy_memory() {
        let pool = crate::shared::db::test_pool();
        write_context_injection_settings(&pool, "scoped".to_string(), false).unwrap();
        let conn = pool.get().unwrap();
        let path: String = conn
            .query_row("PRAGMA database_list", [], |row| row.get(2))
            .unwrap();
        let dir = std::path::Path::new(&path).parent().unwrap().to_path_buf();
        let legacy_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'narrator_memory')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!legacy_exists);
        drop(conn);
        drop(pool);

        let restarted = crate::shared::db::init_pool(&dir).unwrap();
        let context = read_context_injection_settings(&restarted).unwrap();
        assert_eq!(context.entity_context_mode, "scoped");
        assert!(!context.dice_rolls_in_context);
        let conn = restarted.get().unwrap();
        let context: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [SETTINGS_KEY_CONTEXT_INJECTION],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&context).unwrap(),
            json!({"entity_context_mode": "scoped", "dice_rolls_in_context": false})
        );
        let legacy_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'narrator_memory')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!legacy_exists);
    }
}
