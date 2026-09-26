use tauri::State;

use super::{edit, erase, model::{LedgerEntry, LedgerSnapshot}, reducer, repository, turns};
use crate::features::turn::TurnGate;
use crate::shared::db::{blocking, Pool};
use crate::shared::error::AppResult;

#[tauri::command]
pub fn list_ledger_entries(pool: State<Pool>, story_id: String) -> AppResult<LedgerSnapshot> {
    let conn = pool.get()?;
    Ok(reducer::snapshot(
        repository::list_logical_entries(&conn, &story_id)?,
        turns::list_summaries(&conn, &story_id)?,
    ))
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
