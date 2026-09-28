use std::sync::Arc;

use tauri::AppHandle;

use crate::features::turn::{TurnGate, TurnTx};
use crate::features::{
    entities,
    transcript::{erase, model::kind as transcript_kind, query, repository as transcript_repository, turns},
};
use crate::shared::db::{blocking, Pool};
use crate::shared::error::{AppError, AppResult};

use super::{model::RetryResult, submit};

async fn prepare_retry(
    turn: &TurnTx,
    entry_id: &str,
) -> AppResult<(String, String, Option<String>)> {
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
        let entries = query::entries_of_turn(conn, &old_turn.id)?;
        let player_id = entries
            .iter()
            .find(|entry| entry.kind == transcript_kind::PLAYER_MESSAGE)
            .map(|entry| entry.id.clone())
            .ok_or_else(|| AppError::Invalid("turn has no player action".into()))?;
        let player = transcript_repository::active_entry(conn, &player_id)?;
        let mode = player
            .payload
            .get("input_mode")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AppError::Invalid("player action has no input mode".into()))?
            .to_string();
        let content = player.content.unwrap_or_default();
        let rejected = entries
            .iter()
            .rev()
            .find(|entry| entry.kind == transcript_kind::NARRATION)
            .map(|entry| {
                transcript_repository::active_entry(conn, &entry.id)
                    .map(|active| active.content.unwrap_or_default())
            })
            .transpose()?;

        let removed = erase::remove_turn(conn, story_id, &old_turn.id)?;
        entities::projection::replay_after_erase(conn, story_id, &removed.entries)?;
        Ok((mode, content, rejected))
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
    let (mode, content, rejected) = match prepare_retry(&turn, &entry_id).await {
        Ok(input) => input,
        Err(error) => {
            if let Err(rollback_error) = turn.rollback().await {
                log::error!("failed to roll back retry: {rollback_error}");
            }
            return Err(error);
        }
    };

    let result = submit::run_turn(app, pool, Arc::clone(&turn), mode, content, rejected).await?;
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
        crate::shared::test_support::story(&conn, "s");
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
        let (turn_id, action_id, reply_id) = crate::shared::test_support::exchange(
            &conn,
            "s",
            "do",
            "Open the door",
            Some("Original"),
        );
        let reply_id = reply_id.unwrap();
        crate::features::entities::update_entity_sync(
            &conn,
            "s",
            "mira",
            "Mira Changed",
            Some("new cloak"),
            "narrator_tool",
            Some(&reply_id),
            Some(&turn_id),
        )
        .unwrap();
        crate::shared::test_support::record(
            &conn,
            "s",
            transcript_kind::DICEROLL,
            Some("Old roll"),
            json!({"roll":99,"chance_percent":50,"seed":123,"outcome":"success"}),
            Some(&reply_id),
            Some(&turn_id),
        );
        drop(conn);
        (pool, action_id, reply_id, turn_id)
    }

    #[tokio::test]
    async fn edited_player_mode_and_content_are_used_before_erasing() {
        let (pool, action, reply, _) = retry_fixture();
        let conn = pool.get().unwrap();
        crate::shared::test_support::record(
            &conn,
            "s",
            transcript_kind::CONTENT_EDITED,
            Some("I kick the door open"),
            json!({"reason":"user_edit"}),
            Some(&action),
            None,
        );
        crate::shared::test_support::record(
            &conn,
            "s",
            transcript_kind::CONTENT_EDITED,
            Some("An edited answer."),
            json!({"reason":"user_edit"}),
            Some(&reply),
            None,
        );
        drop(conn);

        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let (mode, content, rejected) = prepare_retry(&turn, &reply).await.unwrap();
        assert_eq!(
            (mode.as_str(), content.as_str(), rejected.as_deref()),
            ("do", "I kick the door open", Some("An edited answer."))
        );
        turn.with(|conn| {
            assert!(turns::turn_of(conn, &reply)?.is_none());
            assert!(transcript_repository::get_entry(conn, &action).is_err());
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
        assert_eq!(
            transcript_repository::active_entry(&pool.get().unwrap(), &action)
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
            assert!(transcript_repository::get_entry(conn, &action).is_err());
            assert!(transcript_repository::get_entry(conn, &reply).is_err());
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
            transcript_repository::active_entry(&visible, &reply)
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
            transcript_repository::active_entry(&conn, &reply)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
        assert!(transcript_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == transcript_kind::DICEROLL));
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
    async fn replay_failure_in_retry_restores_the_old_turn() {
        let (pool, action, reply, _) = retry_fixture();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        assert!(matches!(
            turn.with(|conn| {
                let old_turn = turns::turn_of(conn, &reply)?.unwrap();
                let removed = erase::remove_turn(conn, "s", &old_turn.id)?;
                assert!(!removed.entries.is_empty());
                Err::<(), _>(AppError::Other("replay failed".into()))
            }).await,
            Err(AppError::Other(message)) if message == "replay failed"
        ));
        turn.rollback().await.unwrap();
        let conn = pool.get().unwrap();
        assert!(transcript_repository::get_entry(&conn, &action).is_ok());
        assert!(transcript_repository::get_entry(&conn, &reply).is_ok());
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None).unwrap()[0].name,
            "Mira Changed"
        );
        gate.check_idle("s").unwrap();
    }

    #[tokio::test]
    async fn replacement_can_commit_a_new_turn_atomically() {
        let (pool, _, reply, old_turn) = retry_fixture();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let (mode, content, rejected) = prepare_retry(&turn, &reply).await.unwrap();
        assert_eq!(rejected.as_deref(), Some("Original"));
        turn.with(|conn| {
            let new_turn = turns::create_turn(conn, "s")?;
            transcript_repository::append_story_message(
                conn,
                "s",
                "player",
                &mode,
                &content,
                None,
                Some(&new_turn),
            )?;
            transcript_repository::append_story_message(
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
        assert!(transcript_repository::get_entry(&conn, &reply).is_err());
        assert!(transcript_repository::list_logical_entries(&conn, "s")
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
        crate::shared::test_support::story(&conn, "s");
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let action = transcript_repository::append_story_message(
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
            ("continue".into(), "".into(), None)
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
        crate::shared::test_support::story(&conn, "s");
        let scene_turn = turns::create_turn(&conn, "s").unwrap();
        let scene = transcript_repository::append_story_message(
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
        let see = transcript_repository::append_story_message(
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
            ("see".into(), "".into(), None)
        );
        turn.with(|conn| {
            assert!(transcript_repository::get_entry(conn, &scene.id).is_ok());
            assert!(transcript_repository::get_entry(conn, &see.id).is_err());
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
        assert!(transcript_repository::get_entry(&pool.get().unwrap(), &see.id).is_ok());
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

        assert!(transcript_repository::get_entry(&pool.get().unwrap(), &reply).is_ok());
    }
}
