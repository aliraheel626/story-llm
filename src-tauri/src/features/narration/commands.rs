use std::{collections::HashSet, sync::Arc};

use chrono::Utc;
use rig_agent::tool::DynamicTool;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::ai::{
    self, HistoryTurn, NarrateRequest, NarratorChunk, TextModelConfig, ToolActivityPhase,
};
use crate::features::{
    compaction,
    dicerolls::{commands as diceroll_settings, model::DiceMode},
    images, settings, stories,
    timeline::{
        model::{kind as timeline_kind, NarrationVariant, TimelineEntry},
        reducer, repository as timeline_repository,
    },
};
use crate::prompts;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::author_note;
use super::entity_context::{build_entity_context, EntityContextPlan};
use super::history::load_history;
use super::model::ActiveStoryEntry;
use super::repository::{get_last_story_entry, image_paths_for_entry, insert_story_entry};
use super::tools::{self, TurnStaging};

fn narrator_image_tools(
    enabled: bool,
) -> (
    Vec<DynamicTool>,
    Arc<Mutex<Vec<images::model::ImageRequest>>>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let tools = if enabled {
        vec![DynamicTool::from_portable(tools::illustrate_scene_tool(
            requests.clone(),
        ))]
    } else {
        Vec::new()
    };
    (tools, requests)
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitTurnResult {
    pub entry: TimelineEntry,
    pub stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetryResult {
    pub entry_id: String,
    pub stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct NarrationDeltaPayload<'a> {
    stream_id: &'a str,
    text: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct NarrationDonePayload {
    stream_id: String,
    entry: TimelineEntry,
}

#[derive(Debug, Clone, Serialize)]
struct SwipeDonePayload {
    stream_id: String,
    entry: TimelineEntry,
    variants: Vec<NarrationVariant>,
}

#[derive(Debug, Clone, Serialize)]
struct NarrationErrorPayload<'a> {
    stream_id: &'a str,
    message: &'a str,
}

/// Retry/swipe variant payloads share this shape. Reasoning rides along for
/// display only — like a narration entry's, it is never fed back as context.
fn variant_payload(reason: &str, input_mode: &str, thoughts: Option<&str>) -> serde_json::Value {
    match thoughts.map(str::trim).filter(|t| !t.is_empty()) {
        Some(thoughts) => {
            serde_json::json!({"reason": reason, "input_mode": input_mode, "thoughts": thoughts})
        }
        None => serde_json::json!({"reason": reason, "input_mode": input_mode}),
    }
}

fn build_turn_context(
    pool: &Pool,
    story_id: &str,
    history: &[HistoryTurn],
    config: &TextModelConfig,
    extra_instructions: &str,
) -> AppResult<EntityContextPlan> {
    let current_note = author_note::current_for_cut(pool, story_id, None)?;
    let preamble = prompts::narrator_system_prompt(current_note.as_deref());
    let entity_context = build_entity_context(pool, story_id, history, config, &preamble)?;
    Ok(EntityContextPlan {
        live_context: Some(combine_context_blocks(&[
            entity_context.live_context.unwrap_or_default(),
            extra_instructions.to_string(),
        ])),
        full_context: Some(combine_context_blocks(&[
            entity_context.full_context.unwrap_or_default(),
            extra_instructions.to_string(),
        ])),
    })
}

pub(super) fn combine_context_blocks(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The dice roll that produced `entry_id`'s narration, if any — folded
/// into retry/swipe's turn context so regenerating a roll-driven turn stays
/// consistent with the outcome that already happened. The roll's timeline
/// seq is assigned after the narration it explains, so it falls outside the
/// history window a retry/swipe deliberately cuts off at the target's seq;
/// this is the only channel that outcome reaches the regenerated call by.
fn roll_context_block(pool: &Pool, entry_id: &str) -> AppResult<String> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT content FROM timeline_entries WHERE target_entry_id = ?1 AND kind = ?2 ORDER BY seq ASC",
    )?;
    let contents = stmt
        .query_map(
            rusqlite::params![entry_id, timeline_kind::DICEROLL],
            |row| row.get::<_, Option<String>>(0),
        )?
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if contents.is_empty() {
        String::new()
    } else {
        format!("<rolls>\n- {}\n</rolls>", contents.join("\n- "))
    })
}

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
) -> AppResult<ActiveStoryEntry> {
    // Acquired before the transaction opens (not held across an `.await`
    // with it live) so the whole rest of this function stays synchronous —
    // a `rusqlite::Transaction` isn't `Send`, so awaiting anything while one
    // is alive would make this future unusable from `spawn_narration`.
    let staging_guard = match &staging {
        Some(s) => Some(s.lock().await),
        None => None,
    };
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let passage = insert_story_entry(&tx, story_id, "narrator", input_mode, visible, thoughts)?;
    if let Some(guard) = staging_guard {
        guard.commit(&tx, &passage.id)?;
    }
    tx.commit()?;
    Ok(passage)
}

/// Appends a generated variant only after generation has succeeded.
/// The base narration remains immutable and keeps its chronological position.
fn append_variant(
    pool: &Pool,
    target: &ActiveStoryEntry,
    visible: &str,
    thoughts: Option<&str>,
    reason: &str,
) -> AppResult<TimelineEntry> {
    let (image_paths, entry) = with_transaction(pool, |tx| {
        let image_paths = image_paths_for_entry(tx, &target.id)?;
        tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [&target.id])?;
        tx.execute(
            "DELETE FROM timeline_entries WHERE kind = ?1 AND target_entry_id = ?2",
            rusqlite::params![timeline_kind::IMAGE_GENERATED, target.id],
        )?;
        timeline_repository::append_entry(
            tx,
            &target.story_id,
            timeline_kind::NARRATION_VARIANT,
            "hidden",
            Some(visible),
            &variant_payload(reason, &target.input_mode, thoughts),
            Some(&target.id),
        )?;
        Ok((
            image_paths,
            timeline_repository::active_entry(tx, &target.id)?,
        ))
    })?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    Ok(entry)
}

/// Best-effort kick-off of the ChatGPT/Gemini-style auto-title: once a story
/// has its first passage, ask the text model to name it. Never blocks or
/// fails the passage write — generation is fire-and-forget and reports back
/// via the `story-title-updated` event (see `stories::maybe_auto_title`).
fn kick_auto_title(app: &AppHandle, pool: &Pool, story_id: &str) {
    stories::maybe_auto_title(app, pool, story_id);
}

/// Spawns the background narration stream shared by every path that produces
/// or updates a narrator passage without blocking the command's return:
/// `narration-delta` / `narration-thoughts` fire as text arrives, then
/// `on_success` runs with the final text (and should emit its own terminal
/// event), or `narration-error` fires if the stream or `on_success` fails.
/// `turn_context_plan` folds Stage 1/2 context (attribute snapshot, roll
/// outcome constraint) into the final persisted action turn.
struct NarrationJob {
    app: AppHandle,
    pool: Pool,
    story_id: String,
    config: TextModelConfig,
    history: Vec<HistoryTurn>,
    turn_context_plan: EntityContextPlan,
    /// Live tools available to this narration path. Revision jobs receive only
    /// the image tool, never state-changing entity or dice tools.
    tools: Vec<DynamicTool>,
    stop_after_tool_result: bool,
    reasoning_effort: Option<String>,
    before_seq: Option<i64>,
    stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct NarrationToolActivityPayload<'a> {
    stream_id: &'a str,
    call_id: String,
    label: String,
    phase: &'static str,
    /// `None` while starting; `Some(false)` lets the frontend show a tool
    /// call didn't succeed instead of just quietly disappearing.
    ok: Option<bool>,
}

fn spawn_narration<F, Fut>(job: NarrationJob, on_success: F)
where
    F: FnOnce(AppHandle, Pool, String, String, Option<String>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = AppResult<()>> + Send + 'static,
{
    let NarrationJob {
        app,
        pool,
        story_id,
        config,
        history,
        turn_context_plan,
        tools,
        stop_after_tool_result,
        reasoning_effort,
        before_seq,
        stream_id,
    } = job;
    tauri::async_runtime::spawn(async move {
        let current_note =
            author_note::current_for_cut(&pool, &story_id, before_seq).unwrap_or_default();
        let budget_preamble = prompts::narrator_system_prompt(current_note.as_deref());
        let budget_context = turn_context_plan.full_context.unwrap_or_default();
        let prepared = compaction::prepare_history(
            &pool,
            &story_id,
            &config,
            &budget_preamble,
            &budget_context,
            history,
            before_seq,
        )
        .await;
        let mut history = prepared.turns;
        let baked_note =
            author_note::at_boundary(&pool, &story_id, prepared.through_seq).unwrap_or_default();
        let Some(mut action) = history.pop() else {
            let _ = app.emit(
                "narration-error",
                NarrationErrorPayload {
                    stream_id: &stream_id,
                    message: "the narration history has no action turn",
                },
            );
            return;
        };
        if !action.is_player {
            let _ = app.emit(
                "narration-error",
                NarrationErrorPayload {
                    stream_id: &stream_id,
                    message: "the narration history does not end with an action turn",
                },
            );
            return;
        }
        action.content = combine_context_blocks(&[
            turn_context_plan.live_context.unwrap_or_default(),
            action.content,
        ]);
        let preamble = prompts::narrator_system_prompt(baked_note.as_deref());
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
            tools,
        };

        let app_for_chunks = app.clone();
        let stream_id_for_chunks = stream_id.clone();
        let result = ai::stream_narration(req, move |chunk| match chunk {
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
        .await;

        match result {
            Ok((visible, thoughts)) => {
                let visible = visible.trim().to_string();
                let thoughts = thoughts.trim();
                let thoughts_opt = if thoughts.is_empty() {
                    None
                } else {
                    Some(thoughts.to_string())
                };
                if let Err(e) =
                    on_success(app.clone(), pool, stream_id.clone(), visible, thoughts_opt).await
                {
                    let _ = app.emit(
                        "narration-error",
                        NarrationErrorPayload {
                            stream_id: &stream_id,
                            message: &e.to_string(),
                        },
                    );
                }
            }
            Err(e) => {
                let _ = app.emit(
                    "narration-error",
                    NarrationErrorPayload {
                        stream_id: &stream_id,
                        message: &e.to_string(),
                    },
                );
            }
        }
    });
}

fn start_action_generation(
    app: AppHandle,
    pool: Pool,
    story_id: String,
    action: TimelineEntry,
    mode: String,
    history: Vec<HistoryTurn>,
    before_seq: Option<i64>,
    extra_context: String,
) -> AppResult<String> {
    let config = settings::resolve_text_model(&app, &pool)?;
    let dicerolls = diceroll_settings::read_story_diceroll_settings(&pool, &story_id)?;
    let dice_mode = DiceMode::from_str_or_default(&dicerolls.dice_mode);
    let reasoning_effort = dicerolls.reasoning_effort.clone();
    let image_settings = settings::read_image_model_settings(&app, &pool)?;
    let is_see = mode == "see";
    let image_enabled = image_settings.enabled
        && image_settings.has_api_key
        && (is_see || image_settings.narrator_images);
    let (image_tools, image_requests) = narrator_image_tools(image_enabled);
    if is_see && !image_enabled {
        return Err(AppError::Invalid(
            "image generation is disabled or has no API key".into(),
        ));
    }
    let prior_narration = {
        let conn = pool.get()?;
        let raw = timeline_repository::list_logical_entries(&conn, &story_id)?;
        reducer::active_visible_entries(&raw)
            .into_iter()
            .rev()
            .find(|entry| entry.kind == timeline_kind::NARRATION)
            .map(|entry| (entry.id, entry.content.unwrap_or_default()))
    };
    if is_see && prior_narration.is_none() {
        return Err(AppError::Invalid(
            "there is no narrated scene to illustrate".into(),
        ));
    }

    let (mut tool_set, staging) = if dicerolls.attributes_enabled {
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let embedding_api_key = settings::read_api_key(&app, "openrouter").unwrap_or_default();
        (
            tools::narrator_tools(staging.clone(), embedding_api_key, dice_mode),
            Some(staging),
        )
    } else {
        (Vec::new(), None)
    };

    let mut context = vec![extra_context];
    if dicerolls.attributes_enabled {
        context.push(format!(
            "{} <dice_mode>{}</dice_mode>",
            prompts::ENTITY_TOOLS_AVAILABLE_PREFIX,
            prompts::dice_mode_instruction(dice_mode)
        ));
    }
    if image_enabled {
        tool_set.extend(image_tools);
        context.push(prompts::IMAGE_TOOL_AVAILABLE_INSTRUCTION.to_string());
    }
    let context = combine_context_blocks(&context);
    let turn_context_plan = build_turn_context(&pool, &story_id, &history, &config, &context)?;
    let stream_id = Uuid::new_v4().to_string();
    let story_id_bg = story_id.clone();
    let action_for_done = action.clone();
    let source_action_id = action.id.clone();

    spawn_narration(
        NarrationJob {
            app,
            pool,
            story_id,
            config,
            history,
            turn_context_plan,
            tools: tool_set,
            stop_after_tool_result: is_see,
            reasoning_effort,
            before_seq,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            if is_see {
                let request = std::mem::take(&mut *image_requests.lock().await)
                    .into_iter()
                    .next();
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
                timeline_repository::active_entry(&conn, &passage.id)?
            };
            let _ = app.emit(
                "narration-done",
                NarrationDonePayload {
                    stream_id: sid,
                    entry,
                },
            );
            kick_auto_title(&app, &pool, &story_id_bg);
            let requests = std::mem::take(&mut *image_requests.lock().await);
            if !requests.is_empty() {
                images::generate_from_narrator_requests(
                    &app,
                    &pool,
                    &passage.id,
                    &visible,
                    requests,
                    None,
                );
            }
            Ok(())
        },
    );
    Ok(stream_id)
}

#[tauri::command]
pub async fn submit_turn(
    app: AppHandle,
    pool: State<'_, Pool>,
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
        match get_last_story_entry(&conn, &story_id)?.filter(|entry| entry.role == "player") {
            Some(entry) => {
                let has_completed_effect: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM timeline_entries
                     WHERE target_entry_id = ?1 AND kind = ?2)",
                    rusqlite::params![entry.id, timeline_kind::IMAGE_GENERATED],
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
            timeline_repository::active_entry(&conn, &action.id)?
        }
        None => {
            let conn = pool.get()?;
            let action = insert_story_entry(&conn, &story_id, "player", &mode, content, None)?;
            timeline_repository::active_entry(&conn, &action.id)?
        }
    };

    let history = load_history(pool.inner(), &story_id, None)?;
    let stream_id = start_action_generation(
        app,
        pool.inner().clone(),
        story_id,
        action.clone(),
        mode,
        history,
        None,
        String::new(),
    )?;
    Ok(SubmitTurnResult {
        entry: action,
        stream_id,
    })
}

/// "Retry": regenerates the latest narrator passage. The existing persisted
/// passage remains intact while the model is running and is only replaced
/// after a successful generation, so configuration or stream failures cannot
/// destroy the version the user was trying to retry.
#[derive(Clone, Copy)]
enum VariantMode {
    Retry,
    Swipe,
}

impl VariantMode {
    fn action(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Swipe => "swipe",
        }
    }

    fn past_tense(self) -> &'static str {
        match self {
            Self::Retry => "retried",
            Self::Swipe => "swiped",
        }
    }
}

fn start_variant_generation(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    entry_id: String,
    mode: VariantMode,
) -> AppResult<String> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_story_entry(&conn, &story_id)?
            .ok_or_else(|| AppError::Invalid(format!("no narration to {}", mode.action())))?;
        if last.id != entry_id {
            return Err(AppError::Invalid(format!(
                "only the latest narration can be {}",
                mode.past_tense()
            )));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid(format!(
                "only a narration entry can be {}",
                mode.past_tense()
            )));
        }
        last
    };

    let history = load_history(&pool, &story_id, Some(target.seq))?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let reasoning_effort =
        diceroll_settings::read_story_diceroll_settings(pool.inner(), &story_id)?.reasoning_effort;
    let roll_context = roll_context_block(pool.inner(), &target.id)?;
    let image_settings = settings::read_image_model_settings(&app, pool.inner())?;
    let image_enabled =
        image_settings.enabled && image_settings.narrator_images && image_settings.has_api_key;
    let (tool_set, image_requests) = narrator_image_tools(image_enabled);
    let extra_instructions = [
        (!roll_context.is_empty()).then_some(roll_context.as_str()),
        image_enabled.then_some(prompts::IMAGE_TOOL_AVAILABLE_INSTRUCTION),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let turn_context_plan = build_turn_context(
        pool.inner(),
        &story_id,
        &history,
        &config,
        &extra_instructions,
    )?;

    let stream_id = Uuid::new_v4().to_string();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            story_id,
            config,
            history,
            turn_context_plan,
            tools: tool_set,
            stop_after_tool_result: false,
            reasoning_effort,
            before_seq: Some(target.seq),
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            let entry =
                append_variant(&pool, &target, &visible, thoughts.as_deref(), mode.action())?;
            match mode {
                VariantMode::Retry => {
                    let _ = app.emit(
                        "narration-done",
                        NarrationDonePayload {
                            stream_id: sid,
                            entry,
                        },
                    );
                }
                VariantMode::Swipe => {
                    let variants = {
                        let conn = pool.get()?;
                        super::repository::list_variants(&conn, &target.id)?
                    };
                    let _ = app.emit(
                        "swipe-done",
                        SwipeDonePayload {
                            stream_id: sid,
                            entry,
                            variants,
                        },
                    );
                }
            }
            let requests = std::mem::take(&mut *image_requests.lock().await);
            if !requests.is_empty() {
                images::generate_from_narrator_requests(
                    &app, &pool, &target.id, &visible, requests, None,
                );
            }
            Ok(())
        },
    );

    Ok(stream_id)
}

#[tauri::command]
pub async fn retry_narration(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    let trailing_action = {
        let conn = pool.get()?;
        get_last_story_entry(&conn, &story_id)?
            .filter(|entry| entry.id == entry_id && entry.role == "player")
    };
    let stream_id = if let Some(action) = trailing_action {
        let history = load_history(pool.inner(), &story_id, Some(action.seq + 1))?;
        let active_action = {
            let conn = pool.get()?;
            timeline_repository::active_entry(&conn, &action.id)?
        };
        let mode = action.input_mode.clone();
        start_action_generation(
            app,
            pool.inner().clone(),
            story_id,
            active_action,
            mode,
            history,
            Some(action.seq + 1),
            String::new(),
        )?
    } else {
        start_variant_generation(app, pool, story_id, entry_id.clone(), VariantMode::Retry)?
    };
    Ok(RetryResult {
        entry_id,
        stream_id,
    })
}

/// "Swipe": generates an alternate variant of the current (latest) narrator
/// passage and pages between variants without destroying any — unlike Retry,
/// which replaces. Updates the passage in place and emits `swipe-done` with
/// the full variant list.
#[tauri::command]
pub async fn generate_narration_variant(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    entry_id: String,
) -> AppResult<String> {
    start_variant_generation(app, pool, story_id, entry_id, VariantMode::Swipe)
}

#[tauri::command]
pub fn list_narration_variants(
    pool: State<Pool>,
    entry_id: String,
) -> AppResult<Vec<NarrationVariant>> {
    let conn = pool.get()?;
    super::repository::list_variants(&conn, &entry_id)
}

#[tauri::command]
pub fn select_narration_variant(
    pool: State<Pool>,
    entry_id: String,
    variant_entry_id: String,
) -> AppResult<TimelineEntry> {
    let (image_paths, entry) = with_transaction(pool.inner(), |tx| {
        let image_paths = image_paths_for_entry(tx, &entry_id)?;
        let target = timeline_repository::get_entry(tx, &entry_id)?;
        if variant_entry_id != entry_id {
            let variant = timeline_repository::get_entry(tx, &variant_entry_id)?;
            if variant.kind != timeline_kind::NARRATION_VARIANT
                || variant.target_entry_id.as_deref() != Some(&entry_id)
            {
                return Err(AppError::NotFound(format!(
                    "variant {variant_entry_id} not found"
                )));
            }
        }
        timeline_repository::append_entry(
            tx,
            &target.story_id,
            timeline_kind::NARRATION_SELECTED,
            "hidden",
            None,
            &serde_json::json!({"selected_entry_id": variant_entry_id, "reason":"user_selection"}),
            Some(&entry_id),
        )?;
        tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [&entry_id])?;
        tx.execute(
            "DELETE FROM timeline_entries WHERE kind = ?1 AND target_entry_id = ?2",
            rusqlite::params![timeline_kind::IMAGE_GENERATED, entry_id],
        )?;
        Ok((
            image_paths,
            timeline_repository::active_entry(tx, &entry_id)?,
        ))
    })?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    Ok(entry)
}

/// "Edit": append a content override for any visible timeline entry. The
/// override is recorded against whichever variant is currently active
/// (`applies_to`), so it stays attached to that specific variant and survives
/// paging away and back — re-selecting a *different* variant correctly shows
/// that variant's own text instead.
#[tauri::command]
pub fn edit_timeline_entry(
    pool: State<Pool>,
    entry_id: String,
    content: String,
) -> AppResult<TimelineEntry> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    let (image_paths, entry) = with_transaction(pool.inner(), |tx| {
        let image_paths = image_paths_for_entry(tx, &entry_id)?;
        let target = timeline_repository::get_entry(tx, &entry_id)?;
        let raw = timeline_repository::list_logical_entries(tx, &target.story_id)?;
        let applies_to = reducer::active_variant_id(&raw, &entry_id);
        timeline_repository::append_entry(
            tx,
            &target.story_id,
            timeline_kind::CONTENT_EDITED,
            "hidden",
            Some(content),
            &serde_json::json!({"reason":"user_edit", "applies_to": applies_to}),
            Some(&entry_id),
        )?;
        tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [&entry_id])?;
        tx.execute(
            "DELETE FROM timeline_entries WHERE kind = ?1 AND target_entry_id = ?2",
            rusqlite::params![timeline_kind::IMAGE_GENERATED, entry_id],
        )?;
        Ok((
            image_paths,
            timeline_repository::active_entry(tx, &entry_id)?,
        ))
    })?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }
    Ok(entry)
}

fn cascade_entries(
    conn: &rusqlite::Connection,
    root_id: &str,
) -> AppResult<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE doomed(id) AS (
             SELECT ?1
             UNION
             SELECT timeline_entries.id FROM timeline_entries
             JOIN doomed ON timeline_entries.target_entry_id = doomed.id
         )
         SELECT timeline_entries.id, timeline_entries.kind, timeline_entries.payload_json
         FROM timeline_entries JOIN doomed ON doomed.id = timeline_entries.id",
    )?;
    let entries = stmt
        .query_map([root_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(entries)
}

fn collect_cascade_effects(
    conn: &rusqlite::Connection,
    root_id: &str,
    doomed_ids: &mut HashSet<String>,
    affected_entities: &mut HashSet<String>,
    image_paths: &mut Vec<String>,
) -> AppResult<()> {
    for (id, kind, payload_json) in cascade_entries(conn, root_id)? {
        doomed_ids.insert(id);
        if kind == timeline_kind::IMAGE_GENERATED {
            if let Some(asset_id) = serde_json::from_str::<serde_json::Value>(&payload_json)
                .ok()
                .and_then(|payload| {
                    payload
                        .get("asset_id")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
            {
                let path = conn
                    .query_row(
                        "SELECT path FROM image_assets WHERE id = ?1",
                        [&asset_id],
                        |row| row.get::<_, String>(0),
                    )
                    .ok();
                if let Some(path) = path {
                    if !image_paths.contains(&path) {
                        image_paths.push(path);
                    }
                }
                conn.execute("DELETE FROM image_assets WHERE id = ?1", [&asset_id])?;
            }
        } else if matches!(
            kind.as_str(),
            timeline_kind::ENTITY_CREATED
                | timeline_kind::ENTITY_UPDATED
                | timeline_kind::ENTITY_DELETED
                | timeline_kind::ENTITY_ATTRIBUTE_CHANGED
                | timeline_kind::ENTITY_ATTRIBUTE_REMOVED
        ) {
            if let Some(entity_id) = serde_json::from_str::<serde_json::Value>(&payload_json)
                .ok()
                .and_then(|payload| {
                    payload
                        .get("entity_id")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
            {
                affected_entities.insert(entity_id);
            }
        }
    }
    Ok(())
}

fn erase_last_exchange_in_tx(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
) -> AppResult<(Vec<String>, Vec<String>)> {
    let Some(last) = get_last_story_entry(tx, story_id)? else {
        return Ok((vec![], vec![]));
    };
    let mut removed = vec![last.id.clone()];
    let mut image_paths = image_paths_for_entry(tx, &last.id)?;
    let mut doomed_ids = HashSet::new();
    let mut affected_entities = HashSet::new();
    collect_cascade_effects(
        tx,
        &last.id,
        &mut doomed_ids,
        &mut affected_entities,
        &mut image_paths,
    )?;
    tx.execute("DELETE FROM timeline_entries WHERE id = ?1", [&last.id])?;

    let paired = last.role == "narrator" && last.input_mode == "generated";
    if paired {
        if let Some(prev) = get_last_story_entry(tx, story_id)? {
            if prev.role == "player" {
                image_paths.extend(image_paths_for_entry(tx, &prev.id)?);
                collect_cascade_effects(
                    tx,
                    &prev.id,
                    &mut doomed_ids,
                    &mut affected_entities,
                    &mut image_paths,
                )?;
                removed.push(prev.id.clone());
                tx.execute("DELETE FROM timeline_entries WHERE id = ?1", [&prev.id])?;
            }
        }
    }

    // Only drop summaries that actually covered one of the erased entries —
    // a summary covering older, still-intact history must survive so a long
    // story doesn't have to redo all its prior compaction after one Erase.
    {
        let mut stmt = tx.prepare(
            "SELECT id, payload_json FROM timeline_entries WHERE story_id = ?1 AND kind = ?2",
        )?;
        let summaries: Vec<(String, String)> = stmt
            .query_map(
                rusqlite::params![story_id, timeline_kind::CONTEXT_SUMMARY],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<_, _>>()?;
        for (summary_id, payload_json) in summaries {
            let through_entry_id = serde_json::from_str::<serde_json::Value>(&payload_json)
                .ok()
                .and_then(|v| {
                    v.get("through_entry_id")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                });
            if through_entry_id.is_some_and(|through| doomed_ids.contains(&through)) {
                tx.execute("DELETE FROM timeline_entries WHERE id = ?1", [&summary_id])?;
            }
        }
    }

    let now = Utc::now().to_rfc3339();
    crate::features::timeline::projections::replay_entities(tx, story_id, &affected_entities)?;
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, story_id],
    )?;

    Ok((removed, image_paths))
}

/// "Erase": removes the most recent exchange — the latest narration plus the
/// player message (or story draft) that triggered it. Returns the IDs removed
/// so the frontend can splice locally.
#[tauri::command]
pub fn erase_last_exchange(pool: State<Pool>, story_id: String) -> AppResult<Vec<String>> {
    let (removed, image_paths) =
        with_transaction(pool.inner(), |tx| erase_last_exchange_in_tx(tx, &story_id))?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::timeline::repository::append_entry;
    use serde_json::json;

    #[test]
    fn revision_image_tools_include_only_illustration_when_enabled() {
        let (enabled, _) = narrator_image_tools(true);
        let (disabled, _) = narrator_image_tools(false);

        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name(), "illustrate_scene");
        assert!(disabled.is_empty());
    }

    #[test]
    fn append_variant_records_mode_and_selects_the_newest_revision() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let target = insert_story_entry(
            &conn,
            "s",
            "narrator",
            "generated",
            "original",
            Some("original thoughts"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/variant.png', 'prompt', 'now')",
            [&target.id],
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            timeline_kind::IMAGE_GENERATED,
            "hidden",
            Some("image"),
            &json!({}),
            Some(&target.id),
        )
        .unwrap();
        drop(conn);

        append_variant(
            &pool,
            &target,
            "retry text",
            Some("retry thoughts"),
            "retry",
        )
        .unwrap();
        let active = append_variant(
            &pool,
            &target,
            "swipe text",
            Some("swipe thoughts"),
            "swipe",
        )
        .unwrap();
        assert_eq!(active.id, target.id);
        assert_eq!(active.content.as_deref(), Some("swipe text"));

        let conn = pool.get().unwrap();
        let raw = timeline_repository::list_logical_entries(&conn, "s").unwrap();
        let variants = raw
            .iter()
            .filter(|entry| entry.kind == timeline_kind::NARRATION_VARIANT)
            .collect::<Vec<_>>();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].payload["reason"], json!("retry"));
        assert_eq!(variants[1].payload["reason"], json!("swipe"));
        assert_eq!(variants[1].payload["thoughts"], json!("swipe thoughts"));
        assert!(!raw
            .iter()
            .any(|entry| entry.kind == timeline_kind::NARRATION_SELECTED));
        assert_eq!(
            reducer::variants_for_entry(&raw, &target.id)
                .into_iter()
                .find(|variant| variant.is_selected)
                .unwrap()
                .id,
            variants[1].id
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(VariantMode::Retry.past_tense(), "retried");
        assert_eq!(VariantMode::Swipe.past_tense(), "swiped");
    }

    #[test]
    fn roll_context_includes_every_roll_in_chronological_order() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "s",
            timeline_kind::NARRATION,
            "visible",
            Some("result"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        for content in ["First roll succeeded.", "Second roll failed."] {
            append_entry(
                &conn,
                "s",
                timeline_kind::DICEROLL,
                "hidden",
                Some(content),
                &json!({}),
                Some(&narration.id),
            )
            .unwrap();
        }
        drop(conn);

        let context = roll_context_block(&pool, &narration.id).unwrap();
        assert!(context.contains("First roll succeeded.\n- Second roll failed."));
    }

    #[test]
    fn erase_removes_the_action_and_generated_response_for_every_mode() {
        for mode in ["do", "say", "story", "guide", "continue", "see"] {
            let pool = crate::shared::db::test_pool();
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
                 VALUES ('s', 'story', 'now', 'now', '{}')",
                [],
            )
            .unwrap();
            let action = append_entry(
                &conn,
                "s",
                timeline_kind::PLAYER_MESSAGE,
                "visible",
                Some(if matches!(mode, "continue" | "see") {
                    ""
                } else {
                    "action"
                }),
                &json!({"input_mode":mode}),
                None,
            )
            .unwrap();
            let response = append_entry(
                &conn,
                "s",
                timeline_kind::NARRATION,
                "visible",
                Some("response"),
                &json!({"input_mode":"generated"}),
                None,
            )
            .unwrap();
            drop(conn);

            let (removed, _) =
                with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
            assert_eq!(removed, vec![response.id, action.id], "mode {mode}");
        }
    }

    #[test]
    fn erase_trailing_see_removes_its_image_from_the_prior_narration() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "s",
            timeline_kind::NARRATION,
            "visible",
            Some("A moonlit harbor."),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let see = append_entry(
            &conn,
            "s",
            timeline_kind::PLAYER_MESSAGE,
            "visible",
            Some(""),
            &json!({"input_mode":"see"}),
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/see.png', 'prompt', 'now')",
            [&narration.id],
        )
        .unwrap();
        let image_event = append_entry(
            &conn,
            "s",
            timeline_kind::IMAGE_GENERATED,
            "hidden",
            Some("image"),
            &json!({"asset_id":"image"}),
            Some(&see.id),
        )
        .unwrap();
        drop(conn);

        let (removed, paths) =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![see.id]);
        assert_eq!(paths, vec!["C:/tmp/see.png"]);
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM timeline_entries WHERE id = ?1",
                [&image_event.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM timeline_entries WHERE id = ?1",
                [&narration.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn hard_erase_removes_exchange_derivatives_summaries_and_projections() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let baseline = append_entry(
            &conn,
            "s",
            timeline_kind::NARRATION,
            "visible",
            Some("Earlier scene"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "mira",
            "s",
            "character",
            "Mira",
            Some("silver hair"),
            "test",
            None,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "unrelated",
            "s",
            "character",
            "Tomas",
            None,
            "test",
            None,
        )
        .unwrap();
        let trust_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Trust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let trust =
            crate::features::entities::attributes::find_attribute_by_id(&conn, &trust_id).unwrap();
        crate::features::entities::attributes::apply_attribute_delta(
            &conn,
            "s",
            "mira",
            &trust,
            2.0,
            "earlier event",
            &baseline.id,
            false,
        )
        .unwrap();
        let mira_last_event_id: String = conn
            .query_row(
                "SELECT last_event_id FROM story_entity_state WHERE story_id = 's' AND entity_id = 'mira'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let attribute_last_event_id: String = conn
            .query_row(
                "SELECT last_event_id FROM entity_attributes WHERE story_id = 's' AND entity_id = 'mira' AND attribute_id = ?1",
                [&trust_id],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_projection: (String, Option<String>, i64, String, String) = conn
            .query_row(
                "SELECT name, appearance_anchor, is_present, updated_at, last_event_id
                 FROM story_entity_state WHERE story_id = 's' AND entity_id = 'unrelated'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        let older_summary = append_entry(
            &conn,
            "s",
            timeline_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("older summary"),
            &json!({"through_entry_id":baseline.id}),
            None,
        )
        .unwrap();

        let player = append_entry(
            &conn,
            "s",
            timeline_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "s",
            timeline_kind::NARRATION,
            "visible",
            Some("result"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        crate::features::entities::update_entity_sync(
            &conn,
            "s",
            "mira",
            "Mira Changed",
            Some("black armor"),
            "narrator_tool",
            Some(&narration.id),
        )
        .unwrap();
        crate::features::entities::attributes::apply_attribute_delta(
            &conn,
            "s",
            "mira",
            &trust,
            3.0,
            "latest event",
            &narration.id,
            false,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "temporary",
            "s",
            "character",
            "Temporary",
            None,
            "narrator_tool",
            Some(&narration.id),
        )
        .unwrap();
        let query = append_entry(
            &conn,
            "s",
            timeline_kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up Mira"),
            &json!({"entity_ids":["mira"]}),
            Some(&narration.id),
        )
        .unwrap();
        for kind in [
            timeline_kind::DICEROLL,
            timeline_kind::NARRATION_VARIANT,
            timeline_kind::NARRATION_SELECTED,
            timeline_kind::CONTENT_EDITED,
            timeline_kind::IMAGE_GENERATED,
        ] {
            append_entry(
                &conn,
                "s",
                kind,
                "hidden",
                Some("derivative"),
                &json!({}),
                Some(&narration.id),
            )
            .unwrap();
        }
        let doomed_summary = append_entry(
            &conn,
            "s",
            timeline_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("summary"),
            &json!({"through_entry_id":query.id}),
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/image.png', 'prompt', 'now')",
            [&narration.id],
        )
        .unwrap();
        drop(conn);

        let (removed, paths) =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![narration.id, player.id]);
        assert_eq!(paths, vec!["C:/tmp/image.png"]);
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM timeline_entries WHERE id = ?1",
                [&doomed_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM timeline_entries WHERE id = ?1",
                [&older_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let mira: (String, Option<String>, String) = conn
            .query_row(
                "SELECT name, appearance_anchor, last_event_id FROM story_entity_state WHERE story_id = 's' AND entity_id = 'mira'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            mira,
            (
                "Mira".into(),
                Some("silver hair".into()),
                mira_last_event_id
            )
        );
        let restored_attribute: (f64, String, String) = conn
            .query_row(
                "SELECT value, source, last_event_id FROM entity_attributes WHERE story_id = 's' AND entity_id = 'mira' AND attribute_id = ?1",
                [&trust_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            restored_attribute,
            (2.0, "inferred".into(), attribute_last_event_id)
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM story_entity_state WHERE story_id = 's' AND entity_id = 'temporary'",
                [],
                |row| row.get::<_, i64>(0)
            )
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT name, appearance_anchor, is_present, updated_at, last_event_id
                 FROM story_entity_state WHERE story_id = 's' AND entity_id = 'unrelated'",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?
                ))
            )
            .unwrap(),
            unrelated_projection
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE id = 'temporary'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
