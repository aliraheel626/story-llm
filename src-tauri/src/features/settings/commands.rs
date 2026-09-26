use tauri::{AppHandle, State};

use crate::shared::db::{blocking, Pool};
use crate::shared::error::AppResult;

use super::model::{ImageModelSettings, TextModelSettings};
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
pub async fn save_image_model_settings(
    pool: State<'_, Pool>,
    model: String,
    enabled: bool,
    style: String,
) -> AppResult<()> {
    let pool = pool.inner().clone();
    blocking(move || repository::write_image_model_settings(&pool, model, enabled, style)).await
}
