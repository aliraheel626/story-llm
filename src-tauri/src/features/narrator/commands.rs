use tauri::{AppHandle, State};

use crate::features::{context, settings};
use crate::shared::{
    db::{blocking, Pool},
    error::AppResult,
};

#[tauri::command]
pub async fn preview_story_context(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
) -> AppResult<context::preview::ContextPreview> {
    let pool = pool.inner().clone();
    blocking(move || {
        let model = settings::resolve_text_model(&app, &pool)?;
        let conn = pool.get()?;
        context::preview::build_preview(&conn, &story_id, &model)
    })
    .await
}
