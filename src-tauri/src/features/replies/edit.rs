use crate::features::{
    images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        repository as ledger_repository,
    },
};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

/// "Edit": append a content override for any visible ledger entry.
pub(super) fn edit_ledger_entry(
    pool: &Pool,
    entry_id: String,
    content: String,
) -> AppResult<LedgerEntry> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    with_transaction(pool, |tx| {
        let target = ledger_repository::get_entry(tx, &entry_id)?;
        if let Some(turn_id) = &target.turn_id {
            let pending: bool = tx.query_row(
                "SELECT status = 'pending' FROM turns WHERE id = ?1",
                [turn_id],
                |row| row.get(0),
            )?;
            if pending {
                return Err(AppError::Invalid(
                    "cannot edit an entry while its turn is generating".into(),
                ));
            }
        }
        images::detach_from_entry(tx, &entry_id)?;
        ledger_repository::append_entry(
            tx,
            &target.story_id,
            ledger_kind::CONTENT_EDITED,
            "hidden",
            Some(content),
            &serde_json::json!({"reason":"user_edit"}),
            Some(&entry_id),
            target.turn_id.as_deref(),
        )?;
        ledger_repository::active_entry(tx, &entry_id)
    })
}
