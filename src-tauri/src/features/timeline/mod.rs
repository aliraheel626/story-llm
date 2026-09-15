pub mod compaction;
pub mod model;
pub mod projections;
pub mod reducer;
pub mod repository;

use tauri::State;

use crate::shared::db::Pool;
use crate::shared::error::AppResult;
use model::TimelineEntry;

#[tauri::command]
pub fn list_timeline_entries(
    pool: State<Pool>,
    branch_id: String,
) -> AppResult<Vec<TimelineEntry>> {
    let conn = pool.get()?;
    repository::list_logical_entries(&conn, &branch_id)
}
