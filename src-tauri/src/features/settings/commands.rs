use tauri::{AppHandle, State};

use crate::shared::db::{blocking, Pool};
use crate::shared::error::{AppError, AppResult};

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
    captions_enabled: bool,
    caption_model: String,
) -> AppResult<()> {
    let caption_model = caption_model.trim();
    let caption_model = if caption_model.is_empty() { crate::features::images::openrouter::DEFAULT_CAPTION_MODEL } else { caption_model }.to_string();
    if captions_enabled {
        match text_model::openrouter_accepts_images(&caption_model).await {
            Some(false) => return Err(AppError::Invalid(format!("{caption_model} can't read images; pick a vision model for captions"))),
            None => log::warn!("could not verify caption model image support: {caption_model}"),
            Some(true) => {}
        }
    }
    let pool = pool.inner().clone();
    blocking(move || repository::write_image_model_settings(&pool, model, enabled, style, captions_enabled, caption_model)).await
}
