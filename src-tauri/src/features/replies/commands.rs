use tauri::{AppHandle, State};

use crate::features::ledger::model::LedgerEntry;
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

use super::{
    edit, erase,
    model::{RetryResult, SubmitTurnResult},
    retry, submit,
};

#[tauri::command]
pub async fn submit_turn(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    submit::submit_turn(app, pool.inner(), story_id, mode, content).await
}

#[tauri::command]
pub async fn retry_narration(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    retry::retry_narration(app, pool.inner(), story_id, entry_id).await
}

#[tauri::command]
pub fn edit_ledger_entry(
    pool: State<Pool>,
    entry_id: String,
    content: String,
) -> AppResult<LedgerEntry> {
    edit::edit_ledger_entry(pool.inner(), entry_id, content)
}

#[tauri::command]
pub fn erase_last_exchange(pool: State<Pool>, story_id: String) -> AppResult<Vec<String>> {
    erase::erase_last_exchange(pool.inner(), story_id)
}
