use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::features::{
    images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        reducer, repository as ledger_repository,
        turn_tx::{TurnGate, TurnTx},
        turns,
    },
    narrator, stories,
};
use crate::prompts;
use crate::shared::db::{blocking, Pool};
use crate::shared::error::{AppError, AppResult};

use super::model::{NarrationDonePayload, SubmitTurnResult};
use narrator::{transcript::load_transcript, Candidate, NarratorInputs, NarratorPurpose};

#[derive(Clone, serde::Serialize)]
struct StoryTitleUpdatedPayload {
    story_id: String,
    title: String,
}

fn emit_image_results(
    app: &AppHandle,
    entry_id: &str,
    results: Vec<Result<images::model::StoryImage, ()>>,
) {
    for result in results {
        match result {
            Ok(image) => {
                let _ = app.emit("scene-image-generated", image);
            }
            Err(()) => {
                let _ = app.emit("scene-image-failed", entry_id);
            }
        }
    }
}

pub(super) async fn run_turn(
    app: AppHandle,
    pool: &Pool,
    turn: Arc<TurnTx>,
    mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    let result = prepare_and_spawn(app, pool, &turn, mode, content).await;
    if result.is_err() {
        turn.rollback().await?;
    }
    result
}

async fn prepare_and_spawn(
    app: AppHandle,
    pool: &Pool,
    turn: &Arc<TurnTx>,
    mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    let story_id = turn.story_id().to_string();
    let content = content.trim();
    if !prompts::TURN_MODES.contains(&mode.as_str()) {
        return Err(AppError::Invalid(format!("invalid turn mode: {mode}")));
    }
    if !matches!(mode.as_str(), "continue" | "see") && content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }

    let (turn_id, action, prior_narration, history) = turn
        .with(|conn| {
            let prior_narration = reducer::active_visible_entries(
                &ledger_repository::list_logical_entries(conn, &story_id)?,
            )
            .into_iter()
            .rev()
            .find(|entry| entry.kind == ledger_kind::NARRATION)
            .map(|entry| entry.id);
            if mode == "see" && prior_narration.is_none() {
                return Err(AppError::Invalid(
                    "there is no narrated scene to illustrate".into(),
                ));
            }
            let turn_id = turns::create_turn(conn, &story_id)?;
            let action = ledger_repository::append_story_message(
                conn,
                &story_id,
                "player",
                &mode,
                content,
                None,
                Some(&turn_id),
            )?;
            let history = load_transcript(conn, &story_id)?;
            Ok((turn_id, action, prior_narration, history))
        })
        .await?;

    let is_see = mode == "see";
    let target_entry_id = prior_narration
        .as_ref()
        .filter(|_| is_see)
        .cloned()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let prepared = narrator::prepare(NarratorInputs {
        app: &app,
        settings_pool: pool,
        turn: Arc::clone(turn),
        story_id: &story_id,
        transcript: history,
        purpose: if is_see {
            NarratorPurpose::Illustrate
        } else {
            NarratorPurpose::Action
        },
        target_entry_id: target_entry_id.clone(),
        turn_id: turn_id.clone(),
    })
    .await?;
    if !is_see {
        turn.with(|conn| {
            ledger_repository::append_placeholder_narration(
                conn,
                &story_id,
                &turn_id,
                target_entry_id.clone(),
            )?;
            Ok(())
        })
        .await?;
    }

    let live_pool = pool.clone();
    let turn_bg = Arc::clone(turn);
    let action_for_done = action.clone();
    let stream_id = narrator::spawn(prepared, move |app, sid, candidate| async move {
        let result = complete_turn(
            &app,
            &live_pool,
            &turn_bg,
            &turn_id,
            &target_entry_id,
            &action_for_done,
            prior_narration,
            is_see,
            sid,
            candidate,
        )
        .await;
        if result.is_err() {
            if let Err(error) = turn_bg.rollback().await {
                log::error!("failed to roll back narration turn: {error}");
            }
        }
        result
    });
    Ok(SubmitTurnResult {
        entry: action,
        stream_id,
    })
}

#[allow(clippy::too_many_arguments)]
async fn complete_turn(
    app: &AppHandle,
    pool: &Pool,
    turn: &TurnTx,
    turn_id: &str,
    target_entry_id: &str,
    action: &LedgerEntry,
    prior_narration: Option<String>,
    is_see: bool,
    stream_id: String,
    candidate: AppResult<Candidate>,
) -> AppResult<()> {
    let Candidate {
        visible,
        thoughts,
        image_requests,
    } = candidate?;
    if is_see {
        let request = image_requests.into_iter().next().ok_or_else(|| {
            AppError::Other("the narrator did not request an illustration".into())
        })?;
        let target_id = prior_narration
            .ok_or_else(|| AppError::Invalid("no narration to illustrate".into()))?;
        let target = images::ImageTarget {
            entry_id: target_id,
            source_action_id: Some(action.id.clone()),
            turn_id: turn_id.to_string(),
        };
        let results = images::generate_in_turn(app, pool, turn, &target, vec![request]).await;
        turn.commit().await?;
        let _ = app.emit(
            "narration-done",
            NarrationDonePayload {
                stream_id,
                entry: action.clone(),
            },
        );
        emit_image_results(app, &target.entry_id, results);
        return Ok(());
    }
    if visible.trim().is_empty() {
        return Err(AppError::Other(
            "the narrator generated no narration".into(),
        ));
    }
    let entry = turn
        .with(|conn| {
            ledger_repository::finish_narration(
                conn,
                target_entry_id,
                &visible,
                thoughts.as_deref(),
            )
        })
        .await?;
    let target = images::ImageTarget {
        entry_id: target_entry_id.to_string(),
        source_action_id: None,
        turn_id: turn_id.to_string(),
    };
    let (images, title) = tokio::join!(
        async {
            if image_requests.is_empty() {
                Vec::new()
            } else {
                images::generate_in_turn(app, pool, turn, &target, image_requests).await
            }
        },
        stories::title_in_turn(app, pool, turn),
    );
    turn.commit().await?;
    let _ = app.emit("narration-done", NarrationDonePayload { stream_id, entry });
    emit_image_results(app, &target.entry_id, images);
    if let Some(title) = title {
        let _ = app.emit(
            "story-title-updated",
            StoryTitleUpdatedPayload {
                story_id: turn.story_id().to_string(),
                title,
            },
        );
    }
    Ok(())
}

pub(super) async fn submit_turn(
    app: AppHandle,
    pool: &Pool,
    gate: &TurnGate,
    story_id: String,
    mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    let begin_pool = pool.clone();
    let begin_gate = gate.clone();
    let begin_story_id = story_id.clone();
    let turn = blocking(move || TurnTx::begin(&begin_pool, &begin_gate, &begin_story_id)).await?;
    run_turn(app, pool, turn, mode, content).await
}
