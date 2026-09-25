use std::sync::Arc;

use tauri::AppHandle;

use crate::features::ledger::{
    model::kind as ledger_kind,
    repository as ledger_repository,
    turn_tx::{TurnGate, TurnTx},
    turns,
};
use crate::shared::db::{blocking, Pool};
use crate::shared::error::{AppError, AppResult};

use super::{erase, model::RetryResult, submit};

async fn prepare_retry(turn: &TurnTx, entry_id: &str) -> AppResult<(String, String)> {
    turn.with(|conn| {
        let story_id = turn.story_id();
        let old_turn = turns::turn_of(conn, entry_id)?
            .ok_or_else(|| AppError::Invalid("entry has no owning turn".into()))?;
        if old_turn.story_id != story_id {
            return Err(AppError::Invalid(
                "entry does not belong to the requested story".into(),
            ));
        }
        let last = turns::last_turn(conn, story_id)?
            .ok_or_else(|| AppError::Invalid("story has no turn to retry".into()))?;
        if last.id != old_turn.id {
            return Err(AppError::Invalid(
                "only the latest turn can be retried".into(),
            ));
        }
        let player_id = erase::entries_for_turn(conn, &old_turn.id)?
            .into_iter()
            .find(|entry| entry.kind == ledger_kind::PLAYER_MESSAGE)
            .map(|entry| entry.id)
            .ok_or_else(|| AppError::Invalid("turn has no player action".into()))?;
        let player = ledger_repository::active_entry(conn, &player_id)?;
        let mode = player
            .payload
            .get("input_mode")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AppError::Invalid("player action has no input mode".into()))?
            .to_string();
        let content = player.content.unwrap_or_default();

        erase::remove_turn(conn, story_id, &old_turn.id)?;
        Ok((mode, content))
    })
    .await
}

pub(super) async fn retry_narration(
    app: AppHandle,
    pool: &Pool,
    gate: &TurnGate,
    story_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    let begin_pool = pool.clone();
    let begin_gate = gate.clone();
    let begin_story_id = story_id.clone();
    let turn = blocking(move || TurnTx::begin(&begin_pool, &begin_gate, &begin_story_id)).await?;
    let (mode, content) = match prepare_retry(&turn, &entry_id).await {
        Ok(input) => input,
        Err(error) => {
            if let Err(rollback_error) = turn.rollback().await {
                log::error!("failed to roll back retry: {rollback_error}");
            }
            return Err(error);
        }
    };

    let result = submit::run_turn(app, pool, Arc::clone(&turn), mode, content).await?;
    Ok(RetryResult {
        entry_id,
        stream_id: result.stream_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn retry_fixture() -> (Pool, String, String, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "mira",
            "s",
            "character",
            "Mira",
            Some("old cloak"),
            "test",
            None,
            None,
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let action = ledger_repository::append_story_message(
            &conn,
            "s",
            "player",
            "do",
            "Open the door",
            None,
            Some(&turn_id),
        )
        .unwrap();
        let reply = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "Original",
            None,
            Some(&turn_id),
        )
        .unwrap();
        crate::features::entities::update_entity_sync(
            &conn,
            "s",
            "mira",
            "Mira Changed",
            Some("new cloak"),
            "narrator_tool",
            Some(&reply.id),
            Some(&turn_id),
        )
        .unwrap();
        ledger_repository::append_entry(
            &conn,
            "s",
            ledger_kind::DICEROLL,
            "hidden",
            Some("Old roll"),
            &json!({"roll":99,"chance_percent":50,"seed":123,"outcome":"success"}),
            Some(&reply.id),
            Some(&turn_id),
        )
        .unwrap();
        drop(conn);
        (pool, action.id, reply.id, turn_id)
    }

    #[tokio::test]
    async fn edited_player_mode_and_content_are_used_before_erasing() {
        let (pool, action, reply, _) = retry_fixture();
        let conn = pool.get().unwrap();
        ledger_repository::append_entry(
            &conn,
            "s",
            ledger_kind::CONTENT_EDITED,
            "hidden",
            Some("I kick the door open"),
            &json!({"reason":"user_edit"}),
            Some(&action),
            None,
        )
        .unwrap();
        drop(conn);

        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let (mode, content) = prepare_retry(&turn, &reply).await.unwrap();
        assert_eq!(
            (mode.as_str(), content.as_str()),
            ("do", "I kick the door open")
        );
        turn.with(|conn| {
            assert!(turns::turn_of(conn, &reply)?.is_none());
            assert!(ledger_repository::get_entry(conn, &action).is_err());
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
        assert_eq!(
            ledger_repository::active_entry(&pool.get().unwrap(), &action)
                .unwrap()
                .content
                .as_deref(),
            Some("I kick the door open")
        );
    }

    #[tokio::test]
    async fn erasure_is_invisible_until_commit_and_rollback_restores_everything() {
        let (pool, action, reply, turn_id) = retry_fixture();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
                 VALUES ('old-image', ?1, 'old-image.png', 'prompt', 'now')",
                [&reply],
            )
            .unwrap();

        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        prepare_retry(&turn, &reply).await.unwrap();
        turn.with(|conn| {
            assert!(turns::last_turn(conn, "s")?.is_none());
            assert!(ledger_repository::get_entry(conn, &action).is_err());
            assert!(ledger_repository::get_entry(conn, &reply).is_err());
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                crate::features::entities::list_entities_sync(conn, "s", None)?[0].name,
                "Mira"
            );
            Ok(())
        })
        .await
        .unwrap();
        let visible = pool.get().unwrap();
        assert_eq!(
            turns::last_turn(&visible, "s").unwrap().unwrap().id,
            turn_id
        );
        assert_eq!(
            ledger_repository::active_entry(&visible, &reply)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
        assert_eq!(
            visible
                .query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(visible);

        turn.rollback().await.unwrap();
        let conn = pool.get().unwrap();
        assert_eq!(
            turns::last_turn(&conn, "s").unwrap().unwrap().status,
            turns::COMPLETE
        );
        assert_eq!(
            ledger_repository::active_entry(&conn, &reply)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
        assert!(ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None).unwrap()[0].name,
            "Mira Changed"
        );
    }

    #[tokio::test]
    async fn replacement_can_commit_a_new_turn_atomically() {
        let (pool, _, reply, old_turn) = retry_fixture();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let (mode, content) = prepare_retry(&turn, &reply).await.unwrap();
        turn.with(|conn| {
            let new_turn = turns::create_turn(conn, "s")?;
            ledger_repository::append_story_message(
                conn,
                "s",
                "player",
                &mode,
                &content,
                None,
                Some(&new_turn),
            )?;
            ledger_repository::append_story_message(
                conn,
                "s",
                "narrator",
                "generated",
                "New outcome",
                None,
                Some(&new_turn),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            turns::last_turn(&pool.get().unwrap(), "s")
                .unwrap()
                .unwrap()
                .id,
            old_turn
        );
        turn.commit().await.unwrap();
        let conn = pool.get().unwrap();
        assert!(ledger_repository::get_entry(&conn, &reply).is_err());
        assert!(ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.content.as_deref() == Some("New outcome")));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None).unwrap()[0].name,
            "Mira"
        );
    }

    #[tokio::test]
    async fn failed_unanswered_turn_can_be_replaced() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let action = ledger_repository::append_story_message(
            &conn,
            "s",
            "player",
            "continue",
            "",
            None,
            Some(&turn_id),
        )
        .unwrap();
        // A pre-migration failed turn remains retryable.
        conn.execute(
            "UPDATE turns SET status = 'failed' WHERE id = ?1",
            [&turn_id],
        )
        .unwrap();
        drop(conn);
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        assert_eq!(
            prepare_retry(&turn, &action.id).await.unwrap(),
            ("continue".into(), "".into())
        );
        turn.rollback().await.unwrap();
        assert_eq!(
            turns::turn_of(&pool.get().unwrap(), &action.id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
    }

    #[tokio::test]
    async fn see_retry_preserves_prior_scene_and_restores_old_turn_on_rollback() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('s', 'story', 'now', 'now', '{}')", []).unwrap();
        let scene_turn = turns::create_turn(&conn, "s").unwrap();
        let scene = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "Moonlit harbor",
            None,
            Some(&scene_turn),
        )
        .unwrap();
        let see_turn = turns::create_turn(&conn, "s").unwrap();
        let see = ledger_repository::append_story_message(
            &conn,
            "s",
            "player",
            "see",
            "",
            None,
            Some(&see_turn),
        )
        .unwrap();
        drop(conn);

        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        assert_eq!(
            prepare_retry(&turn, &see.id).await.unwrap(),
            ("see".into(), "".into())
        );
        turn.with(|conn| {
            assert!(ledger_repository::get_entry(conn, &scene.id).is_ok());
            assert!(ledger_repository::get_entry(conn, &see.id).is_err());
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
        assert!(ledger_repository::get_entry(&pool.get().unwrap(), &see.id).is_ok());
    }

    #[tokio::test]
    async fn invalid_entry_and_non_latest_turn_do_not_erase() {
        let (pool, _action, reply, _turn_id) = retry_fixture();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "different").unwrap();
        assert!(
            matches!(prepare_retry(&turn, &reply).await, Err(AppError::Invalid(message)) if message == "entry does not belong to the requested story")
        );
        turn.rollback().await.unwrap();
        drop(turn);

        turns::create_turn(&pool.get().unwrap(), "s").unwrap();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        assert!(
            matches!(prepare_retry(&turn, &reply).await, Err(AppError::Invalid(message)) if message == "only the latest turn can be retried")
        );
        turn.rollback().await.unwrap();
        drop(turn);

        assert!(ledger_repository::get_entry(&pool.get().unwrap(), &reply).is_ok());
    }
}
