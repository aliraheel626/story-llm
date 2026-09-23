use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::features::{
    images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        reducer, repository as ledger_repository,
    },
    narrator, stories,
};
use crate::prompts;
use crate::shared::db::Pool;
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
        &tx, story_id, "narrator", input_mode, visible, thoughts,
    )?;
    if let Some(guard) = staging_guard {
        guard.commit(&tx, &passage.id)?;
    }
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
    app: AppHandle,
    pool: Pool,
    story_id: String,
    action: LedgerEntry,
    mode: String,
    transcript: Vec<crate::ai::HistoryTurn>,
    before_seq: Option<i64>,
) -> AppResult<String> {
    let is_see = mode == "see";
    let prior_narration = {
        let conn = pool.get()?;
        let raw = ledger_repository::list_logical_entries(&conn, &story_id)?;
        reducer::active_visible_entries(&raw)
            .into_iter()
            .rev()
            .find(|entry| entry.kind == ledger_kind::NARRATION)
            .map(|entry| (entry.id, entry.content.unwrap_or_default()))
    };
    let prepared: narrator::Prepared = narrator::prepare(NarratorInputs {
        app: &app,
        settings_pool: &pool,
        world_pool: &pool,
        story_id: &story_id,
        transcript,
        before_seq,
        purpose: if is_see {
            NarratorPurpose::Illustrate
        } else {
            NarratorPurpose::Action
        },
    })?;
    if is_see && prior_narration.is_none() {
        return Err(AppError::Invalid(
            "there is no narrated scene to illustrate".into(),
        ));
    }
    let story_id_bg = story_id.clone();
    let action_for_done = action.clone();
    let source_action_id = action.id.clone();

    let stream_id = narrator::spawn(prepared, move |app, sid, candidate| async move {
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
                &target_id,
                &target_content,
                vec![request],
                Some(source_action_id),
            );
            return Ok(());
        }
        if visible.is_empty() {
            let _ = app.emit(
                "narration-done",
                NarrationDonePayload {
                    stream_id: sid,
                    entry: action_for_done,
                },
            );
            return Ok(());
        }
        let passage = append_narration_entry(
            &pool,
            &story_id_bg,
            "generated",
            &visible,
            thoughts.as_deref(),
            staging,
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
                &passage.id,
                &visible,
                image_requests,
                None,
            );
        }
        Ok(())
    });
    Ok(stream_id)
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

    let existing_trailing_action = if mode == "continue" {
        let conn = pool.get()?;
        match ledger_repository::last_active_entry(&conn, &story_id)?
            .filter(|entry| entry.role() == "player")
        {
            Some(entry) => {
                let has_completed_effect: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM ledger_entries
                     WHERE target_entry_id = ?1 AND kind = ?2)",
                    rusqlite::params![entry.id, ledger_kind::IMAGE_GENERATED],
                    |row| row.get(0),
                )?;
                (!has_completed_effect).then_some(entry)
            }
            None => None,
        }
    } else {
        None
    };
    let action = match existing_trailing_action {
        Some(action) => {
            let conn = pool.get()?;
            ledger_repository::active_entry(&conn, &action.id)?
        }
        None => {
            let conn = pool.get()?;
            let action = ledger_repository::append_story_message(
                &conn, &story_id, "player", &mode, content, None,
            )?;
            ledger_repository::active_entry(&conn, &action.id)?
        }
    };

    let history = load_transcript(pool, &story_id, None)?;
    let stream_id = start_action_generation(
        app,
        pool.clone(),
        story_id,
        action.clone(),
        mode,
        history,
        None,
    )?;
    Ok(SubmitTurnResult {
        entry: action,
        stream_id,
    })
}
