use tauri::{AppHandle, State};

use crate::features::ledger::model::LedgerEntry;
use crate::features::ledger::turn_tx::TurnGate;
use crate::shared::db::{blocking, Pool};
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

#[tauri::command]
pub async fn edit_ledger_entry(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    entry_id: String,
    content: String,
) -> AppResult<LedgerEntry> {
    let pool = pool.inner().clone();
    let gate = gate.inner().clone();
    blocking(move || edit::edit_ledger_entry(&pool, &gate, entry_id, content)).await
}

#[tauri::command]
pub async fn erase_last_exchange(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
) -> AppResult<Vec<String>> {
    let ticket = gate.check_idle(&story_id)?;
    let pool = pool.inner().clone();
    let gate = gate.inner().clone();
    blocking(move || erase::erase_last_exchange(&pool, &gate, ticket, &story_id)).await
}
