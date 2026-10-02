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
pub fn list_transcript_entries(pool: State<Pool>, story_id: String) -> AppResult<TranscriptSnapshot> {
    let conn = pool.get()?;
    Ok(reducer::snapshot(
        repository::list_logical_entries(&conn, &story_id)?,
        turns::list_summaries(&conn, &story_id)?,
    ))
}

#[tauri::command]
pub async fn edit_transcript_entry(
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
    use crate::features::entities::events::EntityEvent;
    use crate::features::turn::TurnTx;
    use crate::shared::test_support;

    #[test]
    fn erasing_a_deletion_cannot_restore_a_reused_present_name() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        entities::create_entity_with_id_sync(
            &conn, "a", "s", "character", "Mira", Some("silver hair"), "user", None, None,
        ).unwrap();
        let (turn_id, _, narration_id) = test_support::exchange(&conn, "s", "do", "act", Some("Mira leaves."));
        entities::projection::record(
            &conn, "s", narration_id.as_deref(), "Mira was removed.",
            &EntityEvent::Deleted { entity_id: "a".into(), name: "Mira".into(), source: "narrator_tool".into() },
            Some(&turn_id),
        ).unwrap();
        entities::create_entity_with_id_sync(
            &conn, "b", "s", "character", "mira", Some("black armor"), "user", None, None,
        ).unwrap();
        let snapshot = |conn: &rusqlite::Connection| {
            conn.prepare(
                "SELECT e.id, e.kind, e.name, c.appearance_anchor, e.is_present, e.created_at, e.updated_at
                 FROM entities e JOIN characters c ON c.entity_id=e.id WHERE e.story_id = 's' ORDER BY e.id",
            ).unwrap().query_map([], |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?, row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?, row.get::<_, String>(6)?,
            ))).unwrap().collect::<Result<Vec<_>, _>>().unwrap()
        };
        let before = snapshot(&conn);
        let entries = repository::list_logical_entries(&conn, "s").unwrap();
        let updated_at: String = conn.query_row("SELECT updated_at FROM stories WHERE id = 's'", [], |row| row.get(0)).unwrap();
        drop(conn);

        let gate = TurnGate::default();
        assert!(matches!(
            erase_with_ticket(&pool, &gate, gate.check_idle("s").unwrap(), "s"),
            Err(AppError::Db(rusqlite::Error::SqliteFailure(error, _)))
                if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        ));
        let conn = pool.get().unwrap();
        assert_eq!(snapshot(&conn), before);
        assert_eq!(repository::list_logical_entries(&conn, "s").unwrap(), entries);
        assert_eq!(turns::last_turn(&conn, "s").unwrap().unwrap().id, turn_id);
        assert_eq!(conn.query_row("SELECT updated_at FROM stories WHERE id = 's'", [], |row| row.get::<_, String>(0)).unwrap(), updated_at);
    }

    #[test]
    fn erasing_earlier_narration_allows_reused_historical_names() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        entities::create_entity_with_id_sync(
            &conn, "a", "s", "character", "Mira", Some("silver hair"), "user", None, None,
        ).unwrap();
        let (turn_id, action_id, narration_id) = test_support::exchange(&conn, "s", "do", "act", Some("A new cloak."));
        entities::update_entity_sync(
            &conn, "s", "a", "Mira", Some("red cloak"), "narrator_tool", narration_id.as_deref(), Some(&turn_id),
        ).unwrap();
        entities::repository::delete_entity_sync(&conn, "s", "a").unwrap();
        entities::create_entity_with_id_sync(
            &conn, "b", "s", "character", "mira", Some("black armor"), "user", None, None,
        ).unwrap();
        let b_before: (String, Option<String>, i64, String) = conn.query_row(
            "SELECT e.name, c.appearance_anchor, e.is_present, e.updated_at
             FROM entities e JOIN characters c ON c.entity_id=e.id WHERE e.story_id = 's' AND e.id = 'b'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        drop(conn);

        let gate = TurnGate::default();
        assert_eq!(
            erase_with_ticket(&pool, &gate, gate.check_idle("s").unwrap(), "s").unwrap(),
            vec![action_id, narration_id.unwrap()]
        );
        let conn = pool.get().unwrap();
        assert_eq!(conn.query_row(
            "SELECT e.name, c.appearance_anchor, e.is_present
             FROM entities e JOIN characters c ON c.entity_id=e.id WHERE e.story_id = 's' AND e.id = 'a'",
            [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?)),
        ).unwrap(), ("Mira".into(), Some("silver hair".into()), 0));
        assert_eq!(conn.query_row(
            "SELECT e.name, c.appearance_anchor, e.is_present, e.updated_at
             FROM entities e JOIN characters c ON c.entity_id=e.id WHERE e.story_id = 's' AND e.id = 'b'",
            [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
        ).unwrap(), b_before);
        let present = entities::list_entities_sync(&conn, "s", None).unwrap();
        assert_eq!(present.len(), 1);
        assert_eq!(present[0].id, "b");
    }

    #[test]
    fn erasing_narrator_rename_preserves_later_player_anchor_edit() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        entities::create_entity_with_id_sync(
            &conn, "a", "s", "character", "Mira", Some("silver hair"), "user", None, None,
        ).unwrap();
        let created_at: String = conn.query_row(
            "SELECT created_at FROM entities WHERE id = 'a'", [], |row| row.get(0),
        ).unwrap();
        let (turn_id, action_id, narration_id) = test_support::exchange(&conn, "s", "do", "act", Some("Mira Vale arrives."));
        entities::update_entity_sync(
            &conn, "s", "a", "Mira Vale", Some("silver hair"), "narrator_tool", narration_id.as_deref(), Some(&turn_id),
        ).unwrap();
        entities::update_entity_sync(
            &conn, "s", "a", "Mira Vale", Some("black armor"), "user", None, None,
        ).unwrap();
        drop(conn);

        let gate = TurnGate::default();
        assert_eq!(
            erase_with_ticket(&pool, &gate, gate.check_idle("s").unwrap(), "s").unwrap(),
            vec![action_id, narration_id.unwrap()]
        );
        let present = entities::list_entities_sync(&pool.get().unwrap(), "s", None).unwrap();
        assert_eq!(present.len(), 1);
        assert_eq!((present[0].id.as_str(), present[0].name.as_str(), present[0].character.appearance_anchor.as_deref()),
            ("a", "Mira", Some("black armor")));
        assert_eq!(present[0].created_at, created_at);
    }

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
