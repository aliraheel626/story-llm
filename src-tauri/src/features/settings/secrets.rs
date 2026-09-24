use serde_json::json;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use crate::shared::error::{AppError, AppResult};

const SECRETS_STORE: &str = "secrets.json";

fn api_key_store_key(provider: &str) -> String {
    format!("text_model.{provider}.api_key")
}

pub(super) fn has_api_key(app: &AppHandle, provider: &str) -> AppResult<bool> {
    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    Ok(store.get(api_key_store_key(provider)).is_some())
}

pub(super) fn write_api_key(app: &AppHandle, provider: &str, key: &str) -> AppResult<()> {
    let store = app
        .store(SECRETS_STORE)
        .map_err(|e| AppError::Other(format!("failed to open secrets store: {e}")))?;
    let store_key = api_key_store_key(provider);
    if key.is_empty() {
        store.delete(store_key);
    } else {
        store.set(store_key, json!(key));
    }
    store
        .save()
        .map_err(|e| AppError::Other(format!("failed to persist secrets store: {e}")))?;
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
