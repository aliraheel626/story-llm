use tauri::{AppHandle, State};

use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::model::{ContextInjectionSettings, ImageModelSettings, TextModelSettings};
use super::{repository, text_model};

#[tauri::command]
pub fn get_text_model_settings(app: AppHandle, pool: State<Pool>) -> AppResult<TextModelSettings> {
    repository::read_text_model_settings(&app, pool.inner())
}

#[tauri::command]
pub async fn save_text_model_settings(
    app: AppHandle,
    pool: State<'_, Pool>,
    provider: String,
    model: String,
    api_key: Option<String>,
) -> AppResult<()> {
    text_model::save_text_model_settings(&app, pool.inner(), provider, model, api_key).await
}

#[tauri::command]
pub fn get_image_model_settings(
    app: AppHandle,
    pool: State<Pool>,
) -> AppResult<ImageModelSettings> {
    repository::read_image_model_settings(&app, pool.inner())
}

#[tauri::command]
pub fn save_image_model_settings(
    pool: State<Pool>,
    model: String,
    enabled: bool,
    style: String,
) -> AppResult<()> {
    repository::write_image_model_settings(pool.inner(), model, enabled, style)
}

#[tauri::command]
pub fn get_context_injection_settings(pool: State<Pool>) -> AppResult<ContextInjectionSettings> {
    repository::read_context_injection_settings(pool.inner())
}

#[tauri::command]
pub fn save_context_injection_settings(
    pool: State<Pool>,
    entity_context_mode: String,
) -> AppResult<()> {
    repository::write_context_injection_settings(pool.inner(), entity_context_mode)
}
