use tauri::State;

use super::{model::Story, repository, settings};
use crate::features::turn::TurnGate;
use crate::shared::db::{blocking, Pool};
use crate::shared::error::AppResult;

#[tauri::command]
pub fn list_stories(pool: State<Pool>) -> AppResult<Vec<Story>> {
    repository::list_stories(pool.inner())
}

#[tauri::command]
pub async fn create_story(
    pool: State<'_, Pool>,
    title: Option<String>,
    settings: Option<serde_json::Value>,
) -> AppResult<Story> {
    let pool = pool.inner().clone();
    blocking(move || repository::create_story_in_pool(&pool, title, settings)).await
}

#[tauri::command]
pub async fn rename_story(pool: State<'_, Pool>, story_id: String, title: String) -> AppResult<()> {
    let pool = pool.inner().clone();
    blocking(move || repository::rename_story(&pool, &story_id, &title)).await
}

#[tauri::command]
pub async fn delete_story(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
) -> AppResult<()> {
    let ticket = gate.check_idle(&story_id)?;
    let pool = pool.inner().clone();
    let gate = gate.inner().clone();
    blocking(move || repository::delete_story(&pool, &gate, ticket, &story_id)).await
}

#[tauri::command]
pub fn get_story_narrator_tools(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<settings::NarratorToolSettings> {
    settings::read_story_narrator_tools(pool.inner(), &story_id)
}

#[tauri::command]
pub async fn save_story_narrator_tools(
    pool: State<'_, Pool>,
    story_id: String,
    tools: settings::NarratorToolSettings,
) -> AppResult<()> {
    let pool = pool.inner().clone();
    blocking(move || settings::save_narrator_tools(&pool, &story_id, tools)).await
}

#[tauri::command]
pub fn get_story_reasoning_effort(pool: State<Pool>, story_id: String) -> AppResult<String> {
    settings::read_story_reasoning_effort(pool.inner(), &story_id)
}

#[tauri::command]
pub async fn save_story_reasoning_effort(
    pool: State<'_, Pool>,
    story_id: String,
    reasoning_effort: String,
) -> AppResult<()> {
    let pool = pool.inner().clone();
    blocking(move || settings::save_reasoning_effort(&pool, &story_id, &reasoning_effort)).await
}
