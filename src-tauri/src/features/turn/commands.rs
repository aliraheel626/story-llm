use tauri::{AppHandle, State};

use crate::features::turn::TurnGate;
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::{model::{RetryResult, SubmitTurnResult}, retry, submit};

#[tauri::command]
pub async fn submit_turn(
    app: AppHandle,
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    submit::submit_turn(app, pool.inner(), gate.inner(), story_id, mode, content).await
}

#[tauri::command]
pub async fn retry_narration(
    app: AppHandle,
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    retry::retry_narration(app, pool.inner(), gate.inner(), story_id, entry_id).await
}
