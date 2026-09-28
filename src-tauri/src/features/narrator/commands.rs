use tauri::{AppHandle, State};

use crate::features::{context, settings, stories};
use crate::shared::{
    db::{blocking, Pool},
    error::AppResult,
};

use super::catalog::{self, ToolAvailability};

#[tauri::command]
pub async fn preview_story_context(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
) -> AppResult<context::preview::ContextPreview> {
    let pool = pool.inner().clone();
    blocking(move || {
        let model = settings::resolve_text_model(&app, &pool)?;
        let image = settings::read_image_model_settings(&app, &pool)?;
        let conn = pool.get()?;
        let tool_settings = stories::settings::read_story_narrator_tools_conn(&conn, &story_id)?;
        let tools = catalog::enabled(&ToolAvailability {
            settings: &tool_settings,
            image_enabled: image.enabled && image.has_api_key && tool_settings.illustrate_scene,
            illustrate: false,
        });
        let descriptions = tools
            .iter()
            .map(|spec| context::blocks::ToolDescription {
                name: spec.name,
                instruction: spec.instruction,
            })
            .collect::<Vec<_>>();
        context::preview::build_preview(&conn, &story_id, &model, &descriptions)
    })
    .await
}
