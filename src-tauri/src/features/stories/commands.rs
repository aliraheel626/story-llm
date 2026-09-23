use tauri::State;

use super::{author_note, model::Story, repository, settings};
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

#[tauri::command]
pub fn list_stories(pool: State<Pool>) -> AppResult<Vec<Story>> {
    repository::list_stories(pool.inner())
}

#[tauri::command]
pub fn create_story(
    pool: State<Pool>,
    title: Option<String>,
    settings: Option<serde_json::Value>,
) -> AppResult<Story> {
    repository::create_story_in_pool(pool.inner(), title, settings)
}

#[tauri::command]
pub fn rename_story(pool: State<Pool>, story_id: String, title: String) -> AppResult<()> {
    repository::rename_story(pool.inner(), &story_id, &title)
}

#[tauri::command]
pub fn delete_story(pool: State<Pool>, story_id: String) -> AppResult<()> {
    repository::delete_story(pool.inner(), &story_id)
}

#[tauri::command]
pub fn get_author_note(pool: State<Pool>, story_id: String) -> AppResult<String> {
    author_note::read_author_note(pool.inner(), &story_id)
}

#[tauri::command]
pub fn save_author_note(pool: State<Pool>, story_id: String, note: String) -> AppResult<()> {
    author_note::write_author_note(pool.inner(), &story_id, &note)
}

#[tauri::command]
pub fn get_author_note_enabled(pool: State<Pool>, story_id: String) -> AppResult<bool> {
    author_note::read_author_note_enabled(pool.inner(), &story_id)
}

#[tauri::command]
pub fn set_author_note_enabled(
    pool: State<Pool>,
    story_id: String,
    enabled: bool,
) -> AppResult<()> {
    author_note::write_author_note_enabled(pool.inner(), &story_id, enabled)
}

#[tauri::command]
pub fn get_story_narrator_tools(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<settings::NarratorToolSettings> {
    settings::read_story_narrator_tools(pool.inner(), &story_id)
}

#[tauri::command]
pub fn save_story_narrator_tools(
    pool: State<Pool>,
    story_id: String,
    tools: settings::NarratorToolSettings,
) -> AppResult<()> {
    settings::save_narrator_tools(pool.inner(), &story_id, tools)
}

#[tauri::command]
pub fn get_story_reasoning_effort(pool: State<Pool>, story_id: String) -> AppResult<String> {
    settings::read_story_reasoning_effort(pool.inner(), &story_id)
}

#[tauri::command]
pub fn save_story_reasoning_effort(
    pool: State<Pool>,
    story_id: String,
    reasoning_effort: String,
) -> AppResult<()> {
    settings::save_reasoning_effort(pool.inner(), &story_id, &reasoning_effort)
}
