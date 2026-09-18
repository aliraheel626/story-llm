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

/// "Story" input mode: narration typed directly by the player. The authored
/// text is inserted verbatim as a narrator passage, then the model continues
/// from it in a streamed passage, same as any other generation path.
#[tauri::command]
pub fn submit_story(
    app: AppHandle,
    pool: State<Pool>,
    story_id: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }

    // Resolve everything that can fail before inserting: with no model
    // configured this must error without leaving a draft row the UI never saw.
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let reasoning_effort =
        diceroll_settings::get_story_diceroll_settings(pool.clone(), story_id.clone())?
            .reasoning_effort;

    // Build the prompt history before writing the draft so any context or
    // settings failure leaves the timeline untouched. The draft is guaranteed
    // to remain in the raw-tail floor, so it does not need a durable entry id
    // for compaction-boundary persistence here.
    let mut history = load_history(pool.inner(), &story_id, None)?;
    history.push(HistoryTurn {
        entry_id: None,
        is_player: false,
        content: content.to_string(),
    });
    let turn_context_plan = build_turn_context(
        pool.inner(),
        &story_id,
        &history,
        &config,
        prompts::FINISH_STORY_DRAFT_PROMPT,
        "",
    )?;

    let authored = {
        let conn = pool.get()?;
        insert_story_entry(&conn, &story_id, "narrator", "story", content, None)?
    };
    kick_auto_title(&app, pool.inner(), &story_id);

    let stream_id = Uuid::new_v4().to_string();
    let story_id_bg = story_id.clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            story_id: story_id.clone(),
            config,
            history,
            prompt: prompts::FINISH_STORY_DRAFT_PROMPT.to_string(),
            turn_context_plan,
            tools: Vec::new(),
            reasoning_effort,
            before_seq: None,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| {
            finish_append(
                app,
                pool,
                sid,
                story_id_bg,
                "generated_story".to_string(),
                visible,
                thoughts,
            )
        },
    );

    let entry = {
        let conn = pool.get()?;
        timeline_repository::active_entry(&conn, &authored.id)?
    };
    Ok(SubmitTurnResult { entry, stream_id })
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

fn format_prompt(input_mode: &str, content: &str) -> String {
    match input_mode {
        "say" => format!("[The player says] \"{content}\""),
        _ => format!("[The player does] {content}"),
    }
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

/// Author's Note (spec §6.6), formatted as a context block, if the story has
/// one set. Empty string when there isn't one, so callers can always append
/// it unconditionally via `combine_context_blocks`.
fn author_note_block(pool: &Pool, story_id: &str) -> AppResult<String> {
    match stories::read_author_note(pool, story_id)? {
        Some(note) => Ok(format!("{}{note}", prompts::AUTHOR_NOTE_PREFIX)),
        None => Ok(String::new()),
    }
}

fn entity_context_block(
    pool: &Pool,
    story_id: &str,
    detailed_entity_ids: Option<&HashSet<String>>,
) -> AppResult<String> {
    let author_note = author_note_block(pool, story_id)?;
    let conn = pool.get()?;
    let entities = crate::features::entities::list_entities_sync(&conn, story_id, None)?;
    let entity_ids = entities
        .iter()
        .filter(|entity| {
            detailed_entity_ids.is_none_or(|entity_ids| entity_ids.contains(&entity.id))
        })
        .map(|entity| entity.id.as_str())
        .collect::<Vec<_>>();
    let attrs_by_entity =
        crate::features::entities::attributes::list_entity_attributes_for_entities_sync(
            &conn,
            story_id,
            &entity_ids,
        )?;

    let mut lines = vec![prompts::ENTITY_CONTEXT_HEADER.to_string()];
    for entity in entities {
        if detailed_entity_ids.is_some_and(|entity_ids| !entity_ids.contains(&entity.id)) {
            lines.push(format!("- {} ({})", entity.name, entity.kind));
            continue;
        }
        let appearance = entity
            .appearance_anchor
            .as_deref()
            .map(|a| format!("; appearance: {a}"))
            .unwrap_or_default();
        let attributes = match attrs_by_entity.get(&entity.id) {
            Some(attrs) if !attrs.is_empty() => format!(
                "; attributes: {}",
                attrs
                    .iter()
                    .map(|attribute| {
                        format!("{}={}", attribute.canonical_name, attribute.value)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        };
        lines.push(format!(
            "- {} ({}){appearance}{attributes}",
            entity.name, entity.kind
        ));
    }
    Ok(combine_context_blocks(&[author_note, lines.join("\n")]))
}

fn touched_entity_ids(pool: &Pool, raw_tail: &[HistoryTurn]) -> AppResult<HashSet<String>> {
    let conn = pool.get()?;
    let mut touched = HashSet::new();
    let entry_ids = raw_tail
        .iter()
        .filter_map(|turn| turn.entry_id.as_deref())
        .collect::<Vec<_>>();
    if entry_ids.is_empty() {
        return Ok(touched);
    }
    let entry_ids_json = serde_json::to_string(&entry_ids).map_err(|error| {
        AppError::Other(format!("failed to serialize timeline entry ids: {error}"))
    })?;
    let mut stmt = conn.prepare(
        "SELECT kind, payload_json FROM timeline_entries
         WHERE id IN (SELECT value FROM json_each(?1))
           AND kind IN (?2, ?3, ?4, ?5)",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            entry_ids_json,
            timeline_kind::ENTITY_CREATED,
            timeline_kind::ENTITY_UPDATED,
            timeline_kind::ENTITY_ATTRIBUTE_CHANGED,
            timeline_kind::ENTITY_QUERIED,
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    for row in rows {
        let (entry_kind, payload_json) = row?;
        let payload = serde_json::from_str::<serde_json::Value>(&payload_json)
            .map_err(|error| AppError::Other(format!("invalid timeline payload JSON: {error}")))?;
        match entry_kind.as_str() {
            timeline_kind::ENTITY_CREATED
            | timeline_kind::ENTITY_UPDATED
            | timeline_kind::ENTITY_ATTRIBUTE_CHANGED => {
                if let Some(entity_id) = payload.get("entity_id").and_then(|id| id.as_str()) {
                    touched.insert(entity_id.to_string());
                }
            }
            timeline_kind::ENTITY_QUERIED => {
                if let Some(entity_ids) = payload.get("entity_ids").and_then(|ids| ids.as_array()) {
                    touched.extend(
                        entity_ids
                            .iter()
                            .filter_map(|id| id.as_str().map(str::to_string)),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(touched)
}

#[derive(Debug, Clone)]
struct TurnContextPlan {
    live_context: String,
    full_context: String,
}

fn build_turn_context(
    pool: &Pool,
    story_id: &str,
    history: &[HistoryTurn],
    config: &TextModelConfig,
    prompt: &str,
    extra_instructions: &str,
) -> AppResult<TurnContextPlan> {
    let memory = settings::read_narrator_memory_settings(pool)?;
    if memory.entity_context_mode == "none" {
        // No entity dump at all — cheaper than "all", skips the entity/attribute
        // queries entirely. The author's note is a distinct, deliberate
        // instruction (not part of the entity context this mode turns off), so
        // it's still included here.
        let author_note = author_note_block(pool, story_id)?;
        let context = combine_context_blocks(&[author_note, extra_instructions.to_string()]);
        return Ok(TurnContextPlan {
            live_context: context.clone(),
            full_context: context,
        });
    }

    let full_context = entity_context_block(pool, story_id, None)?;
    let full_context = combine_context_blocks(&[full_context, extra_instructions.to_string()]);
    if memory.entity_context_mode == "all" {
        return Ok(TurnContextPlan {
            live_context: full_context.clone(),
            full_context,
        });
    }

    // Size compaction against the full snapshot. That is today's budget and
    // avoids a circular dependency where the scoped context changes the
    // boundary used to decide which entities belong in that same context.
    let budget_prompt = combine_context_blocks(&[full_context.clone(), prompt.to_string()]);
    let split = crate::features::timeline::compaction::raw_tail_boundary(
        history,
        config,
        prompts::NARRATOR_SYSTEM_PROMPT,
        &budget_prompt,
    );
    let touched = touched_entity_ids(pool, &history[split..])?;
    let scoped_context = entity_context_block(pool, story_id, Some(&touched))?;
    Ok(TurnContextPlan {
        live_context: combine_context_blocks(&[scoped_context, extra_instructions.to_string()]),
        full_context,
    })
}

fn combine_context_blocks(parts: &[String]) -> String {
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
        format!("{}{}", prompts::ROLL_CONTEXT_HEADER, contents.join("\n- "))
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
/// outcome constraint) into the per-turn prompt.
struct NarrationJob {
    app: AppHandle,
    pool: Pool,
    story_id: String,
    config: TextModelConfig,
    history: Vec<HistoryTurn>,
    prompt: String,
    turn_context_plan: TurnContextPlan,
    /// Live tools available to this narration path. Revision jobs receive only
    /// the image tool, never state-changing entity or dice tools.
    tools: Vec<DynamicTool>,
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
        prompt,
        turn_context_plan,
        tools,
        reasoning_effort,
        before_seq,
        stream_id,
    } = job;
    tauri::async_runtime::spawn(async move {
        let budget_prompt =
            combine_context_blocks(&[turn_context_plan.full_context, prompt.clone()]);
        let history = crate::features::timeline::compaction::prepare_history(
            &pool,
            &story_id,
            &config,
            prompts::NARRATOR_SYSTEM_PROMPT,
            &budget_prompt,
            history,
            before_seq,
        )
        .await;
        // The system prompt is the static rulebook only — never reformatted,
        // byte-identical on every call. Per-turn facts (entity state, roll
        // outcome, author's note) go on the message itself instead, since
        // that's what actually changes turn to turn, not the rules.
        let prompt = combine_context_blocks(&[turn_context_plan.live_context, prompt]);
        let req = NarrateRequest {
            config,
            preamble: prompts::NARRATOR_SYSTEM_PROMPT.to_string(),
            history,
            prompt,
            stop_after_tool_result: false,
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
                if visible.is_empty() {
                    let _ = app.emit(
                        "narration-error",
                        NarrationErrorPayload {
                            stream_id: &stream_id,
                            message: "the model returned no visible text",
                        },
                    );
                    return;
                }
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

/// Shared terminal step for plain append flows without narrator tools: insert
/// the passage, persist a roll if one was already resolved for it, emit
/// `narration-done`.
async fn finish_append(
    app: AppHandle,
    pool: Pool,
    stream_id: String,
    story_id: String,
    input_mode: String,
    visible: String,
    thoughts: Option<String>,
) -> AppResult<()> {
    let passage = append_narration_entry(
        &pool,
        &story_id,
        &input_mode,
        &visible,
        thoughts.as_deref(),
        None,
    )
    .await?;
    let entry_id = passage.id.clone();
    let entry = {
        let conn = pool.get()?;
        timeline_repository::active_entry(&conn, &entry_id)?
    };
    let _ = app.emit("narration-done", NarrationDonePayload { stream_id, entry });
    kick_auto_title(&app, &pool, &story_id);
    Ok(())
}

/// "Do"/"Say" input modes: persists the player's passage immediately, then
/// streams a narrator continuation in the background (see `spawn_narration`).
/// When Attributes are enabled for the story, the narrator gets a tool set
/// (see `tools`) to check, create, and update entities and their attributes,
/// and to roll dice, mid-generation — their writes are staged and committed
/// atomically alongside the passage in `append_narration_entry`.
#[tauri::command]
pub async fn submit_turn(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    input_mode: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    if input_mode != "do" && input_mode != "say" {
        return Err(AppError::Invalid(format!(
            "invalid input_mode: {input_mode}"
        )));
    }

    let history = load_history(&pool, &story_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let dicerolls = diceroll_settings::get_story_diceroll_settings(pool.clone(), story_id.clone())?;
    let dice_mode = DiceMode::from_str_or_default(&dicerolls.dice_mode);
    let reasoning_effort = dicerolls.reasoning_effort.clone();
    let image_settings = settings::read_image_model_settings(&app, pool.inner())?;
    let image_enabled =
        image_settings.enabled && image_settings.narrator_images && image_settings.has_api_key;
    let (image_tools, image_requests) = narrator_image_tools(image_enabled);

    let player_passage = {
        let conn = pool.get()?;
        insert_story_entry(&conn, &story_id, "player", &input_mode, content, None)?
    };

    let prompt = format_prompt(&input_mode, content);

    let (mut tool_set, staging) = if dicerolls.attributes_enabled {
        let staging = Arc::new(Mutex::new(TurnStaging::new(
            pool.inner().clone(),
            story_id.clone(),
        )));
        // Attribute-similarity embeddings always go to OpenRouter, regardless
        // of the active narration provider (e.g. Nous Portal) — most turns
        // never reach this call at all (exact name matches skip it), so a
        // missing OpenRouter key degrades gracefully to a per-call error
        // rather than blocking the whole turn.
        let embedding_api_key = settings::read_api_key(&app, "openrouter").unwrap_or_default();
        let tool_set = tools::narrator_tools(staging.clone(), embedding_api_key, dice_mode);
        (tool_set, Some(staging))
    } else {
        (Vec::new(), None)
    };

    let mut tool_instructions = Vec::new();
    if dicerolls.attributes_enabled {
        tool_instructions.push(format!(
            "{} {}",
            prompts::ENTITY_TOOLS_AVAILABLE_PREFIX,
            prompts::dice_mode_instruction(dice_mode)
        ));
    }
    if image_enabled {
        tool_set.extend(image_tools);
        tool_instructions.push(prompts::IMAGE_TOOL_AVAILABLE_INSTRUCTION.to_string());
    }
    let tool_instructions = tool_instructions.join(" ");
    let turn_context_plan = build_turn_context(
        pool.inner(),
        &story_id,
        &history,
        &config,
        &prompt,
        &tool_instructions,
    )?;

    let stream_id = Uuid::new_v4().to_string();
    let story_id_bg = story_id.clone();

    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            story_id: story_id.clone(),
            config,
            history,
            prompt,
            turn_context_plan,
            tools: tool_set,
            reasoning_effort,
            before_seq: None,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
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
                );
            }
            Ok(())
        },
    );

    let entry = {
        let conn = pool.get()?;
        timeline_repository::active_entry(&conn, &player_passage.id)?
    };
    Ok(SubmitTurnResult { entry, stream_id })
}

/// "Guide" mode: an ephemeral, out-of-character steering note. No player
/// passage is created — the note only shapes this one generation, then it's
/// gone (the spec's Director's Note behavior, exposed directly in the
/// composer instead of a settings field).
#[tauri::command]
pub async fn submit_guide(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    note: String,
) -> AppResult<String> {
    let note = note.trim();
    if note.is_empty() {
        return Err(AppError::Invalid("note must not be empty".into()));
    }

    let history = load_history(&pool, &story_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let reasoning_effort =
        diceroll_settings::get_story_diceroll_settings(pool.clone(), story_id.clone())?
            .reasoning_effort;
    let prompt = format!(
        "[Director's note — out of character, steer the story but do not narrate it directly: {note}] \
         Continue the scene, letting that note shape what happens next."
    );
    let turn_context_plan =
        build_turn_context(pool.inner(), &story_id, &history, &config, &prompt, "")?;

    let stream_id = Uuid::new_v4().to_string();
    let story_id_bg = story_id.clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            story_id: story_id.clone(),
            config,
            history,
            prompt,
            turn_context_plan,
            tools: Vec::new(),
            reasoning_effort,
            before_seq: None,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| {
            finish_append(
                app,
                pool,
                sid,
                story_id_bg,
                "generated_guide".to_string(),
                visible,
                thoughts,
            )
        },
    );

    Ok(stream_id)
}

/// "Continue": narrator advances the scene with no new player input.
#[tauri::command]
pub async fn continue_scene(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
) -> AppResult<String> {
    let history = load_history(&pool, &story_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let reasoning_effort =
        diceroll_settings::get_story_diceroll_settings(pool.clone(), story_id.clone())?
            .reasoning_effort;
    // A trailing story draft was never rendered (its generation failed), so
    // Continue completes it into prose instead of writing past the note.
    let (prompt, input_mode) = {
        let conn = pool.get()?;
        match get_last_story_entry(&conn, &story_id)? {
            Some(p) if p.input_mode == "story" => (
                prompts::FINISH_STORY_DRAFT_PROMPT,
                "generated_story".to_string(),
            ),
            _ => (
                prompts::CONTINUE_SCENE_PROMPT,
                "generated_continue".to_string(),
            ),
        }
    };
    let turn_context_plan =
        build_turn_context(pool.inner(), &story_id, &history, &config, prompt, "")?;

    let stream_id = Uuid::new_v4().to_string();
    let story_id_bg = story_id.clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            story_id: story_id.clone(),
            config,
            history,
            prompt: prompt.to_string(),
            turn_context_plan,
            tools: Vec::new(),
            reasoning_effort,
            before_seq: None,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| {
            finish_append(app, pool, sid, story_id_bg, input_mode, visible, thoughts)
        },
    );

    Ok(stream_id)
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
        diceroll_settings::get_story_diceroll_settings(pool.clone(), story_id.clone())?
            .reasoning_effort;
    let prompt = if target.input_mode == "generated_story" {
        prompts::FINISH_STORY_DRAFT_PROMPT
    } else {
        prompts::CONTINUE_SCENE_PROMPT
    };
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
        prompt,
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
            prompt: prompt.to_string(),
            turn_context_plan,
            tools: tool_set,
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
                    &app, &pool, &target.id, &visible, requests,
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
    let stream_id =
        start_variant_generation(app, pool, story_id, entry_id.clone(), VariantMode::Retry)?;
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
) -> AppResult<()> {
    for (id, kind, payload_json) in cascade_entries(conn, root_id)? {
        doomed_ids.insert(id);
        if matches!(
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
    collect_cascade_effects(tx, &last.id, &mut doomed_ids, &mut affected_entities)?;
    tx.execute("DELETE FROM timeline_entries WHERE id = ?1", [&last.id])?;

    let paired = last.role == "narrator"
        && matches!(last.input_mode.as_str(), "generated" | "generated_story");
    if paired {
        if let Some(prev) = get_last_story_entry(tx, story_id)? {
            if prev.role == "player" || prev.input_mode == "story" {
                image_paths.extend(image_paths_for_entry(tx, &prev.id)?);
                collect_cascade_effects(tx, &prev.id, &mut doomed_ids, &mut affected_entities)?;
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
    fn scoped_context_details_touched_entities_and_lists_the_rest() {
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
            "bob",
            "s",
            "character",
            "Bob",
            Some("a red cloak"),
            "test",
            None,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "mill",
            "s",
            "location",
            "Old Mill",
            Some("a mossy waterwheel"),
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
        conn.execute(
            "INSERT INTO entity_attributes
             (story_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
             VALUES ('s', 'bob', ?1, 7, 'user', 'now', NULL),
                    ('s', 'mill', ?1, 3, 'user', 'now', NULL)",
            [&trust_id],
        )
        .unwrap();
        let query = append_entry(
            &conn,
            "s",
            timeline_kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up: Bob"),
            &json!({"entity_ids":["bob"]}),
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('narrator_memory', ?1)",
            [json!({"tool_call_persistence":true,"entity_context_mode":"scoped"}).to_string()],
        )
        .unwrap();
        drop(conn);

        let history = vec![HistoryTurn {
            entry_id: Some(query.id),
            is_player: false,
            content: "[Authoritative story event: entity_queried]\nLooked up: Bob".into(),
        }];
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 32_768,
        };
        let plan = build_turn_context(&pool, "s", &history, &config, "prompt", "").unwrap();

        assert!(plan
            .live_context
            .contains("- Bob (character); appearance: a red cloak; attributes: Trust=7"));
        assert!(plan.live_context.contains("- Old Mill (location)"));
        assert!(!plan
            .live_context
            .contains("Old Mill (location); appearance:"));
        assert!(plan
            .full_context
            .contains("Old Mill (location); appearance: a mossy waterwheel; attributes: Trust=3"));
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
            &json!({"input_mode":"generated_continue"}),
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
