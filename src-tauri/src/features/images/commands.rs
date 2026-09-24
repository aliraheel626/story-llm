use tauri::State;

use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::{model::StoryImage, repository};

/// All images for every passage in a story, in one call — the frontend
/// groups them by `entry_id` itself rather than issuing one query per entry.
#[tauri::command]
pub fn list_images_for_story(pool: State<Pool>, story_id: String) -> AppResult<Vec<StoryImage>> {
    let conn = pool.get()?;
    repository::list_for_story(&conn, &story_id)
}
