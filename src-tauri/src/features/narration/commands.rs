use std::sync::Arc;

use chrono::Utc;
use rig_agent::tool::DynamicTool;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::ai::{
    self, HistoryTurn, NarrateRequest, NarratorChunk, TextModelConfig, ToolActivityPhase,
};
use crate::features::{
    images,
    mechanics::{commands as mechanics_settings, pipeline::DiceMode},
    settings, stories,
    timeline::{
        model::{kind as timeline_kind, NarrationVariant, TimelineEntry},
        reducer, repository as timeline_repository,
    },
};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::history::load_history;
use super::model::ActiveStoryEntry;
use super::repository::{
    get_active_story_entry, get_last_story_entry, get_story_id_for_branch, image_paths_for_entry,
    insert_story_entry,
};
use super::tools::{self, TurnStaging};

const NARRATOR_PREAMBLE: &str = "You are the narrator of an interactive story. Continue the scene \
in vivid, literary prose that follows naturally from what has already happened and from the \
player's latest action, matching the established tone, tense, and style. Always narrate the \
player's actions and perceptions in the second person (\"you\"); other characters stay in the \
third person. Never speak as the player, never break the fourth wall, and never add \
meta-commentary, author's notes, or content outside the story itself.";

const CONTINUE_PROMPT: &str =
    "Continue the scene naturally from where it left off, in the established voice and pacing.";

const STORY_CONTINUE_PROMPT: &str =
    "The latest turn is the player's draft of the next passage — it may read like a terse note or a \
     directive. Complete it into the passage itself: open with the draft rendered as prose, then keep \
     writing seamlessly to a natural ending. Do not reply to it as an instruction, and do not \
     summarize it away.";

/// "Story" input mode: narration typed directly by the player. The authored
/// text is inserted verbatim as a narrator passage, then the model continues
/// from it in a streamed passage, same as any other generation path.
#[tauri::command]
pub fn submit_story(
    app: AppHandle,
    pool: State<Pool>,
    branch_id: String,
    content: String,
) -> AppResult<SubmitTurnResult> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }

    // Resolve everything that can fail before inserting: with no model
    // configured this must error without leaving a draft row the UI never saw.
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let story_id = {
        let conn = pool.get()?;
        get_story_id_for_branch(&conn, &branch_id)?
    };
    let extra_preamble = story_context_preamble(pool.inner(), &story_id, &branch_id)?;

    let authored = {
        let conn = pool.get()?;
        insert_story_entry(&conn, &branch_id, "narrator", "story", content, None)?
    };
    kick_auto_title(&app, pool.inner(), &branch_id);

    // After the insert, so the draft is the newest turn the model completes.
    let history = load_history(pool.inner(), &branch_id, None)?;

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            branch_id: branch_id.clone(),
            config,
            history,
            prompt: STORY_CONTINUE_PROMPT.to_string(),
            extra_preamble,
            tools: Vec::new(),
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| {
            finish_append(
                app,
                pool,
                sid,
                branch_id_bg,
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

/// Author's Note (spec §6.6), formatted for the preamble, if the story has
/// one set. Empty string when there isn't one, so callers can always append
/// it unconditionally via `combine_preambles`.
fn author_note_preamble(pool: &Pool, story_id: &str) -> AppResult<String> {
    match stories::read_author_note(pool, story_id)? {
        Some(note) => Ok(format!(
            "Author's note — keep this in mind throughout: {note}"
        )),
        None => Ok(String::new()),
    }
}

fn story_context_preamble(pool: &Pool, story_id: &str, branch_id: &str) -> AppResult<String> {
    let author_note = author_note_preamble(pool, story_id)?;
    let conn = pool.get()?;
    let entities = crate::features::entities::list_entities_sync(&conn, story_id, branch_id, None)?;

    let mut attrs_by_entity: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT entity_attributes.entity_id, attribute_registry.canonical_name, entity_attributes.value
             FROM entity_attributes JOIN attribute_registry ON attribute_registry.id = entity_attributes.attribute_id
             WHERE entity_attributes.branch_id = ?1 ORDER BY attribute_registry.canonical_name",
        )?;
        let rows = stmt.query_map([branch_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;
        for row in rows {
            let (entity_id, name, value) = row?;
            attrs_by_entity
                .entry(entity_id)
                .or_default()
                .push(format!("{name}={value}"));
        }
    }

    let mut lines = vec!["Current entity state is authoritative. User overrides take precedence over inferred updates. Mechanical outcomes must not be contradicted.".to_string()];
    for entity in entities {
        let appearance = entity
            .appearance_anchor
            .as_deref()
            .map(|a| format!("; appearance: {a}"))
            .unwrap_or_default();
        let attributes = match attrs_by_entity.get(&entity.id) {
            Some(attrs) if !attrs.is_empty() => format!("; attributes: {}", attrs.join(", ")),
            _ => String::new(),
        };
        lines.push(format!(
            "- {} ({}){appearance}{attributes}",
            entity.name, entity.kind
        ));
    }
    Ok(combine_preambles(&[author_note, lines.join("\n")]))
}

fn combine_preambles(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The mechanical roll that produced `entry_id`'s narration, if any — folded
/// into retry/swipe's preamble so regenerating a roll-driven turn stays
/// consistent with the outcome that already happened. The roll's timeline
/// seq is assigned after the narration it explains, so it falls outside the
/// history window a retry/swipe deliberately cuts off at the target's seq;
/// this is the only channel that outcome reaches the regenerated call by.
fn roll_context_preamble(pool: &Pool, entry_id: &str) -> AppResult<String> {
    let conn = pool.get()?;
    let content: Option<String> = conn
        .query_row(
            "SELECT content FROM timeline_entries WHERE target_entry_id = ?1 AND kind = ?2 ORDER BY seq DESC LIMIT 1",
            rusqlite::params![entry_id, timeline_kind::MECHANICAL_RESULT],
            |row| row.get(0),
        )
        .optional()?;
    Ok(match content {
        Some(content) => {
            format!("The roll that determined this outcome must not be contradicted: {content}")
        }
        None => String::new(),
    })
}

/// Inserts a fresh narration entry. If the narrator's tool calls staged any
/// entity/attribute/roll writes this turn, they're committed in the same
/// transaction — atomically with the passage, and not at all if anything
/// above this point failed first.
async fn append_narration_entry(
    pool: &Pool,
    branch_id: &str,
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
    let passage = insert_story_entry(&tx, branch_id, "narrator", input_mode, visible, thoughts)?;
    if let Some(guard) = staging_guard {
        guard.commit(&tx, &passage.id)?;
    }
    tx.commit()?;
    Ok(passage)
}

/// Appends and selects a retry variant only after generation has succeeded.
/// The base narration remains immutable and keeps its chronological position.
fn append_retry_variant(
    pool: &Pool,
    target: &ActiveStoryEntry,
    visible: &str,
    _thoughts: Option<&str>,
) -> AppResult<ActiveStoryEntry> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let image_paths = image_paths_for_entry(&tx, &target.id)?;
    tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [&target.id])?;
    tx.execute(
        "DELETE FROM timeline_entries WHERE kind = ?1 AND target_entry_id = ?2",
        rusqlite::params![timeline_kind::IMAGE_GENERATED, target.id],
    )?;
    let variant = timeline_repository::append_entry(
        &tx,
        &target.branch_id,
        timeline_kind::NARRATION_VARIANT,
        "hidden",
        Some(visible),
        &serde_json::json!({"reason":"retry", "input_mode": target.input_mode}),
        Some(&target.id),
    )?;
    timeline_repository::append_entry(
        &tx,
        &target.branch_id,
        timeline_kind::NARRATION_SELECTED,
        "hidden",
        None,
        &serde_json::json!({"selected_entry_id": variant.id, "reason":"retry"}),
        Some(&target.id),
    )?;
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    get_active_story_entry(&conn, &target.id)
}

/// Best-effort kick-off of the ChatGPT/Gemini-style auto-title: once a story
/// has its first passage, ask the text model to name it. Never blocks or
/// fails the passage write — generation is fire-and-forget and reports back
/// via the `story-title-updated` event (see `stories::maybe_auto_title`).
fn kick_auto_title(app: &AppHandle, pool: &Pool, branch_id: &str) {
    let story_id = {
        let Ok(conn) = pool.get() else { return };
        match get_story_id_for_branch(&conn, branch_id) {
            Ok(id) => id,
            Err(_) => return,
        }
    };
    stories::maybe_auto_title(app, pool, &story_id);
}

/// Spawns the background narration stream shared by every path that produces
/// or updates a narrator passage without blocking the command's return:
/// `narration-delta` / `narration-thoughts` fire as text arrives, then
/// `on_success` runs with the final text (and should emit its own terminal
/// event), or `narration-error` fires if the stream or `on_success` fails.
/// `extra_preamble` folds in Stage 1/2 context (attribute snapshot, roll
/// outcome constraint) ahead of the narrator's own system preamble.
struct NarrationJob {
    app: AppHandle,
    pool: Pool,
    branch_id: String,
    config: TextModelConfig,
    history: Vec<HistoryTurn>,
    prompt: String,
    extra_preamble: String,
    /// Non-empty only for `submit_turn`'s tool-calling path (see `tools`
    /// module) — every other narration-triggering command passes `Vec::new()`.
    tools: Vec<DynamicTool>,
    stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct NarrationToolActivityPayload<'a> {
    stream_id: &'a str,
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
        branch_id,
        config,
        history,
        prompt,
        extra_preamble,
        tools,
        stream_id,
    } = job;
    tauri::async_runtime::spawn(async move {
        let preamble = if extra_preamble.is_empty() {
            NARRATOR_PREAMBLE.to_string()
        } else {
            format!("{NARRATOR_PREAMBLE}\n\n{extra_preamble}")
        };
        let history = crate::features::timeline::compaction::prepare_history(
            &pool, &branch_id, &config, &preamble, &prompt, history,
        )
        .await;
        let req = NarrateRequest {
            config,
            preamble,
            history,
            prompt,
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

/// Shared terminal step for the plain (non-mechanics) append flows: insert
/// the passage, persist a roll if one was already resolved for it, emit
/// `narration-done`.
async fn finish_append(
    app: AppHandle,
    pool: Pool,
    stream_id: String,
    branch_id: String,
    input_mode: String,
    visible: String,
    thoughts: Option<String>,
) -> AppResult<()> {
    let passage = append_narration_entry(
        &pool,
        &branch_id,
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
    kick_auto_title(&app, &pool, &branch_id);
    images::maybe_auto_image(&app, &pool, &entry_id, &visible);
    Ok(())
}

/// Instructs the narrator on when to use `roll_check`, phrased from the
/// story's dice-mode setting — replaces the old classifier's deterministic
/// gate with guidance the narrator applies itself.
fn dice_mode_instruction(dice_mode: DiceMode) -> &'static str {
    match dice_mode {
        DiceMode::Always => {
            "Call the roll_check tool for every meaningful action before narrating its outcome."
        }
        DiceMode::Classifier => {
            "Call the roll_check tool only when the outcome is genuinely uncertain — routine or \
             clearly one-sided actions don't need it."
        }
        DiceMode::Never => "Do not call the roll_check tool; narrate outcomes purely from context.",
    }
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
    branch_id: String,
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

    let history = load_history(&pool, &branch_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let story_id = {
        let conn = pool.get()?;
        get_story_id_for_branch(&conn, &branch_id)?
    };
    let mechanics =
        mechanics_settings::get_story_mechanics_settings(pool.clone(), story_id.clone())?;

    let player_passage = {
        let conn = pool.get()?;
        insert_story_entry(&conn, &branch_id, "player", &input_mode, content, None)?
    };

    let prompt = format_prompt(&input_mode, content);

    let (tool_set, staging) = if mechanics.attributes_enabled {
        let staging = Arc::new(Mutex::new(TurnStaging::new(
            pool.inner().clone(),
            story_id.clone(),
            branch_id.clone(),
        )));
        let tool_set = tools::narrator_tools(staging.clone(), config.clone());
        (tool_set, Some(staging))
    } else {
        (Vec::new(), None)
    };

    let tools_preamble = if tool_set.is_empty() {
        String::new()
    } else {
        let dice_mode = DiceMode::from_str_or_default(&mechanics.dice_mode);
        format!(
            "You have tools to check, create, and update entities and their attributes as the story \
             unfolds — use them to keep the world consistent. {}",
            dice_mode_instruction(dice_mode)
        )
    };
    let extra_preamble = combine_preambles(&[
        story_context_preamble(pool.inner(), &story_id, &branch_id)?,
        tools_preamble,
    ]);

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();

    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            branch_id: branch_id.clone(),
            config,
            history,
            prompt,
            extra_preamble,
            tools: tool_set,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            let passage = append_narration_entry(
                &pool,
                &branch_id_bg,
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
            kick_auto_title(&app, &pool, &branch_id_bg);
            images::maybe_auto_image(&app, &pool, &passage.id, &visible);
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
    branch_id: String,
    note: String,
) -> AppResult<String> {
    let note = note.trim();
    if note.is_empty() {
        return Err(AppError::Invalid("note must not be empty".into()));
    }

    let history = load_history(&pool, &branch_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let story_id = {
        let conn = pool.get()?;
        get_story_id_for_branch(&conn, &branch_id)?
    };
    let extra_preamble = story_context_preamble(pool.inner(), &story_id, &branch_id)?;

    let stream_id = Uuid::new_v4().to_string();
    let prompt = format!(
        "[Director's note — out of character, steer the story but do not narrate it directly: {note}] \
         Continue the scene, letting that note shape what happens next."
    );
    let branch_id_bg = branch_id.clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            branch_id: branch_id.clone(),
            config,
            history,
            prompt,
            extra_preamble,
            tools: Vec::new(),
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| {
            finish_append(
                app,
                pool,
                sid,
                branch_id_bg,
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
    branch_id: String,
) -> AppResult<String> {
    let history = load_history(&pool, &branch_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let story_id = {
        let conn = pool.get()?;
        get_story_id_for_branch(&conn, &branch_id)?
    };
    let extra_preamble = story_context_preamble(pool.inner(), &story_id, &branch_id)?;

    // A trailing story draft was never rendered (its generation failed), so
    // Continue completes it into prose instead of writing past the note.
    let (prompt, input_mode) = {
        let conn = pool.get()?;
        match get_last_story_entry(&conn, &branch_id)? {
            Some(p) if p.input_mode == "story" => {
                (STORY_CONTINUE_PROMPT, "generated_story".to_string())
            }
            _ => (CONTINUE_PROMPT, "generated_continue".to_string()),
        }
    };

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            branch_id: branch_id.clone(),
            config,
            history,
            prompt: prompt.to_string(),
            extra_preamble,
            tools: Vec::new(),
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| {
            finish_append(app, pool, sid, branch_id_bg, input_mode, visible, thoughts)
        },
    );

    Ok(stream_id)
}

/// "Retry": regenerates the latest narrator passage. The existing persisted
/// passage remains intact while the model is running and is only replaced
/// after a successful generation, so configuration or stream failures cannot
/// destroy the version the user was trying to retry.
#[tauri::command]
pub async fn retry_narration(
    app: AppHandle,
    pool: State<'_, Pool>,
    branch_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_story_entry(&conn, &branch_id)?
            .ok_or_else(|| AppError::Invalid("no narration to retry".into()))?;
        if last.id != entry_id {
            return Err(AppError::Invalid(
                "only the latest narration can be retried".into(),
            ));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid(
                "only a narration entry can be retried".into(),
            ));
        }
        last
    };

    let history = load_history(&pool, &branch_id, Some(target.seq))?;
    let config = settings::resolve_text_model(&app, pool.inner())?;

    let prompt = if target.input_mode == "generated_story" {
        STORY_CONTINUE_PROMPT
    } else {
        CONTINUE_PROMPT
    };

    let story_id = {
        let conn = pool.get()?;
        get_story_id_for_branch(&conn, &branch_id)?
    };
    let extra_preamble = combine_preambles(&[
        story_context_preamble(pool.inner(), &story_id, &branch_id)?,
        roll_context_preamble(pool.inner(), &target.id)?,
    ]);

    let stream_id = Uuid::new_v4().to_string();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            branch_id: branch_id.clone(),
            config,
            history,
            prompt: prompt.to_string(),
            extra_preamble,
            tools: Vec::new(),
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            let passage = append_retry_variant(&pool, &target, &visible, thoughts.as_deref())?;
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
            images::maybe_auto_image(&app, &pool, &target.id, &visible);
            Ok(())
        },
    );

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
    branch_id: String,
    entry_id: String,
) -> AppResult<String> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_story_entry(&conn, &branch_id)?
            .ok_or_else(|| AppError::Invalid("no narration to swipe".into()))?;
        if last.id != entry_id {
            return Err(AppError::Invalid(
                "only the latest narration can be swiped".into(),
            ));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid(
                "only a narration entry can be swiped".into(),
            ));
        }
        last
    };

    let history = load_history(&pool, &branch_id, Some(target.seq))?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let story_id = {
        let conn = pool.get()?;
        get_story_id_for_branch(&conn, &branch_id)?
    };
    let extra_preamble = combine_preambles(&[
        story_context_preamble(pool.inner(), &story_id, &branch_id)?,
        roll_context_preamble(pool.inner(), &target.id)?,
    ]);

    let stream_id = Uuid::new_v4().to_string();
    let prompt = if target.input_mode == "generated_story" {
        STORY_CONTINUE_PROMPT
    } else {
        CONTINUE_PROMPT
    };
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            branch_id: branch_id.clone(),
            config,
            history,
            prompt: prompt.to_string(),
            extra_preamble,
            tools: Vec::new(),
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, _thoughts| async move {
            let mut conn = pool.get()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let image_paths = image_paths_for_entry(&tx, &target.id)?;
            tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [&target.id])?;
            tx.execute(
                "DELETE FROM timeline_entries WHERE kind = ?1 AND target_entry_id = ?2",
                rusqlite::params![timeline_kind::IMAGE_GENERATED, target.id],
            )?;
            let variant = timeline_repository::append_entry(
                &tx,
                &target.branch_id,
                timeline_kind::NARRATION_VARIANT,
                "hidden",
                Some(&visible),
                &serde_json::json!({"reason":"swipe", "input_mode": target.input_mode}),
                Some(&target.id),
            )?;
            timeline_repository::append_entry(
                &tx,
                &target.branch_id,
                timeline_kind::NARRATION_SELECTED,
                "hidden",
                None,
                &serde_json::json!({"selected_entry_id": variant.id, "reason":"swipe"}),
                Some(&target.id),
            )?;
            tx.commit()?;

            for path in image_paths {
                let _ = std::fs::remove_file(path);
            }

            let entry = timeline_repository::active_entry(&conn, &target.id)?;
            let variants = super::repository::list_variants(&conn, &target.id)?;

            let _ = app.emit(
                "swipe-done",
                SwipeDonePayload {
                    stream_id: sid,
                    entry,
                    variants,
                },
            );
            images::maybe_auto_image(&app, &pool, &target.id, &visible);
            Ok(())
        },
    );

    Ok(stream_id)
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
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let image_paths = image_paths_for_entry(&tx, &entry_id)?;
    let target = timeline_repository::get_entry(&tx, &entry_id)?;
    if variant_entry_id != entry_id {
        let variant = timeline_repository::get_entry(&tx, &variant_entry_id)?;
        if variant.kind != timeline_kind::NARRATION_VARIANT
            || variant.target_entry_id.as_deref() != Some(&entry_id)
        {
            return Err(AppError::NotFound(format!(
                "variant {variant_entry_id} not found"
            )));
        }
    }
    timeline_repository::append_entry(
        &tx,
        &target.branch_id,
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
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    timeline_repository::active_entry(&conn, &entry_id)
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
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let image_paths = image_paths_for_entry(&tx, &entry_id)?;
    let target = timeline_repository::get_entry(&tx, &entry_id)?;
    let raw = timeline_repository::list_logical_entries(&tx, &target.branch_id)?;
    let applies_to = reducer::active_variant_id(&raw, &entry_id);
    timeline_repository::append_entry(
        &tx,
        &target.branch_id,
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
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }
    timeline_repository::active_entry(&conn, &entry_id)
}

fn erase_last_exchange_in_conn(
    conn: &mut rusqlite::Connection,
    branch_id: &str,
) -> AppResult<(Vec<String>, Vec<String>)> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let Some(last) = get_last_story_entry(&tx, branch_id)? else {
        return Ok((vec![], vec![]));
    };
    let mut removed = vec![last.id.clone()];
    let mut image_paths = image_paths_for_entry(&tx, &last.id)?;
    tx.execute("DELETE FROM timeline_entries WHERE id = ?1", [&last.id])?;

    let paired = last.role == "narrator"
        && matches!(last.input_mode.as_str(), "generated" | "generated_story");
    if paired {
        if let Some(prev) = get_last_story_entry(&tx, branch_id)? {
            if prev.role == "player" || prev.input_mode == "story" {
                image_paths.extend(image_paths_for_entry(&tx, &prev.id)?);
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
            "SELECT id, payload_json FROM timeline_entries WHERE branch_id = ?1 AND kind = ?2",
        )?;
        let summaries: Vec<(String, String)> = stmt
            .query_map(
                rusqlite::params![branch_id, timeline_kind::CONTEXT_SUMMARY],
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
            if through_entry_id.is_some_and(|through| removed.contains(&through)) {
                tx.execute("DELETE FROM timeline_entries WHERE id = ?1", [&summary_id])?;
            }
        }
    }

    let now = Utc::now().to_rfc3339();
    crate::features::timeline::projections::rebuild_branch(&tx, branch_id)?;
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (
             SELECT story_id FROM branches WHERE id = ?2
         )",
        rusqlite::params![now, branch_id],
    )?;
    tx.commit()?;

    Ok((removed, image_paths))
}

/// "Erase": removes the most recent exchange — the latest narration plus the
/// player message (or story draft) that triggered it. Returns the IDs removed
/// so the frontend can splice locally.
#[tauri::command]
pub fn erase_last_exchange(pool: State<Pool>, branch_id: String) -> AppResult<Vec<String>> {
    let mut conn = pool.get()?;
    let (removed, image_paths) = erase_last_exchange_in_conn(&mut conn, &branch_id)?;

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
    fn hard_erase_removes_exchange_derivatives_summaries_and_projections() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE branches(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, parent_branch_id TEXT, forked_at_entry_id TEXT);
            CREATE TABLE timeline_entries(id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT REFERENCES timeline_entries(id) ON DELETE CASCADE, created_at TEXT NOT NULL, UNIQUE(branch_id,seq));
            CREATE TABLE image_assets(id TEXT PRIMARY KEY, entry_id TEXT NOT NULL REFERENCES timeline_entries(id) ON DELETE CASCADE, path TEXT NOT NULL, prompt TEXT NOT NULL, seed INTEGER, provider TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE branch_entity_state(branch_id TEXT NOT NULL, entity_id TEXT NOT NULL, name TEXT NOT NULL, appearance_anchor TEXT, is_present INTEGER NOT NULL, updated_at TEXT NOT NULL, last_event_id TEXT, PRIMARY KEY(branch_id,entity_id));
            CREATE TABLE entity_attributes(branch_id TEXT NOT NULL, entity_id TEXT NOT NULL, attribute_id TEXT NOT NULL, value REAL NOT NULL, source TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_id TEXT, PRIMARY KEY(branch_id,entity_id,attribute_id));
            INSERT INTO stories VALUES ('s','now');
            INSERT INTO branches VALUES ('b','s',NULL,NULL);").unwrap();

        let player = append_entry(
            &conn,
            "b",
            timeline_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            timeline_kind::ENTITY_CREATED,
            "hidden",
            Some("Actor created"),
            &json!({"entity_id":"actor","name":"Actor"}),
            Some(&player.id),
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "b",
            timeline_kind::NARRATION,
            "visible",
            Some("result"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            timeline_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("summary"),
            &json!({"through_entry_id":narration.id.clone()}),
            None,
        )
        .unwrap();
        conn.execute("INSERT INTO image_assets VALUES ('image',?1,'C:/tmp/image.png','prompt',NULL,'test','now')", [&narration.id]).unwrap();
        conn.execute(
            "INSERT INTO branch_entity_state VALUES ('b','stale','Stale',NULL,1,'now',NULL)",
            [],
        )
        .unwrap();

        let (removed, paths) = erase_last_exchange_in_conn(&mut conn, "b").unwrap();
        assert_eq!(removed, vec![narration.id, player.id]);
        assert_eq!(paths, vec!["C:/tmp/image.png"]);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM timeline_entries", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM branch_entity_state", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
