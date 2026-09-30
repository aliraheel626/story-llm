use serde_json::json;
use rusqlite::OptionalExtension;
use tauri::AppHandle;

use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{
    ImageModelSettings, TextModelSettings, DEFAULT_IMAGE_STYLE, DEFAULT_TEXT_CONTEXT_WINDOW,
    DEFAULT_TEXT_MODEL, DEFAULT_TEXT_PROVIDER, DEFAULT_TEXT_SUPPORTS_IMAGES,
};
use super::secrets::has_api_key;

const SETTINGS_KEY_TEXT_MODEL: &str = "text_model_default";
const SETTINGS_KEY_IMAGE_MODEL: &str = "image_model_default";

pub(super) fn stored_text_model_row(pool: &Pool) -> AppResult<Option<serde_json::Value>> {
    let conn = pool.get()?;
    let row: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTINGS_KEY_TEXT_MODEL],
            |row| row.get(0),
        )
        .optional()?;
    row.map(|value| {
        serde_json::from_str(&value)
            .map_err(|error| AppError::Other(format!("invalid text model settings: {error}")))
    })
    .transpose()
}

pub(super) fn needs_capability_refresh(stored: &serde_json::Value) -> bool {
    stored.get("supports_images").is_none()
}

/// One atomic update preserves a foreground model save and unrelated JSON fields.
pub(super) fn write_missing_image_support(
    pool: &Pool,
    stored: &serde_json::Value,
    supports_images: bool,
) -> AppResult<bool> {
    let provider = stored.get("provider").and_then(|value| value.as_str()).unwrap_or("openrouter");
    let model = stored.get("model").and_then(|value| value.as_str()).unwrap_or_default();
    let context_window = stored.get("context_window").and_then(|value| value.as_i64()).unwrap_or(32_768);
    let conn = pool.get()?;
    Ok(conn.execute(
        "UPDATE settings SET value = json_set(value, '$.supports_images', json(?1))
         WHERE key = ?2 AND json_type(value, '$.supports_images') IS NULL
           AND json_extract(value, '$.provider') = ?3
           AND json_extract(value, '$.model') = ?4
           AND COALESCE(json_extract(value, '$.context_window'), 32768) = ?5",
        rusqlite::params![if supports_images { "true" } else { "false" }, SETTINGS_KEY_TEXT_MODEL, provider, model, context_window],
    )? == 1)
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

    let (provider, model, context_window, supports_images) = text_model_fields(stored.as_deref());

    let has_api_key = has_api_key(app, &provider)?;

    Ok(TextModelSettings {
        provider,
        model,
        has_api_key,
        context_window,
        supports_images,
    })
}

fn text_model_fields(stored: Option<&str>) -> (String, String, usize, bool) {
    stored
        .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
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
            let supports_images = stored_image_support(&v);
            (provider, model, context_window, supports_images)
        })
        .unwrap_or_else(|| {
            (
                DEFAULT_TEXT_PROVIDER.to_string(),
                DEFAULT_TEXT_MODEL.to_string(),
                DEFAULT_TEXT_CONTEXT_WINDOW,
                DEFAULT_TEXT_SUPPORTS_IMAGES,
            )
        })
}

fn stored_image_support(value: &serde_json::Value) -> bool {
    value
        .get("supports_images")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) fn write_text_model_settings(
    pool: &Pool,
    provider: &str,
    model: &str,
    context_window: usize,
    supports_images: bool,
) -> AppResult<()> {
    let conn = pool.get()?;
    let value = json!({ "provider": provider, "model": model, "context_window": context_window, "supports_images": supports_images })
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

    let (model, enabled, style, captions_enabled, caption_model) = image_model_fields(stored.as_deref());
    let has_api_key = has_api_key(app, "openrouter")?;
    Ok(ImageModelSettings { model, enabled, style, captions_enabled, caption_model, has_api_key })
}

fn image_model_fields(stored: Option<&str>) -> (String, bool, String, bool, String) {
    use crate::features::images::openrouter::DEFAULT_CAPTION_MODEL;
    stored
        .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
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
            let captions_enabled = v.get("captions_enabled").and_then(|e| e.as_bool()).unwrap_or(true);
            let caption_model = v.get("caption_model").and_then(|m| m.as_str())
                .map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_CAPTION_MODEL).to_string();
            (model, enabled, style, captions_enabled, caption_model)
        })
        .unwrap_or_else(|| {
            (
                crate::features::images::openrouter::DEFAULT_IMAGE_MODEL.to_string(),
                true,
                DEFAULT_IMAGE_STYLE.to_string(),
                true,
                DEFAULT_CAPTION_MODEL.to_string(),
            )
        })
}

pub(super) fn write_image_model_settings(
    pool: &Pool,
    model: String,
    enabled: bool,
    style: String,
    captions_enabled: bool,
    caption_model: String,
) -> AppResult<()> {
    let conn = pool.get()?;
    let style = if style.trim().is_empty() {
        DEFAULT_IMAGE_STYLE.to_string()
    } else {
        style.trim().to_string()
    };
    let caption_model = if caption_model.trim().is_empty() {
        crate::features::images::openrouter::DEFAULT_CAPTION_MODEL
    } else { caption_model.trim() };
    let value = json!({ "model": model, "enabled": enabled, "style": style,
        "captions_enabled": captions_enabled, "caption_model": caption_model }).to_string();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![SETTINGS_KEY_IMAGE_MODEL, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_settings_default_and_round_trip() {
        use crate::features::images::openrouter::DEFAULT_CAPTION_MODEL;
        for row in [None, Some("{}"), Some(r#"{"caption_model":"   "}"#)] {
            let fields = image_model_fields(row);
            assert!(fields.3);
            assert_eq!(fields.4, DEFAULT_CAPTION_MODEL);
        }
        let pool = crate::shared::db::test_pool();
        for (enabled, model, expected) in [(true, "", DEFAULT_CAPTION_MODEL), (false, " custom/vision ", "custom/vision")] {
            write_image_model_settings(&pool, "image".into(), true, "".into(), enabled, model.into()).unwrap();
            let row: String = pool.get().unwrap().query_row("SELECT value FROM settings WHERE key=?1", [SETTINGS_KEY_IMAGE_MODEL], |r| r.get(0)).unwrap();
            let fields = image_model_fields(Some(&row));
            assert_eq!((fields.3, fields.4.as_str()), (enabled, expected));
        }
    }

    #[test]
    fn new_install_uses_grok_4_7_without_a_settings_row() {
        let pool = crate::shared::db::test_pool();
        assert!(stored_text_model_row(&pool).unwrap().is_none());
        assert_eq!(
            text_model_fields(None),
            ("openrouter".into(), "x-ai/grok-4.7".into(), 500_000, true)
        );
    }

    #[test]
    fn saved_text_model_keeps_its_own_settings() {
        let pool = crate::shared::db::test_pool();
        write_text_model_settings(&pool, "openrouter", "x-ai/grok-4.3", 131_072, false).unwrap();
        let row = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [SETTINGS_KEY_TEXT_MODEL],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(
            text_model_fields(Some(&row)),
            ("openrouter".into(), "x-ai/grok-4.3".into(), 131_072, false)
        );
    }

    #[test]
    fn capability_refresh_only_applies_to_legacy_rows() {
        assert!(needs_capability_refresh(&json!({
            "provider":"openrouter","model":"x","context_window":1
        })));
        assert!(!needs_capability_refresh(&json!({"supports_images":true})));
        assert!(!needs_capability_refresh(&json!({"supports_images":false})));
        let missing: Option<serde_json::Value> = None;
        assert!(!missing.as_ref().is_some_and(needs_capability_refresh));
    }

    #[test]
    fn missing_image_support_update_is_atomic_and_preserves_other_settings() {
        let pool = crate::shared::db::test_pool();
        let legacy = json!({"provider":"openrouter","model":"legacy","context_window":1234,"custom":"kept"});
        pool.get().unwrap().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![SETTINGS_KEY_TEXT_MODEL, legacy.to_string()],
        ).unwrap();
        let stored = stored_text_model_row(&pool).unwrap().unwrap();
        assert!(write_missing_image_support(&pool, &stored, true).unwrap());
        assert_eq!(stored_text_model_row(&pool).unwrap().unwrap(), json!({
            "provider":"openrouter","model":"legacy","context_window":1234,"custom":"kept","supports_images":true
        }));

        pool.get().unwrap().execute(
            "UPDATE settings SET value = ?1 WHERE key = ?2",
            rusqlite::params![legacy.to_string(), SETTINGS_KEY_TEXT_MODEL],
        ).unwrap();
        write_text_model_settings(&pool, "openrouter", "foreground", 5678, false).unwrap();
        assert!(!write_missing_image_support(&pool, &stored, true).unwrap());
        assert_eq!(stored_text_model_row(&pool).unwrap().unwrap(), json!({
            "provider":"openrouter","model":"foreground","context_window":5678,"supports_images":false
        }));
    }

    #[test]
    fn legacy_and_invalid_image_support_default_to_false() {
        assert!(!stored_image_support(
            &json!({"provider":"openrouter","context_window":32768})
        ));
        assert!(!stored_image_support(&json!({"supports_images":"true"})));
        assert!(stored_image_support(&json!({"supports_images":true})));
    }
}
