use std::collections::HashSet;

use tauri::{AppHandle, Emitter};

use crate::features::{
    compaction, entities, images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        reducer, repository as ledger_repository, turns,
    },
    narrator,
};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::{
    erase::{delete_turn_images, entries_for_turn, touched_entities},
    model::{NarrationDonePayload, RetryResult},
};
use narrator::{transcript::load_transcript, Candidate, NarratorInputs, NarratorPurpose};

#[derive(Clone)]
struct RetryTurn {
    id: String,
    attempt: i64,
    player: LedgerEntry,
    original_narration: Option<LedgerEntry>,
    prior_narration: Option<(String, String)>,
    before_seq: i64,
    illustrate: bool,
}

struct CommittedRetry {
    entry: LedgerEntry,
    image_paths: Vec<String>,
    image_target: Option<images::ImageTarget>,
    image_requests: Vec<images::model::ImageRequest>,
}

fn begin_retry(pool: &Pool, story_id: &str, entry_id: &str) -> AppResult<RetryTurn> {
    with_transaction(pool, |tx| {
        let turn = turns::turn_of(tx, entry_id)?
            .ok_or_else(|| AppError::Invalid("entry has no owning turn".into()))?;
        if turn.story_id != story_id {
            return Err(AppError::Invalid(
                "entry does not belong to the requested story".into(),
            ));
        }
        let last = turns::last_turn(tx, story_id)?
            .ok_or_else(|| AppError::Invalid("story has no turn to retry".into()))?;
        if last.id != turn.id {
            return Err(AppError::Invalid(
                "only the latest turn can be retried".into(),
            ));
        }
        if turn.status == turns::PENDING {
            return Err(AppError::Invalid("a turn is already generating".into()));
        }

        let entries = entries_for_turn(tx, &turn.id)?;
        let player_id = entries
            .iter()
            .find(|entry| entry.kind == ledger_kind::PLAYER_MESSAGE)
            .map(|entry| entry.id.clone())
            .ok_or_else(|| AppError::Invalid("turn has no player action".into()))?;
        let player = ledger_repository::active_entry(tx, &player_id)?;
        let original_narration = entries
            .iter()
            .find(|entry| entry.kind == ledger_kind::NARRATION)
            .map(|entry| ledger_repository::active_entry(tx, &entry.id))
            .transpose()?;
        let illustrate = player
            .payload
            .get("input_mode")
            .and_then(serde_json::Value::as_str)
            == Some("see");
        let prior_narration = if illustrate {
            reducer::active_visible_entries(&ledger_repository::list_logical_entries(tx, story_id)?)
                .into_iter()
                .rev()
                .find(|entry| entry.kind == ledger_kind::NARRATION && entry.seq < player.seq)
                .map(|entry| (entry.id, entry.content.unwrap_or_default()))
        } else {
            None
        };
        let before_seq = original_narration
            .as_ref()
            .map_or(player.seq + 1, |reply| reply.seq);
        let attempt = turns::begin_attempt(tx, &turn.id)?;
        Ok(RetryTurn {
            id: turn.id,
            attempt,
            player,
            original_narration,
            prior_narration,
            before_seq,
            illustrate,
        })
    })
}

fn restore_turn(pool: &Pool, turn: &RetryTurn) {
    let result = with_transaction(pool, |tx| {
        let original_exists = match &turn.original_narration {
            Some(reply) => tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM ledger_entries WHERE id = ?1 AND turn_id = ?2)",
                rusqlite::params![reply.id, turn.id],
                |row| row.get::<_, bool>(0),
            )?,
            None => false,
        };
        tx.execute(
            "UPDATE turns SET status = ?1
             WHERE id = ?2 AND status = ?3 AND attempt = ?4",
            rusqlite::params![
                if original_exists {
                    turns::COMPLETE
                } else {
                    turns::FAILED
                },
                turn.id,
                turns::PENDING,
                turn.attempt
            ],
        )?;
        Ok(())
    });
    if let Err(error) = result {
        log::error!("failed to recover retry turn status: {error}");
    }
}

fn changed_during_retry() -> AppError {
    AppError::Invalid("story changed during retry; original narration was preserved".into())
}

fn transcript_for_retry(
    pool: &Pool,
    story_id: &str,
    turn: &RetryTurn,
) -> AppResult<Vec<crate::ai::HistoryTurn>> {
    load_transcript(pool, story_id, Some(turn.before_seq))
}

async fn commit_candidate(
    pool: &Pool,
    story_id: &str,
    turn: &RetryTurn,
    candidate: Candidate,
) -> AppResult<CommittedRetry> {
    let Candidate {
        visible,
        thoughts,
        staging,
        image_requests,
    } = candidate;
    let image_requests = if turn.illustrate {
        vec![image_requests.into_iter().next().ok_or_else(|| {
            AppError::Other("the narrator did not request an illustration".into())
        })?]
    } else {
        if visible.trim().is_empty() {
            return Err(AppError::Invalid(
                "retry generated no narration; original was preserved".into(),
            ));
        }
        image_requests
    };
    let illustrate_target = if turn.illustrate {
        Some(
            turn.prior_narration
                .clone()
                .ok_or_else(|| AppError::Invalid("no narration to illustrate".into()))?,
        )
    } else {
        None
    };
    let staging_guard = match &staging {
        Some(staging) => Some(staging.lock().await),
        None => None,
    };

    let (entry, image_paths) = with_transaction(pool, |tx| {
        let last = turns::last_turn(tx, story_id)?.ok_or_else(changed_during_retry)?;
        if last.id != turn.id || last.status != turns::PENDING || last.attempt != turn.attempt {
            return Err(changed_during_retry());
        }

        let entries = entries_for_turn(tx, &turn.id)?;
        let doomed = entries
            .into_iter()
            .filter(|entry| {
                entry.id != turn.player.id
                    && !(entry.kind == ledger_kind::CONTENT_EDITED
                        && entry.target_entry_id.as_deref() == Some(&turn.player.id))
            })
            .collect::<Vec<_>>();
        let image_paths = delete_turn_images(tx, &doomed)?;
        let doomed_ids = doomed
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<HashSet<_>>();
        let affected_entities = touched_entities(&doomed);
        compaction::prune_summaries_covering(tx, story_id, &doomed_ids)?;
        for entry in &doomed {
            tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [&entry.id])?;
        }
        entities::projection::replay(tx, story_id, &affected_entities, None)?;

        let entry = if turn.illustrate {
            if let Some(staging) = &staging_guard {
                let target_id = &illustrate_target
                    .as_ref()
                    .expect("validated illustration target")
                    .0;
                staging.commit(tx, target_id, &turn.id)?;
            }
            ledger_repository::active_entry(tx, &turn.player.id)?
        } else {
            let passage = ledger_repository::append_story_message(
                tx,
                story_id,
                "narrator",
                "generated",
                &visible,
                thoughts.as_deref(),
                Some(&turn.id),
            )?;
            if let Some(staging) = &staging_guard {
                staging.commit(tx, &passage.id, &turn.id)?;
            }
            ledger_repository::active_entry(tx, &passage.id)?
        };
        turns::set_status(tx, &turn.id, turns::COMPLETE)?;
        Ok((entry, image_paths))
    })?;

    let image_target = if image_requests.is_empty() {
        None
    } else if turn.illustrate {
        let (entry_id, expected_content) =
            illustrate_target.expect("validated illustration target");
        Some(images::ImageTarget {
            entry_id,
            expected_content,
            source_action_id: Some(turn.player.id.clone()),
            turn_id: turn.id.clone(),
            attempt: turn.attempt,
        })
    } else {
        Some(images::ImageTarget {
            entry_id: entry.id.clone(),
            expected_content: visible,
            source_action_id: None,
            turn_id: turn.id.clone(),
            attempt: turn.attempt,
        })
    };
    Ok(CommittedRetry {
        entry,
        image_paths,
        image_target,
        image_requests,
    })
}

async fn process_candidate(
    pool: &Pool,
    story_id: &str,
    turn: &RetryTurn,
    candidate: AppResult<Candidate>,
) -> AppResult<CommittedRetry> {
    let result = match candidate {
        Ok(candidate) => commit_candidate(pool, story_id, turn, candidate).await,
        Err(error) => Err(error),
    };
    if result.is_err() {
        restore_turn(pool, turn);
    }
    result
}

pub(super) async fn retry_narration(
    app: AppHandle,
    pool: &Pool,
    story_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    let turn = begin_retry(pool, &story_id, &entry_id)?;
    let prepared = (|| {
        if turn.illustrate && turn.prior_narration.is_none() {
            return Err(AppError::Invalid(
                "there is no narrated scene to illustrate".into(),
            ));
        }
        let world_pool = if turn.original_narration.is_some() {
            entities::view::excluding_turn(pool, &story_id, &turn.id)?
        } else {
            pool.clone()
        };
        let transcript = transcript_for_retry(pool, &story_id, &turn)?;
        narrator::prepare(NarratorInputs {
            app: &app,
            settings_pool: pool,
            world_pool: &world_pool,
            story_id: &story_id,
            transcript,
            before_seq: Some(turn.before_seq),
            purpose: if turn.illustrate {
                NarratorPurpose::Illustrate
            } else {
                NarratorPurpose::Action
            },
        })
    })();
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            restore_turn(pool, &turn);
            return Err(error);
        }
    };

    let live_pool = pool.clone();
    let story_id_bg = story_id.clone();
    let turn_bg = turn.clone();
    let stream_id = narrator::spawn(prepared, move |app, sid, candidate| async move {
        let committed = process_candidate(&live_pool, &story_id_bg, &turn_bg, candidate).await?;
        images::delete_assets(&committed.image_paths);
        let _ = app.emit(
            "narration-done",
            NarrationDonePayload {
                stream_id: sid,
                entry: committed.entry,
            },
        );
        if let Some(target) = committed.image_target {
            images::generate_from_narrator_requests(
                &app,
                &live_pool,
                target,
                committed.image_requests,
            );
        }
        Ok(())
    });

    Ok(RetryResult {
        entry_id,
        stream_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        narrator::{
            catalog::{self, ToolAvailability, ToolDeps},
            staging::TurnStaging,
        },
        stories,
    };
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn narrator_tools(
        staging: Arc<Mutex<TurnStaging>>,
    ) -> Vec<rig_agent::tool::PortableDynamicTool> {
        let settings = stories::settings::NarratorToolSettings::default();
        let specs = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: false,
            illustrate: false,
        });
        let deps = ToolDeps {
            staging: Some(staging),
            embedding_api_key: String::new(),
            image_requests: Arc::new(Mutex::new(Vec::new())),
        };
        specs.iter().map(|spec| (spec.build)(&deps)).collect()
    }

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
        crate::features::entities::update_entity_in_turn(
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
        turns::set_status(&conn, &turn_id, turns::COMPLETE).unwrap();
        (pool, action.id, reply.id, turn_id)
    }

    fn candidate(visible: &str, staging: Option<Arc<Mutex<TurnStaging>>>) -> Candidate {
        Candidate {
            visible: visible.into(),
            thoughts: None,
            staging,
            image_requests: Vec::new(),
        }
    }

    #[test]
    fn retry_transcript_uses_player_edit_appended_after_reply() {
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

        let turn = begin_retry(&pool, "s", &reply).unwrap();
        let transcript = transcript_for_retry(&pool, "s", &turn).unwrap();

        assert_eq!(
            transcript.last().map(|entry| entry.content.as_str()),
            Some("<do>I kick the door open</do>")
        );
        restore_turn(&pool, &turn);
    }

    #[tokio::test]
    async fn replacement_removes_old_roll_and_effects_but_keeps_action() {
        let (pool, action, reply, turn_id) = retry_fixture();
        let conn = pool.get().unwrap();
        ledger_repository::append_entry(
            &conn,
            "s",
            ledger_kind::CONTENT_EDITED,
            "hidden",
            Some("Open the iron door"),
            &json!({"reason":"user_edit"}),
            Some(&action),
            Some(&turn_id),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('old-image', ?1, 'old-image.png', 'prompt', 'now')",
            [&reply],
        )
        .unwrap();
        drop(conn);

        let turn = begin_retry(&pool, "s", &reply).unwrap();
        let committed = process_candidate(&pool, "s", &turn, Ok(candidate("New outcome", None)))
            .await
            .unwrap();
        assert_eq!(committed.image_paths, vec!["old-image.png"]);
        assert_ne!(committed.entry.id, reply);
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        assert!(entries.iter().any(|entry| entry.id == action));
        assert!(entries.iter().any(|entry| {
            entry.kind == ledger_kind::CONTENT_EDITED
                && entry.target_entry_id.as_deref() == Some(&action)
        }));
        assert!(!entries
            .iter()
            .any(|entry| entry.id == reply || entry.kind == ledger_kind::DICEROLL));
        assert_eq!(
            ledger_repository::active_entry(&conn, &action)
                .unwrap()
                .content
                .as_deref(),
            Some("Open the iron door")
        );
        assert_eq!(committed.entry.content.as_deref(), Some("New outcome"));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|entity| entity.id == "mira")
                .unwrap()
                .name,
            "Mira"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn retry_skips_player_attribute_event_for_entity_created_by_replaced_turn() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        ledger_repository::append_story_message(
            &conn,
            "s",
            "player",
            "do",
            "Enter the room",
            None,
            Some(&turn_id),
        )
        .unwrap();
        let reply = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "A guard appears.",
            None,
            Some(&turn_id),
        )
        .unwrap();
        entities::create_entity_with_id_in_turn(
            &conn,
            "guard",
            "s",
            "character",
            "Guard",
            None,
            "narrator_tool",
            Some(&reply.id),
            Some(&turn_id),
        )
        .unwrap();
        turns::set_status(&conn, &turn_id, turns::COMPLETE).unwrap();
        let health_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Health'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        entities::attributes::set_entity_attribute_sync(&conn, "s", "guard", &health_id, 7.0)
            .unwrap();
        drop(conn);

        let turn = begin_retry(&pool, "s", &reply.id).unwrap();
        let committed =
            process_candidate(&pool, "s", &turn, Ok(candidate("The room is empty.", None)))
                .await
                .unwrap();

        assert_eq!(
            committed.entry.content.as_deref(),
            Some("The room is empty.")
        );
        let conn = pool.get().unwrap();
        let counts: (i64, i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM entities WHERE id = 'guard'),
                    (SELECT COUNT(*) FROM story_entity_state WHERE entity_id = 'guard'),
                    (SELECT COUNT(*) FROM entity_attributes WHERE entity_id = 'guard')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0, 0));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::ENTITY_ATTRIBUTE_CHANGED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            turns::turn_of(&conn, &committed.entry.id)
                .unwrap()
                .unwrap()
                .status,
            turns::COMPLETE
        );
    }

    #[tokio::test]
    async fn empty_generation_keeps_the_original_outcome() {
        let (pool, _, reply, turn_id) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        assert!(
            process_candidate(&pool, "s", &turn, Ok(candidate("  ", None)))
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
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
            turns::turn_of(&conn, &reply).unwrap().unwrap().status,
            turns::COMPLETE
        );
        assert_eq!(turn.id, turn_id);
    }

    #[tokio::test]
    async fn candidate_error_keeps_the_original_outcome() {
        let (pool, _, reply, _) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        assert!(process_candidate(
            &pool,
            "s",
            &turn,
            Err(AppError::Other("generation failed".into())),
        )
        .await
        .is_err());
        let conn = pool.get().unwrap();
        assert_eq!(
            ledger_repository::active_entry(&conn, &reply)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
        assert_eq!(
            turns::turn_of(&conn, &reply).unwrap().unwrap().status,
            turns::COMPLETE
        );
    }

    #[test]
    fn duplicate_pending_retry_is_rejected() {
        let (pool, _, reply, _) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        let error = match begin_retry(&pool, "s", &reply) {
            Err(error) => error,
            Ok(_) => panic!("a pending turn started a duplicate retry"),
        };
        assert!(matches!(
            error,
            AppError::Invalid(message) if message == "a turn is already generating"
        ));
        restore_turn(&pool, &turn);
    }

    #[tokio::test]
    async fn adding_a_turn_mid_retry_rejects_commit_and_preserves_the_original() {
        let (pool, _, reply, turn_id) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO turns (id, story_id, seq, status, created_at)
                 VALUES ('later', 's', 1, 'complete', 'now')",
                [],
            )
            .unwrap();
        let error = match process_candidate(&pool, "s", &turn, Ok(candidate("New", None))).await {
            Err(error) => error,
            Ok(_) => panic!("a changed story accepted a retry candidate"),
        };
        assert!(matches!(
            error,
            AppError::Invalid(message)
                if message == "story changed during retry; original narration was preserved"
        ));
        let conn = pool.get().unwrap();
        assert_eq!(
            ledger_repository::active_entry(&conn, &reply)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
        assert_eq!(
            turns::turn_of(&conn, &reply).unwrap().unwrap().status,
            turns::COMPLETE
        );
        assert_eq!(turn.id, turn_id);
    }

    #[tokio::test]
    async fn a_newer_attempt_rejects_stale_retry_commit_and_stays_pending() {
        let (pool, _, reply, turn_id) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        pool.get()
            .unwrap()
            .execute(
                "UPDATE turns SET attempt = attempt + 1 WHERE id = ?1",
                [&turn_id],
            )
            .unwrap();

        let error = match process_candidate(&pool, "s", &turn, Ok(candidate("Stale", None))).await {
            Err(error) => error,
            Ok(_) => panic!("a stale retry attempt committed"),
        };

        assert!(matches!(
            error,
            AppError::Invalid(message)
                if message == "story changed during retry; original narration was preserved"
        ));
        let current = turns::last_turn(&pool.get().unwrap(), "s")
            .unwrap()
            .unwrap();
        assert_eq!(current.status, turns::PENDING);
        assert_eq!(current.attempt, turn.attempt + 1);
        assert_eq!(
            ledger_repository::active_entry(&pool.get().unwrap(), &reply)
                .unwrap()
                .content
                .as_deref(),
            Some("Original")
        );
    }

    #[tokio::test]
    async fn erasing_the_turn_mid_retry_rejects_commit() {
        let (pool, _, reply, turn_id) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        pool.get()
            .unwrap()
            .execute("DELETE FROM turns WHERE id = ?1", [&turn_id])
            .unwrap();
        let error = match process_candidate(&pool, "s", &turn, Ok(candidate("New", None))).await {
            Err(error) => error,
            Ok(_) => panic!("an erased turn accepted a retry candidate"),
        };
        assert!(matches!(
            error,
            AppError::Invalid(message)
                if message == "story changed during retry; original narration was preserved"
        ));
        assert!(ledger_repository::get_entry(&pool.get().unwrap(), &reply).is_err());
    }

    #[tokio::test]
    async fn failed_unanswered_turn_uses_the_same_commit_path() {
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
        turns::set_status(&conn, &turn_id, turns::FAILED).unwrap();
        drop(conn);

        let turn = begin_retry(&pool, "s", &action.id).unwrap();
        assert!(turn.original_narration.is_none());
        let committed = process_candidate(&pool, "s", &turn, Ok(candidate("Continued", None)))
            .await
            .unwrap();
        assert_eq!(committed.entry.content.as_deref(), Some("Continued"));
        assert_eq!(
            turns::turn_of(&pool.get().unwrap(), &action.id)
                .unwrap()
                .unwrap()
                .status,
            turns::COMPLETE
        );
    }

    #[tokio::test]
    async fn retry_stages_fresh_roll_and_entity_changes_against_the_pre_turn_view() {
        let (pool, _, reply, _) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        let world = entities::view::excluding_turn(&pool, "s", &turn.id).unwrap();
        let staging = Arc::new(Mutex::new(TurnStaging::new(world, "s".into())));
        let tool_set = narrator_tools(staging.clone());
        let named = |name: &str| tool_set.iter().find(|tool| tool.name() == name).unwrap();
        let found = named("get_entities")
            .execute(json!({"name":"Mira"}))
            .await
            .unwrap();
        assert_eq!(
            found.as_json().unwrap()["entities"][0]["name"],
            json!("Mira")
        );
        named("roll_check")
            .execute(json!({"chance_percent":100,"reason":"retry"}))
            .await
            .unwrap();
        named("update_entity")
            .execute(json!({"id":"mira", "name":"Mira Retried"}))
            .await
            .unwrap();
        named("create_entity")
            .execute(json!({"kind":"location", "name":"New chamber"}))
            .await
            .unwrap();

        let committed =
            process_candidate(&pool, "s", &turn, Ok(candidate("Retried", Some(staging))))
                .await
                .unwrap();
        let conn = pool.get().unwrap();
        let rolls = ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .into_iter()
            .filter(|entry| entry.kind == ledger_kind::DICEROLL)
            .collect::<Vec<_>>();
        assert_eq!(rolls.len(), 1);
        assert_eq!(
            rolls[0].target_entry_id.as_deref(),
            Some(committed.entry.id.as_str())
        );
        assert_eq!(rolls[0].payload["chance_percent"], json!(100));
        let entities = crate::features::entities::list_entities_sync(&conn, "s", None).unwrap();
        assert_eq!(
            entities
                .iter()
                .find(|entity| entity.id == "mira")
                .unwrap()
                .name,
            "Mira Retried"
        );
        assert!(entities.iter().any(|entity| entity.name == "New chamber"));
    }

    #[tokio::test]
    async fn retry_minted_attribute_commits_to_the_live_registry() {
        let (pool, _, reply, _) = retry_fixture();
        crate::features::entities::create_entity_with_id_sync(
            &pool.get().unwrap(),
            "storm",
            "s",
            "weather",
            "Storm",
            None,
            "test",
            None,
        )
        .unwrap();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        let world = entities::view::excluding_turn(&pool, "s", &turn.id).unwrap();
        let staging = Arc::new(Mutex::new(TurnStaging::new(world, "s".into())));
        let tool_set = narrator_tools(staging.clone());
        tool_set
            .iter()
            .find(|tool| tool.name() == "adjust_entity_attribute")
            .unwrap()
            .execute(json!({
                "entity_id":"storm",
                "attribute":"Barometric Whim",
                "delta":2.0,
                "reason":"the storm intensifies"
            }))
            .await
            .unwrap();

        process_candidate(
            &pool,
            "s",
            &turn,
            Ok(candidate("The storm intensifies.", Some(staging))),
        )
        .await
        .unwrap();
        let conn = pool.get().unwrap();
        let attribute_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Barometric Whim'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entity_attributes
                 WHERE story_id = 's' AND entity_id = 'storm' AND attribute_id = ?1",
                [&attribute_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn story_settings_changes_do_not_block_retry_commit() {
        let (pool, _, reply, _) = retry_fixture();
        let turn = begin_retry(&pool, "s", &reply).unwrap();
        pool.get()
            .unwrap()
            .execute(
                "UPDATE stories SET settings_json = '{\"author_note\":\"new guidance\"}' WHERE id = 's'",
                [],
            )
            .unwrap();
        let committed = process_candidate(&pool, "s", &turn, Ok(candidate("New", None)))
            .await
            .unwrap();
        assert_eq!(committed.entry.content.as_deref(), Some("New"));
    }

    #[tokio::test]
    async fn see_retry_reuses_the_prior_narration_target_without_adding_narration() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
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
        turns::set_status(&conn, &scene_turn, turns::COMPLETE).unwrap();
        let see_turn = turns::create_turn(&conn, "s").unwrap();
        let action = ledger_repository::append_story_message(
            &conn,
            "s",
            "player",
            "see",
            "",
            None,
            Some(&see_turn),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('old-image', ?1, 'old-see.png', 'prompt', 'now')",
            [&scene.id],
        )
        .unwrap();
        ledger_repository::append_entry(
            &conn,
            "s",
            ledger_kind::IMAGE_GENERATED,
            "hidden",
            Some("old image"),
            &json!({"asset_id":"old-image","prompt":"prompt"}),
            Some(&action.id),
            Some(&see_turn),
        )
        .unwrap();
        turns::set_status(&conn, &see_turn, turns::COMPLETE).unwrap();
        drop(conn);

        let turn = begin_retry(&pool, "s", &action.id).unwrap();
        let mut staging = TurnStaging::new(pool.clone(), "s".into());
        staging.stage_tool_call(
            "illustrate_scene".into(),
            json!({"description":"A moonlit harbor"}),
            json!({"queued":true}),
            true,
            "Sketching the scene".into(),
        );
        let committed = process_candidate(
            &pool,
            "s",
            &turn,
            Ok(Candidate {
                visible: String::new(),
                thoughts: None,
                staging: Some(Arc::new(Mutex::new(staging))),
                image_requests: vec![images::model::ImageRequest {
                    description: "A moonlit harbor".into(),
                    character_ids: Vec::new(),
                }],
            }),
        )
        .await
        .unwrap();
        assert_eq!(committed.entry.id, action.id);
        assert_eq!(committed.image_paths, vec!["old-see.png"]);
        let target = committed.image_target.unwrap();
        assert_eq!(target.entry_id, scene.id);
        assert_eq!(target.expected_content, "Moonlit harbor");
        assert_eq!(target.source_action_id.as_deref(), Some(action.id.as_str()));
        assert_eq!(target.turn_id, see_turn);
        assert_eq!(target.attempt, 1);
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM ledger_entries WHERE story_id = 's' AND kind = 'narration'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        let conn = pool.get().unwrap();
        let (target, owner): (String, String) = conn
            .query_row(
                "SELECT target_entry_id, turn_id FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::TOOL_CALL],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(target, scene.id);
        assert_eq!(owner, see_turn);
    }
}
