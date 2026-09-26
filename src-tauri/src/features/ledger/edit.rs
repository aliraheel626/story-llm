use super::{
    attachments,
    model::{kind as ledger_kind, LedgerEntry},
    repository as ledger_repository,
};
use crate::shared::error::AppResult;

/// "Edit": append a content override for any visible ledger entry.
pub(super) fn edit_ledger_entry(
    conn: &rusqlite::Transaction<'_>,
    entry_id: &str,
    content: &str,
) -> AppResult<LedgerEntry> {
    let target = ledger_repository::get_entry(conn, entry_id)?;
    attachments::detach_from_entry(conn, entry_id)?;
    ledger_repository::append_entry(
        conn,
        &target.story_id,
        ledger_kind::CONTENT_EDITED,
        "hidden",
        Some(content),
        &serde_json::json!({"reason":"user_edit"}),
        Some(entry_id),
        target.turn_id.as_deref(),
    )?;
    ledger_repository::active_entry(conn, entry_id)
}
