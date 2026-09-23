pub mod model;
pub mod projections;
pub mod reducer;
pub mod repository;

use tauri::State;

use crate::shared::db::Pool;
use crate::shared::error::AppResult;
use model::LedgerSnapshot;

#[tauri::command]
pub fn list_ledger_entries(pool: State<Pool>, story_id: String) -> AppResult<LedgerSnapshot> {
    let conn = pool.get()?;
    let entries = repository::list_logical_entries(&conn, &story_id)?;
    Ok(LedgerSnapshot {
        visible: reducer::active_visible_entries(&entries),
        hidden: entries
            .into_iter()
            .filter(|entry| entry.visibility == "hidden")
            .collect(),
    })
}
