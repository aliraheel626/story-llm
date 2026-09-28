use tauri::State;

use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::{model::StoryStats, repository};

#[tauri::command]
pub fn get_story_stats(pool: State<Pool>, story_id: String) -> AppResult<StoryStats> {
    let conn = pool.get()?;
    repository::story_stats(&conn, &story_id)
}
