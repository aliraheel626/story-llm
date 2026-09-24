use rusqlite::OptionalExtension;
use serde_json::json;
use tauri::AppHandle;

use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{
    ContextInjectionSettings, ImageModelSettings, LedgerRetentionSettings, TextModelSettings,
    DEFAULT_IMAGE_STYLE,
};
use super::secrets::has_api_key;

const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";
const SETTINGS_KEY_CONTEXT_INJECTION: &str = "context_injection";
const SETTINGS_KEY_LEDGER_RETENTION: &str = "ledger_retention";

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

fn write_legacy_narrator_memory_settings(
    conn: &rusqlite::Connection,
    entity_context_mode: &str,
    tool_call_persistence: bool,
) -> AppResult<()> {
    let value = json!({
        "entity_context_mode": entity_context_mode,
        "tool_call_persistence": tool_call_persistence,
    })
    .to_string();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('narrator_memory', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [value],
    )?;
    Ok(())
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
    let mut conn = pool.get()?;
    let context = ContextInjectionSettings {
        entity_context_mode,
        dice_rolls_in_context,
    };
    let value = serde_json::to_string(&context).map_err(|error| {
        AppError::Other(format!(
            "failed to serialize context injection settings: {error}"
        ))
    })?;
    let tx = conn.transaction()?;
    let retention: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_LEDGER_RETENTION],
            |row| row.get(0),
        )
        .optional()?;
    let tool_call_persistence = retention
        .and_then(|value| serde_json::from_str::<LedgerRetentionSettings>(&value).ok())
        .unwrap_or_default()
        .tool_call_persistence;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_CONTEXT_INJECTION, value],
    )?;
    write_legacy_narrator_memory_settings(
        &tx,
        &context.entity_context_mode,
        tool_call_persistence,
    )?;
    tx.commit()?;
    Ok(())
}

pub fn read_ledger_retention_settings(pool: &Pool) -> AppResult<LedgerRetentionSettings> {
    let conn = pool.get()?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_LEDGER_RETENTION],
            |row| row.get(0),
        )
        .ok();
    Ok(stored
        .and_then(|value| serde_json::from_str::<LedgerRetentionSettings>(&value).ok())
        .unwrap_or_default())
}

pub(super) fn write_ledger_retention_settings(
    pool: &Pool,
    tool_call_persistence: bool,
) -> AppResult<()> {
    let mut conn = pool.get()?;
    let value = serde_json::to_string(&LedgerRetentionSettings {
        tool_call_persistence,
    })
    .map_err(|error| {
        AppError::Other(format!(
            "failed to serialize ledger retention settings: {error}"
        ))
    })?;
    let tx = conn.transaction()?;
    let context: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_CONTEXT_INJECTION],
            |row| row.get(0),
        )
        .optional()?;
    let entity_context_mode = context
        .and_then(|value| serde_json::from_str::<ContextInjectionSettings>(&value).ok())
        .unwrap_or_default()
        .entity_context_mode;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_LEDGER_RETENTION, value],
    )?;
    write_legacy_narrator_memory_settings(&tx, &entity_context_mode, tool_call_persistence)?;
    tx.commit()?;
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
    fn settings_saves_keep_legacy_preferences_current_for_rollback() {
        let pool = crate::shared::db::test_pool();
        write_context_injection_settings(&pool, "scoped".to_string(), false).unwrap();
        write_ledger_retention_settings(&pool, false).unwrap();
        let conn = pool.get().unwrap();
        let legacy: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'narrator_memory'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let legacy: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        assert_eq!(legacy["entity_context_mode"], "scoped");
        assert_eq!(legacy["tool_call_persistence"], false);
        drop(conn);

        write_context_injection_settings(&pool, "none".to_string(), false).unwrap();
        let conn = pool.get().unwrap();
        let legacy: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'narrator_memory'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let legacy: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        assert_eq!(legacy["entity_context_mode"], "none");
        assert_eq!(legacy["tool_call_persistence"], false);
    }
}
