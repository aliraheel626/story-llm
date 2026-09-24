use std::future::Future;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::ai::{self, NarrateRequest, NarratorChunk, ToolActivityPhase};
use crate::features::compaction;
use crate::prompts;
use crate::shared::error::{AppError, AppResult};

use super::generation::Prepared;
use super::injection::combine_context_blocks;
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
            world_pool,
            story_id,
            config,
            transcript,
            context,
            tools: available_tools,
            stop_after_tool_result,
            reasoning_effort,
            before_seq,
            staging,
            image_requests,
        } = prepared;
        let result = async {
            let preamble = prompts::narrator_system_prompt();
            let mut history = compaction::prepare_history(
                &world_pool,
                &story_id,
                &config,
                &preamble,
                &context.full,
                transcript,
                before_seq,
            )
            .await
            .turns;
            let mut action = history.pop().ok_or_else(|| {
                AppError::Other("the narration history has no action turn".into())
            })?;
            if !action.is_player {
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
                        if turn.is_player { "player" } else { "narrator" },
                        &turn.content
                    ))
                    .collect::<Vec<_>>(),
                action.content,
            );
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
            let (visible, thoughts) = ai::stream_narration(req, move |chunk| match chunk {
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
            let thoughts = thoughts.trim();
            Ok(Candidate {
                visible: visible.trim().to_string(),
                thoughts: (!thoughts.is_empty()).then(|| thoughts.to_string()),
                staging,
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
