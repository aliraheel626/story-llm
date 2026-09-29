use std::future::Future;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::ai::{
    self, HistoryRole, HistoryTurnMarker, NarrateRequest, NarratorChunk, ToolActivityPhase,
};
use crate::features::{
    context::{self, combine_context_blocks, prepare_history},
    transcript::{model::kind as transcript_kind, repository as transcript_repository},
    usage::model::{UsageKind, UsageRecord},
};
use crate::prompts;
use crate::shared::error::{AppError, AppResult};

use super::generation::Prepared;
use super::model::Candidate;
use super::tools;

#[derive(Clone, Serialize)]
struct NarrationDeltaPayload<'a> {
    stream_id: &'a str,
    text: &'a str,
}

#[derive(Clone, Serialize)]
struct NarrationErrorPayload<'a> {
    stream_id: &'a str,
    message: &'a str,
}

#[derive(Clone, Serialize)]
struct NarrationToolActivityPayload<'a> {
    stream_id: &'a str,
    call_id: String,
    label: String,
    phase: &'static str,
    ok: Option<bool>,
}

fn emit_error(app: &AppHandle, stream_id: &str, error: &AppError) {
    let _ = app.emit(
        "narration-error",
        NarrationErrorPayload {
            stream_id,
            message: &error.to_string(),
        },
    );
}

/// Streams one candidate without persisting a reply or emitting narration-done.
pub fn spawn<F, Fut>(prepared: Prepared, on_candidate: F) -> String
where
    F: FnOnce(AppHandle, String, AppResult<Candidate>) -> Fut + Send + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let stream_id = Uuid::new_v4().to_string();
    let sid = stream_id.clone();
    tauri::async_runtime::spawn(async move {
        let Prepared {
            app,
            turn,
            story_id,
            target_entry_id,
            turn_id,
            config,
            transcript,
            context,
            tools: available_tools,
            stop_after_tool_result,
            reasoning_effort,
            image_requests,
        } = prepared;
        let result = async {
            let preamble = prompts::narrator_system_prompt();
            let carry_over = if transcript
                .first()
                .is_some_and(|turn| turn.marker == HistoryTurnMarker::Summary)
                && context::raw_tail_boundary(&transcript, &config, &preamble, &context.full) > 0
            {
                turn.with(|conn| Ok(context::latest_summary_artifact(conn, &story_id)))
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            let (prepared_history, summary_usage) = prepare_history(
                &story_id,
                &config,
                &preamble,
                &context.full,
                transcript,
                carry_over,
            )
            .await;
            for usage in summary_usage {
                turn.record_usage(UsageRecord::text(UsageKind::Summary, &config, usage));
            }
            if let Some(write) = &prepared_history.summary_write {
                let _ = turn.with(|conn| write.persist(conn, &story_id)).await;
            }
            let mut history = prepared_history.turns;
            let mut action = history.pop().ok_or_else(|| {
                AppError::Other("the narration history has no action turn".into())
            })?;
            if action.role != HistoryRole::Player {
                return Err(AppError::Other(
                    "the narration history does not end with an action turn".into(),
                ));
            }
            action.content = combine_context_blocks(&[context.live, action.content]);
            #[cfg(debug_assertions)]
            log::info!(
                "assembled narrator request: system={:?} history={:?} last_turn={:?}",
                preamble,
                history
                    .iter()
                    .map(|turn| (
                        match turn.role {
                            HistoryRole::Player => "player",
                            HistoryRole::Narrator => "narrator",
                            HistoryRole::Record => "record",
                        },
                        &turn.content
                    ))
                    .collect::<Vec<_>>(),
                action.content,
            );
            let usage_config = config.clone();
            let req = NarrateRequest {
                config,
                preamble,
                history,
                prompt: action.content,
                stop_after_tool_result,
                reasoning_effort,
                tools: available_tools,
            };

            let app_for_chunks = app.clone();
            let stream_id_for_chunks = sid.clone();
            let turn_for_usage = std::sync::Arc::clone(&turn);
            let (visible, thoughts, tool_calls) =
                ai::stream_narration(req, move |chunk| match chunk {
                    NarratorChunk::Usage(usage) => turn_for_usage.record_usage(UsageRecord::text(
                        UsageKind::Narration,
                        &usage_config,
                        usage,
                    )),
                    // A See turn keeps no text; Grok also writes its tool call out as text.
                    NarratorChunk::Text(_) if stop_after_tool_result => {}
                    NarratorChunk::Text(text) => {
                        let _ = app_for_chunks.emit(
                            "narration-delta",
                            NarrationDeltaPayload {
                                stream_id: &stream_id_for_chunks,
                                text: &text,
                            },
                        );
                    }
                    NarratorChunk::Reasoning(text) => {
                        let _ = app_for_chunks.emit(
                            "narration-thoughts",
                            NarrationDeltaPayload {
                                stream_id: &stream_id_for_chunks,
                                text: &text,
                            },
                        );
                    }
                    NarratorChunk::ToolActivity {
                        call_id,
                        tool_name,
                        args,
                        phase,
                    } => {
                        let (phase_str, ok) = match phase {
                            ToolActivityPhase::Started => ("started", None),
                            ToolActivityPhase::Finished { ok } => ("finished", Some(ok)),
                        };
                        let _ = app_for_chunks.emit(
                            "narration-tool-activity",
                            NarrationToolActivityPayload {
                                stream_id: &stream_id_for_chunks,
                                call_id,
                                label: tools::friendly_tool_label(&tool_name, &args),
                                phase: phase_str,
                                ok,
                            },
                        );
                    }
                })
                .await?;
            for call in tool_calls {
                let args_for_label = match &call.args {
                    serde_json::Value::String(raw) => raw.clone(),
                    value => value.to_string(),
                };
                let label = tools::friendly_tool_label(&call.tool, &args_for_label);
                turn.with(|conn| {
                    transcript_repository::append_entry(
                        conn,
                        &story_id,
                        transcript_kind::TOOL_CALL,
                        "hidden",
                        Some(&label),
                        &serde_json::json!({
                            "tool": call.tool, "args": call.args,
                            "result": call.result, "ok": call.ok,
                        }),
                        Some(&target_entry_id),
                        Some(&turn_id),
                    )?;
                    Ok(())
                })
                .await?;
            }
            let thoughts = thoughts.trim();
            Ok(Candidate {
                visible: visible.trim().to_string(),
                thoughts: (!thoughts.is_empty()).then(|| thoughts.to_string()),
                image_requests: std::mem::take(&mut *image_requests.lock().await),
            })
        }
        .await;

        if let Err(error) = on_candidate(app.clone(), sid.clone(), result).await {
            emit_error(&app, &sid, &error);
        }
    });
    stream_id
}
