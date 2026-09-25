use crate::features::{
    images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        repository as ledger_repository,
        turn_tx::TurnGate,
    },
};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

/// "Edit": append a content override for any visible ledger entry.
pub(super) fn edit_ledger_entry(
    pool: &Pool,
    gate: &TurnGate,
    entry_id: String,
    content: String,
) -> AppResult<LedgerEntry> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    let conn = pool.get()?;
    let story_id = ledger_repository::get_entry(&conn, &entry_id)?.story_id;
    drop(conn);
    gate.check_idle(&story_id)?;
    with_transaction(pool, |tx| {
        let target = ledger_repository::get_entry(tx, &entry_id)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_the_generating_story_is_refused_before_writing() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now')",
            [],
        ).unwrap();
        let entry = ledger_repository::append_story_message(
            &conn, "s", "player", "do", "Original", None, None,
        )
        .unwrap();
        drop(conn);
        let gate = TurnGate::default();
        let _active = gate.acquire("s").unwrap();
        assert!(matches!(
            edit_ledger_entry(&pool, &gate, entry.id.clone(), "Changed".into()),
            Err(AppError::Invalid(message)) if message == "a turn is already generating"
        ));
        assert_eq!(
            ledger_repository::active_entry(&pool.get().unwrap(), &entry.id)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
    }
}
