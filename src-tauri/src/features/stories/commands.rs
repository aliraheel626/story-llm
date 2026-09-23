use tauri::State;

use super::author_note;
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

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
