use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, State};
use tauri_plugin_store::StoreExt;

use crate::ai::TextModelConfig;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

const SECRETS_STORE: &str = "secrets.json";
const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";
const SETTINGS_KEY_CONTEXT_INJECTION: &str = "context_injection";
const SETTINGS_KEY_LEDGER_RETENTION: &str = "ledger_retention";

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
    if !matches!(provider.as_str(), "openrouter" | "nous_portal") {
        return Err(AppError::Invalid(format!(
            "unsupported text model provider: {provider}"
        )));
    }
    let context_window = match provider.as_str() {
        "nous_portal" => fetch_context_window(
            &format!("{}/models", crate::ai::NOUS_PORTAL_BASE_URL),
            &model,
            Some(crate::ai::NOUS_PORTAL_USER_AGENT),
        )
        .await
        .unwrap_or(NOUS_PORTAL_DEFAULT_CONTEXT_WINDOW),
        _ => fetch_context_window("https://openrouter.ai/api/v1/models", &model, None)
            .await
            .unwrap_or(32_768),
    };
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
/// A failed fetch (or a model id that doesn't exactly match the provider's
/// listing) is stored as the same fallback as a real value, so re-saving the
/// model is the only way to pick up a corrected number.
async fn fetch_context_window(
    models_url: &str,
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

/// Hermes 4 (70B and 405B) both document a 131,072-token context window;
/// used when Nous Portal's own listing doesn't carry a `context_length`
/// field for the model (unlike OpenRouter, Nous Portal's `/v1/models` is not
/// guaranteed to include one — it's plain OpenAI-compatible, and real
/// OpenAI's own listing omits this field too).
const NOUS_PORTAL_DEFAULT_CONTEXT_WINDOW: usize = 131_072;

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
    /// Whether narrator-driven images are enabled. This gates the live
    /// `illustrate_scene` tool on narration paths that support it.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInjectionSettings {
    pub entity_context_mode: String,
    #[serde(default = "default_dice_rolls_in_context")]
    pub dice_rolls_in_context: bool,
}

fn default_dice_rolls_in_context() -> bool {
    true
}

impl Default for ContextInjectionSettings {
    fn default() -> Self {
        Self {
            entity_context_mode: "all".to_string(),
            dice_rolls_in_context: true,
        }
    }
}

#[tauri::command]
pub fn get_context_injection_settings(pool: State<Pool>) -> AppResult<ContextInjectionSettings> {
    read_context_injection_settings(pool.inner())
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

#[tauri::command]
pub fn save_context_injection_settings(
    pool: State<Pool>,
    entity_context_mode: String,
    dice_rolls_in_context: bool,
) -> AppResult<()> {
    write_context_injection_settings(pool.inner(), entity_context_mode, dice_rolls_in_context)
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

fn write_context_injection_settings(
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerRetentionSettings {
    pub tool_call_persistence: bool,
}

impl Default for LedgerRetentionSettings {
    fn default() -> Self {
        Self {
            tool_call_persistence: true,
        }
    }
}

#[tauri::command]
pub fn get_ledger_retention_settings(pool: State<Pool>) -> AppResult<LedgerRetentionSettings> {
    read_ledger_retention_settings(pool.inner())
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

#[tauri::command]
pub fn save_ledger_retention_settings(
    pool: State<Pool>,
    tool_call_persistence: bool,
) -> AppResult<()> {
    write_ledger_retention_settings(pool.inner(), tool_call_persistence)
}

fn write_ledger_retention_settings(pool: &Pool, tool_call_persistence: bool) -> AppResult<()> {
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
    Ok(TextModelConfig {
        provider: settings.provider,
        model: settings.model,
        api_key,
        context_window: settings.context_window,
    })
}
