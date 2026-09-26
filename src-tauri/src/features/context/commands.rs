use std::collections::BTreeMap;

use tauri::{AppHandle, State};

use crate::shared::db::{blocking, with_transaction, Pool};
use crate::shared::error::AppResult;

use super::preview::{self, ContextPreview, PreviewCallbacks};
use super::settings::{self, ContextItem, InjectionSettings};

#[tauri::command]
pub async fn preview_story_context(
    app: AppHandle,
    pool: State<'_, Pool>,
    callbacks: State<'_, PreviewCallbacks>,
    story_id: String,
) -> AppResult<ContextPreview> {
    let pool = pool.inner().clone();
    let callbacks = *callbacks.inner();
    blocking(move || {
        let config = (callbacks.config)(&app, &pool)?;
        let conn = pool.get()?;
        preview::build_preview(&conn, &story_id, &config, |conn, story_id| {
            (callbacks.metadata)(conn, story_id, config.image_enabled)
        })
    })
    .await
}

#[tauri::command]
pub fn get_story_context_settings(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<Vec<ContextItem>> {
    let conn = pool.get()?;
    Ok(settings::read_context_settings(&conn, &story_id)?.items())
}

#[tauri::command]
pub async fn save_story_context_settings(
    pool: State<'_, Pool>,
    story_id: String,
    include: BTreeMap<String, bool>,
) -> AppResult<Vec<ContextItem>> {
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            settings::write_context_settings(tx, &story_id, include)?;
            Ok(settings::read_context_settings(tx, &story_id)?.items())
        })
    })
    .await
}

#[tauri::command]
pub fn get_story_injection_settings(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<InjectionSettings> {
    let conn = pool.get()?;
    settings::read_injection_settings(&conn, &story_id)
}

#[tauri::command]
pub async fn save_story_injection_settings(
    pool: State<'_, Pool>,
    story_id: String,
    settings: InjectionSettings,
) -> AppResult<()> {
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            super::settings::write_injection_settings(tx, &story_id, settings)
        })
    })
    .await
}
