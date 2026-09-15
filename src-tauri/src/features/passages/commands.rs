use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::ai::{self, HistoryTurn, NarrateRequest, NarratorChunk, TextModelConfig};
use crate::features::{
    images,
    mechanics::{
        commands as mechanics_settings,
        pipeline::{self, DiceMode, PendingRoll},
    },
    settings, stories,
};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::history::load_history;
use super::model::{Passage, PassageVariant};
use super::repository::{
    get_last_passage, get_passage, get_story_id_for_branch, image_paths_for_passage,
    insert_passage, row_to_variant,
};

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

#[tauri::command]
pub fn list_passages(pool: State<Pool>, branch_id: String) -> AppResult<Vec<Passage>> {
    let conn = pool.get()?;
    super::repository::list_passages(&conn, &branch_id)
}

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
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    let authored = {
        let conn = pool.get()?;
        insert_passage(&conn, &branch_id, "narrator", "story", content, None)?
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
            config,
            history,
            prompt: STORY_CONTINUE_PROMPT.to_string(),
            extra_preamble,
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

    Ok(SubmitTurnResult {
        passage: authored,
        stream_id,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitTurnResult {
    pub passage: Passage,
    pub stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetryResult {
    pub removed_passage_id: String,
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
    passage: Passage,
}

#[derive(Debug, Clone, Serialize)]
struct SwipeDonePayload {
    stream_id: String,
    passage: Passage,
    variants: Vec<PassageVariant>,
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

fn combine_preambles(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Inserts a fresh narrator passage and emits `narration-done` — the shared
/// terminal step for every flow that appends a new passage (turn, guide, and
/// continue). Retry and Swipe update an existing passage in place.
/// If a Stage 1/2 roll produced this passage, it's persisted here too, now
/// that the passage it belongs to actually exists.
fn append_narrator_passage(
    pool: &Pool,
    branch_id: &str,
    input_mode: &str,
    visible: &str,
    thoughts: Option<&str>,
    roll: Option<PendingRoll>,
) -> AppResult<Passage> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let passage = insert_passage(&tx, branch_id, "narrator", input_mode, visible, thoughts)?;
    if let Some(roll) = roll {
        pipeline::persist_roll(&tx, &passage.id, roll)?;
    }
    tx.commit()?;
    Ok(passage)
}

/// Replaces a completed narrator passage in place after Retry has generated a
/// valid successor. Keeping the same passage id preserves its position and
/// any mechanical roll tied to the action, while variants and scene images
/// derived from the old prose are discarded.
fn replace_narrator_passage(
    pool: &Pool,
    target: &Passage,
    visible: &str,
    thoughts: Option<&str>,
) -> AppResult<Passage> {
    let mut conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let image_paths = image_paths_for_passage(&tx, &target.id)?;
    tx.execute(
        "UPDATE passages SET content = ?1, thoughts = ?2, edited_at = ?3 WHERE id = ?4",
        rusqlite::params![visible, thoughts, now, target.id],
    )?;
    tx.execute(
        "DELETE FROM passage_variants WHERE passage_id = ?1",
        [&target.id],
    )?;
    tx.execute("DELETE FROM images WHERE passage_id = ?1", [&target.id])?;
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (
             SELECT story_id FROM branches WHERE id = ?2
         )",
        rusqlite::params![now, target.branch_id],
    )?;
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    get_passage(&conn, &target.id)
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
    config: TextModelConfig,
    history: Vec<HistoryTurn>,
    prompt: String,
    extra_preamble: String,
    stream_id: String,
}

fn spawn_narration<F, Fut>(job: NarrationJob, on_success: F)
where
    F: FnOnce(AppHandle, Pool, String, String, Option<String>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = AppResult<()>> + Send + 'static,
{
    let NarrationJob {
        app,
        pool,
        config,
        history,
        prompt,
        extra_preamble,
        stream_id,
    } = job;
    tauri::async_runtime::spawn(async move {
        let preamble = if extra_preamble.is_empty() {
            NARRATOR_PREAMBLE.to_string()
        } else {
            format!("{NARRATOR_PREAMBLE}\n\n{extra_preamble}")
        };
        let req = NarrateRequest {
            config,
            preamble,
            history,
            prompt,
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
    let passage = append_narrator_passage(
        &pool,
        &branch_id,
        &input_mode,
        &visible,
        thoughts.as_deref(),
        None,
    )?;
    let passage_id = passage.id.clone();
    let _ = app.emit(
        "narration-done",
        NarrationDonePayload { stream_id, passage },
    );
    kick_auto_title(&app, &pool, &branch_id);
    images::maybe_auto_image(&app, &pool, &passage_id, &visible);
    Ok(())
}

/// "Do"/"Say" input modes: persists the player's passage immediately, then
/// streams a narrator continuation in the background (see `spawn_narration`).
/// When Attributes are enabled for the story, Stage 1 (classify) and Stage 2
/// (resolve) run first — cheap/fast and deterministic respectively — so
/// their outcome can be folded into Stage 3's preamble as a hard constraint;
/// Stage 4 (update) runs after the passage is saved.
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
        insert_passage(&conn, &branch_id, "player", &input_mode, content, None)?
    };

    let prompt = format_prompt(&input_mode, content);

    let (extra_preamble, roll_to_persist) = if mechanics.attributes_enabled {
        let recent_context = history
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|t| {
                format!(
                    "{}: {}",
                    if t.is_player { "Player" } else { "Narrator" },
                    t.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let dice_mode = DiceMode::from_str_or_default(&mechanics.dice_mode);
        match pipeline::run_classify_and_resolve(
            pool.inner(),
            &config,
            &story_id,
            dice_mode,
            &recent_context,
            &prompt,
        )
        .await
        {
            Ok(result) => (result.extra_preamble, result.roll_to_persist),
            Err(_) => (String::new(), None),
        }
    } else {
        (String::new(), None)
    };
    let extra_preamble = combine_preambles(&[
        author_note_preamble(pool.inner(), &story_id)?,
        extra_preamble,
    ]);

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();
    let attributes_enabled = mechanics.attributes_enabled;
    let story_id_bg = story_id.clone();
    let config_bg = config.clone();

    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            config,
            history,
            prompt,
            extra_preamble,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            let passage = append_narrator_passage(
                &pool,
                &branch_id_bg,
                "generated",
                &visible,
                thoughts.as_deref(),
                roll_to_persist,
            )?;
            let _ = app.emit(
                "narration-done",
                NarrationDonePayload {
                    stream_id: sid,
                    passage: passage.clone(),
                },
            );
            kick_auto_title(&app, &pool, &branch_id_bg);
            images::maybe_auto_image(&app, &pool, &passage.id, &visible);
            if attributes_enabled {
                let _ =
                    pipeline::run_update(&pool, &config_bg, &story_id_bg, &passage.id, &visible)
                        .await;
            }
            Ok(())
        },
    );

    Ok(SubmitTurnResult {
        passage: player_passage,
        stream_id,
    })
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
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

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
            config,
            history,
            prompt,
            extra_preamble,
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
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    // A trailing story draft was never rendered (its generation failed), so
    // Continue completes it into prose instead of writing past the note.
    let (prompt, input_mode) = {
        let conn = pool.get()?;
        match get_last_passage(&conn, &branch_id)? {
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
            config,
            history,
            prompt: prompt.to_string(),
            extra_preamble,
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
pub async fn retry_passage(
    app: AppHandle,
    pool: State<'_, Pool>,
    branch_id: String,
    passage_id: String,
) -> AppResult<RetryResult> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_passage(&conn, &branch_id)?
            .ok_or_else(|| AppError::Invalid("no passages to retry".into()))?;
        if last.id != passage_id {
            return Err(AppError::Invalid(
                "only the latest passage can be retried".into(),
            ));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid(
                "only a narrator passage can be retried".into(),
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
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    let stream_id = Uuid::new_v4().to_string();
    spawn_narration(
        NarrationJob {
            app,
            pool: pool.inner().clone(),
            config,
            history,
            prompt: prompt.to_string(),
            extra_preamble,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            let passage = replace_narrator_passage(&pool, &target, &visible, thoughts.as_deref())?;
            let _ = app.emit(
                "narration-done",
                NarrationDonePayload {
                    stream_id: sid,
                    passage: passage.clone(),
                },
            );
            images::maybe_auto_image(&app, &pool, &target.id, &visible);
            Ok(())
        },
    );

    Ok(RetryResult {
        removed_passage_id: passage_id,
        stream_id,
    })
}

/// "Swipe": generates an alternate variant of the current (latest) narrator
/// passage and pages between variants without destroying any — unlike Retry,
/// which replaces. Updates the passage in place and emits `swipe-done` with
/// the full variant list.
#[tauri::command]
pub async fn swipe_passage(
    app: AppHandle,
    pool: State<'_, Pool>,
    branch_id: String,
    passage_id: String,
) -> AppResult<String> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_passage(&conn, &branch_id)?
            .ok_or_else(|| AppError::Invalid("no passages to swipe".into()))?;
        if last.id != passage_id {
            return Err(AppError::Invalid(
                "only the latest passage can be swiped".into(),
            ));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid(
                "only a narrator passage can be swiped".into(),
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
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

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
            config,
            history,
            prompt: prompt.to_string(),
            extra_preamble,
            stream_id: stream_id.clone(),
        },
        move |app, pool, sid, visible, thoughts| async move {
            let mut conn = pool.get()?;
            let now = Utc::now().to_rfc3339();
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let image_paths = image_paths_for_passage(&tx, &target.id)?;

            let existing_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM passage_variants WHERE passage_id = ?1",
                [&target.id],
                |r| r.get(0),
            )?;
            if existing_count == 0 {
                let orig_id = Uuid::new_v4().to_string();
                tx.execute(
                "INSERT INTO passage_variants (id, passage_id, content, is_selected, created_at) VALUES (?1, ?2, ?3, 0, ?4)",
                rusqlite::params![orig_id, target.id, target.content, target.created_at],
            )?;
            }

            tx.execute(
                "UPDATE passage_variants SET is_selected = 0 WHERE passage_id = ?1",
                [&target.id],
            )?;
            let new_variant_id = Uuid::new_v4().to_string();
            tx.execute(
            "INSERT INTO passage_variants (id, passage_id, content, is_selected, created_at) VALUES (?1, ?2, ?3, 1, ?4)",
            rusqlite::params![new_variant_id, target.id, visible, now],
        )?;
            tx.execute(
                "UPDATE passages SET content = ?1, thoughts = ?2, edited_at = ?3 WHERE id = ?4",
                rusqlite::params![visible, thoughts, now, target.id],
            )?;
            tx.execute("DELETE FROM images WHERE passage_id = ?1", [&target.id])?;
            tx.execute(
                "UPDATE stories SET updated_at = ?1 WHERE id = (
                     SELECT story_id FROM branches WHERE id = ?2
                 )",
                rusqlite::params![now, target.branch_id],
            )?;
            tx.commit()?;

            for path in image_paths {
                let _ = std::fs::remove_file(path);
            }

            let passage = get_passage(&conn, &target.id)?;
            let mut stmt = conn.prepare(
            "SELECT id, passage_id, content, is_selected, created_at FROM passage_variants WHERE passage_id = ?1 ORDER BY created_at ASC",
        )?;
            let mut variants = Vec::new();
            for r in stmt.query_map([&target.id], row_to_variant)? {
                variants.push(r?);
            }

            let _ = app.emit(
                "swipe-done",
                SwipeDonePayload {
                    stream_id: sid,
                    passage,
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
pub fn list_variants(pool: State<Pool>, passage_id: String) -> AppResult<Vec<PassageVariant>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, passage_id, content, is_selected, created_at FROM passage_variants WHERE passage_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([passage_id], row_to_variant)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[tauri::command]
pub fn switch_variant(
    pool: State<Pool>,
    passage_id: String,
    variant_id: String,
) -> AppResult<Passage> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let image_paths = image_paths_for_passage(&tx, &passage_id)?;
    let content: String = tx
        .query_row(
            "SELECT content FROM passage_variants WHERE id = ?1 AND passage_id = ?2",
            rusqlite::params![variant_id, passage_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("variant {variant_id} not found")))?;
    tx.execute(
        "UPDATE passage_variants SET is_selected = 0 WHERE passage_id = ?1",
        [&passage_id],
    )?;
    tx.execute(
        "UPDATE passage_variants SET is_selected = 1 WHERE id = ?1",
        [&variant_id],
    )?;
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE passages SET content = ?1, edited_at = ?2 WHERE id = ?3",
        rusqlite::params![content, now, passage_id],
    )?;
    tx.execute("DELETE FROM images WHERE passage_id = ?1", [&passage_id])?;
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (
             SELECT branches.story_id FROM branches
             JOIN passages ON passages.branch_id = branches.id
             WHERE passages.id = ?2
         )",
        rusqlite::params![now, passage_id],
    )?;
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    get_passage(&conn, &passage_id)
}

/// "Edit": rewrite any passage in place. If it currently has a selected
/// variant, that variant's stored text is kept in sync too.
#[tauri::command]
pub fn edit_passage(pool: State<Pool>, passage_id: String, content: String) -> AppResult<Passage> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    let mut conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let image_paths = image_paths_for_passage(&tx, &passage_id)?;
    tx.execute(
        "UPDATE passages SET content = ?1, edited_at = ?2 WHERE id = ?3",
        rusqlite::params![content, now, passage_id],
    )?;
    tx.execute(
        "UPDATE passage_variants SET content = ?1 WHERE passage_id = ?2 AND is_selected = 1",
        rusqlite::params![content, passage_id],
    )?;
    tx.execute("DELETE FROM images WHERE passage_id = ?1", [&passage_id])?;
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (
             SELECT branches.story_id FROM branches
             JOIN passages ON passages.branch_id = branches.id
             WHERE passages.id = ?2
         )",
        rusqlite::params![now, passage_id],
    )?;
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }
    get_passage(&conn, &passage_id)
}

/// "Erase": removes the most recent exchange — the latest passage, plus the
/// player passage (or story draft) that triggered it. Returns the ids removed
/// so the frontend can splice locally.
#[tauri::command]
pub fn erase_last_exchange(pool: State<Pool>, branch_id: String) -> AppResult<Vec<String>> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let Some(last) = get_last_passage(&tx, &branch_id)? else {
        return Ok(vec![]);
    };
    let mut removed = vec![last.id.clone()];
    let mut image_paths = image_paths_for_passage(&tx, &last.id)?;
    tx.execute("DELETE FROM passages WHERE id = ?1", [&last.id])?;

    let paired = last.role == "narrator"
        && matches!(last.input_mode.as_str(), "generated" | "generated_story");
    if paired {
        if let Some(prev) = get_last_passage(&tx, &branch_id)? {
            if prev.role == "player" || prev.input_mode == "story" {
                image_paths.extend(image_paths_for_passage(&tx, &prev.id)?);
                removed.push(prev.id.clone());
                tx.execute("DELETE FROM passages WHERE id = ?1", [&prev.id])?;
            }
        }
    }

    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (
             SELECT story_id FROM branches WHERE id = ?2
         )",
        rusqlite::params![now, branch_id],
    )?;
    tx.commit()?;

    for path in image_paths {
        let _ = std::fs::remove_file(path);
    }

    Ok(removed)
}
