use tauri::State;

use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::{model::{StoryCostBreakdown, StoryUsage}, repository};

#[tauri::command]
pub fn get_story_usage(pool: State<Pool>, story_id: String) -> AppResult<StoryUsage> {
    let conn = pool.get()?;
    repository::story_usage(&conn, &story_id)
}

#[tauri::command]
pub fn get_story_usage_breakdown(pool: State<Pool>, story_id: String) -> AppResult<StoryCostBreakdown> {
    let conn = pool.get()?;
    repository::cost_breakdown(&conn, &story_id)
}
