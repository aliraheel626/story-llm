use tauri::State;

use super::{
    edit, erase,
    model::{TranscriptEntry, TranscriptSnapshot},
    reducer, repository, turns,
};
use crate::features::{
    entities,
    turn::{TurnGate, TurnTicket},
};
use crate::shared::db::{blocking, with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

fn edit_with_gate(
    pool: &Pool,
    gate: &TurnGate,
    entry_id: &str,
    content: &str,
) -> AppResult<TranscriptEntry> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    let conn = pool.get()?;
    let story_id = repository::get_entry(&conn, entry_id)?.story_id;
    drop(conn);
    let ticket = gate.check_idle(&story_id)?;
    edit_with_ticket(pool, gate, ticket, entry_id, content)
}

fn edit_with_ticket(
    pool: &Pool,
    gate: &TurnGate,
    ticket: TurnTicket,
    entry_id: &str,
    content: &str,
) -> AppResult<TranscriptEntry> {
    with_transaction(pool, |tx| {
        gate.still_idle(&ticket)?;
        edit::edit_transcript_entry(tx, entry_id, content)
    })
}

fn erase_with_ticket(
    pool: &Pool,
    gate: &TurnGate,
    ticket: TurnTicket,
    story_id: &str,
) -> AppResult<Vec<String>> {
    with_transaction(pool, |tx| {
        gate.still_idle(&ticket)?;
        let Some(removed) = erase::erase_last_exchange_in_tx(tx, story_id)? else {
            return Ok(vec![]);
        };
        entities::projection::replay_after_erase(tx, story_id, &removed.entries)?;
        Ok(removed.visible_ids)
    })
}

#[tauri::command]
pub fn list_ledger_entries(pool: State<Pool>, story_id: String) -> AppResult<TranscriptSnapshot> {
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
) -> AppResult<TranscriptEntry> {
    let pool = pool.inner().clone();
    let gate = gate.inner().clone();
    blocking(move || edit_with_gate(&pool, &gate, &entry_id, &content)).await
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
    blocking(move || erase_with_ticket(&pool, &gate, ticket, &story_id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::turn::TurnTx;
    use crate::shared::test_support;

    #[test]
    fn editing_the_generating_story_is_refused_before_writing() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        let entry =
            repository::append_story_message(&conn, "s", "player", "do", "Original", None, None)
                .unwrap();
        drop(conn);
        let gate = TurnGate::default();
        let _active = gate.acquire("s").unwrap();
        assert!(matches!(
            edit_with_gate(&pool, &gate, &entry.id, "Changed"),
            Err(AppError::Invalid(message)) if message == "a turn is already generating"
        ));
        assert_eq!(
            repository::active_entry(&pool.get().unwrap(), &entry.id)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
    }

    #[tokio::test]
    async fn edit_rejects_a_turn_started_since_the_idle_check() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        let entry =
            repository::append_story_message(&conn, "s", "player", "do", "Original", None, None)
                .unwrap();
        drop(conn);
        let gate = TurnGate::default();
        let ticket = gate.check_idle("s").unwrap();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.commit().await.unwrap();
        assert!(matches!(
            edit_with_ticket(&pool, &gate, ticket, &entry.id, "Changed"),
            Err(AppError::Invalid(message)) if message == "a turn is already generating"
        ));
        assert_eq!(
            repository::active_entry(&pool.get().unwrap(), &entry.id)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
    }

    #[tokio::test]
    async fn erase_rejects_a_turn_started_since_the_idle_check() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        test_support::story(&pool.get().unwrap(), "s");
        let ticket = gate.check_idle("s").unwrap();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let entry_id = turn
            .with(|conn| {
                let (_, action_id, _) =
                    test_support::exchange(conn, "s", "do", "action", Some("new response"));
                Ok(action_id)
            })
            .await
            .unwrap();
        turn.commit().await.unwrap();

        assert!(matches!(
            erase_with_ticket(&pool, &gate, ticket, "s"),
            Err(AppError::Invalid(message)) if message == "a turn is already generating"
        ));
        assert!(repository::get_entry(&pool.get().unwrap(), &entry_id).is_ok());
    }
}
