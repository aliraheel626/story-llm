use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::features::{
    images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        reducer, repository as ledger_repository, turns,
    },
    narrator, stories,
};
use crate::prompts;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::model::{NarrationDonePayload, SubmitTurnResult};
use narrator::{
    staging::TurnStaging, transcript::load_transcript, Candidate, NarratorInputs, NarratorPurpose,
};

/// Inserts a fresh narration entry. If the narrator's tool calls staged any
/// entity/attribute/roll writes this turn, they're committed in the same
/// transaction — atomically with the passage, and not at all if anything
/// above this point failed first.
async fn append_narration_entry(
    pool: &Pool,
    story_id: &str,
    input_mode: &str,
    visible: &str,
    thoughts: Option<&str>,
    staging: Option<Arc<Mutex<TurnStaging>>>,
    turn_id: &str,
) -> AppResult<LedgerEntry> {
    // Acquired before the transaction opens (not held across an `.await`
    // with it live) so the whole rest of this function stays synchronous —
    // a `rusqlite::Transaction` isn't `Send`, so awaiting anything while one
    // is alive would make this future unusable from `narrator::spawn`.
    let staging_guard = match &staging {
        Some(s) => Some(s.lock().await),
        None => None,
    };
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let passage = ledger_repository::append_story_message(
        &tx,
        story_id,
        "narrator",
        input_mode,
        visible,
        thoughts,
        Some(turn_id),
    )?;
    if let Some(guard) = staging_guard {
        guard.commit(&tx, &passage.id, turn_id)?;
    }
    turns::set_status(&tx, turn_id, turns::COMPLETE)?;
    tx.commit()?;
    Ok(passage)
}

/// Best-effort kick-off of the ChatGPT/Gemini-style auto-title: once a story
/// has its first passage, ask the text model to name it. Never blocks or
/// fails the passage write — generation is fire-and-forget and reports back
/// via the `story-title-updated` event (see `stories::maybe_auto_title`).
fn kick_auto_title(app: &AppHandle, pool: &Pool, story_id: &str) {
    stories::maybe_auto_title(app, pool, story_id);
}

pub(super) fn start_action_generation(
    pool: Pool,
    story_id: String,
    action: LedgerEntry,
    turn_id: String,
    is_see: bool,
    prior_narration: Option<(String, String)>,
    prepared: narrator::Prepared,
) -> String {
    let story_id_bg = story_id.clone();
    let action_for_done = action.clone();
    let source_action_id = action.id.clone();
    let turn_id_for_done = turn_id.clone();

    narrator::spawn(prepared, move |app, sid, candidate| async move {
        let completion = async {
            let candidate = candidate?;
            let Candidate {
                visible,
                thoughts,
                staging,
                image_requests,
            } = candidate;
            if is_see {
                let request = image_requests.into_iter().next();
                let Some(request) = request else {
                    return Err(AppError::Other(
                        "the narrator did not request an illustration".into(),
                    ));
                };
                let (target_id, target_content) = prior_narration
                    .ok_or_else(|| AppError::Invalid("no narration to illustrate".into()))?;
                {
                    let staging_guard = match &staging {
                        Some(staging) => Some(staging.lock().await),
                        None => None,
                    };
                    let mut conn = pool.get()?;
                    let tx = conn.transaction()?;
                    if let Some(staging) = staging_guard {
                        staging.commit(&tx, &target_id, &turn_id_for_done)?;
                    }
                    turns::set_status(&tx, &turn_id_for_done, turns::COMPLETE)?;
                    tx.commit()?;
                }
                let _ = app.emit(
                    "narration-done",
                    NarrationDonePayload {
                        stream_id: sid,
                        entry: action_for_done,
                    },
                );
                images::generate_from_narrator_requests(
                    &app,
                    &pool,
                    images::ImageTarget {
                        entry_id: target_id,
                        expected_content: target_content,
                        source_action_id: Some(source_action_id),
                        turn_id: turn_id_for_done.clone(),
                    },
                    vec![request],
                );
                return Ok(());
            }
            if visible.is_empty() {
                return Err(AppError::Other(
                    "the narrator generated no narration".into(),
                ));
            }
            let passage = append_narration_entry(
                &pool,
                &story_id_bg,
                "generated",
                &visible,
                thoughts.as_deref(),
                staging,
                &turn_id_for_done,
            )
            .await?;
            let entry = {
                let conn = pool.get()?;
                ledger_repository::active_entry(&conn, &passage.id)?
            };
            let _ = app.emit(
                "narration-done",
                NarrationDonePayload {
                    stream_id: sid,
                    entry,
                },
            );
            kick_auto_title(&app, &pool, &story_id_bg);
            if !image_requests.is_empty() {
                images::generate_from_narrator_requests(
                    &app,
                    &pool,
                    images::ImageTarget {
                        entry_id: passage.id,
                        expected_content: visible,
                        source_action_id: None,
                        turn_id: turn_id_for_done.clone(),
                    },
                    image_requests,
                );
            }
            Ok(())
        }
        .await;
        if completion.is_err() {
            if let Ok(conn) = pool.get() {
                if let Err(error) = turns::set_status(&conn, &turn_id_for_done, turns::FAILED) {
                    log::error!("failed to mark turn failed: {error}");
                }
            }
        }
        completion
    })
}

pub(super) async fn submit_turn(
    app: AppHandle,
    pool: &Pool,
    story_id: String,
    mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    if !prompts::TURN_MODES.contains(&mode.as_str()) {
        return Err(AppError::Invalid(format!("invalid turn mode: {mode}")));
    }
    let content = content.trim();
    if !matches!(mode.as_str(), "continue" | "see") && content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }

    let failed_turn = if mode == "continue" {
        let conn = pool.get()?;
        turns::last_turn(&conn, &story_id)?.filter(|turn| turn.status == turns::FAILED)
    } else {
        None
    };
    let existing_action = if let Some(turn) = &failed_turn {
        let conn = pool.get()?;
        let entry_id: String = conn
            .query_row(
                "SELECT id FROM ledger_entries
                 WHERE turn_id = ?1 AND kind = ?2 ORDER BY seq ASC LIMIT 1",
                rusqlite::params![turn.id, ledger_kind::PLAYER_MESSAGE],
                |row| row.get(0),
            )
            .map_err(|_| AppError::Invalid("failed turn has no player action".into()))?;
        Some(ledger_repository::active_entry(&conn, &entry_id)?)
    } else {
        None
    };
    let generation_mode = existing_action
        .as_ref()
        .and_then(|action| action.payload.get("input_mode"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&mode);
    let prior_narration = {
        let conn = pool.get()?;
        let raw = ledger_repository::list_logical_entries(&conn, &story_id)?;
        reducer::active_visible_entries(&raw)
            .into_iter()
            .rev()
            .find(|entry| entry.kind == ledger_kind::NARRATION)
            .map(|entry| (entry.id, entry.content.unwrap_or_default()))
    };
    if generation_mode == "see" && prior_narration.is_none() {
        return Err(AppError::Invalid(
            "there is no narrated scene to illustrate".into(),
        ));
    }

    let mut history = load_transcript(pool, &story_id, None)?;
    if existing_action.is_none() {
        history.push(crate::ai::HistoryTurn {
            entry_id: None,
            is_player: true,
            content: prompts::render_turn(&mode, content).unwrap_or_else(|| content.to_string()),
            marker: crate::ai::HistoryTurnMarker::Ledger,
        });
    }
    let prepared = narrator::prepare(NarratorInputs {
        app: &app,
        settings_pool: pool,
        world_pool: pool,
        story_id: &story_id,
        transcript: history,
        before_seq: None,
        purpose: if generation_mode == "see" {
            NarratorPurpose::Illustrate
        } else {
            NarratorPurpose::Action
        },
    })?;

    let (turn_id, action) = with_transaction(pool, |tx| {
        if let (Some(turn), Some(action)) = (&failed_turn, &existing_action) {
            turns::set_status(tx, &turn.id, turns::PENDING)?;
            return Ok((turn.id.clone(), action.clone()));
        }
        let turn_id = turns::create_turn(tx, &story_id)?;
        let action = ledger_repository::append_story_message(
            tx,
            &story_id,
            "player",
            &mode,
            content,
            None,
            Some(&turn_id),
        )?;
        Ok((turn_id, action))
    })?;

    let stream_id = start_action_generation(
        pool.clone(),
        story_id,
        action.clone(),
        turn_id,
        generation_mode == "see",
        prior_narration,
        prepared,
    );
    Ok(SubmitTurnResult {
        entry: action,
        stream_id,
    })
}
