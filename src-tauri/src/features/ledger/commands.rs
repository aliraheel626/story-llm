use tauri::State;

use super::{model::LedgerSnapshot, reducer, repository, turns};
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

#[tauri::command]
pub fn list_ledger_entries(pool: State<Pool>, story_id: String) -> AppResult<LedgerSnapshot> {
    let conn = pool.get()?;
    Ok(reducer::snapshot(
        repository::list_logical_entries(&conn, &story_id)?,
        turns::list_summaries(&conn, &story_id)?,
    ))
}
