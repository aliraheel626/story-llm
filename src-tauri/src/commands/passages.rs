use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::commands::{mechanics as mechanics_settings, settings, stories};
use crate::db::{Pool, PooledConn};
use crate::error::{AppError, AppResult};
use crate::mechanics::pipeline::{self, DiceMode, PendingRoll};
use crate::models::{Passage, PassageVariant};
use crate::narrator::{self, HistoryTurn, NarrateRequest, NarratorChunk, TextModelConfig, TextProviderKind};

const HISTORY_LIMIT: i64 = 60;

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

fn row_to_passage(row: &rusqlite::Row) -> rusqlite::Result<Passage> {
    Ok(Passage {
        id: row.get(0)?,
        branch_id: row.get(1)?,
        seq: row.get(2)?,
        role: row.get(3)?,
        input_mode: row.get(4)?,
        content: row.get(5)?,
        thoughts: row.get(6)?,
        created_at: row.get(7)?,
        edited_at: row.get(8)?,
    })
}

fn row_to_variant(row: &rusqlite::Row) -> rusqlite::Result<PassageVariant> {
    Ok(PassageVariant {
        id: row.get(0)?,
        passage_id: row.get(1)?,
        content: row.get(2)?,
        is_selected: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
    })
}

fn next_seq(conn: &PooledConn, branch_id: &str) -> AppResult<i64> {
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM passages WHERE branch_id = ?1",
        [branch_id],
        |row| row.get(0),
    )?;
    Ok(seq)
}

fn get_passage(conn: &PooledConn, passage_id: &str) -> AppResult<Passage> {
    conn.query_row(
        "SELECT id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at FROM passages WHERE id = ?1",
        [passage_id],
        row_to_passage,
    )
    .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))
}

fn get_story_id_for_branch(conn: &PooledConn, branch_id: &str) -> AppResult<String> {
    conn.query_row("SELECT story_id FROM branches WHERE id = ?1", [branch_id], |r| r.get(0))
        .map_err(|_| AppError::NotFound(format!("branch {branch_id} not found")))
}

fn get_last_passage(conn: &PooledConn, branch_id: &str) -> AppResult<Option<Passage>> {
    conn.query_row(
        "SELECT id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at
         FROM passages WHERE branch_id = ?1 ORDER BY seq DESC LIMIT 1",
        [branch_id],
        row_to_passage,
    )
    .optional()
    .map_err(AppError::from)
}

fn insert_passage(
    conn: &PooledConn,
    branch_id: &str,
    role: &str,
    input_mode: &str,
    content: &str,
    thoughts: Option<&str>,
) -> AppResult<Passage> {
    let id = Uuid::new_v4().to_string();
    let seq = next_seq(conn, branch_id)?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO passages (id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        rusqlite::params![id, branch_id, seq, role, input_mode, content, thoughts, now],
    )?;
    Ok(Passage {
        id,
        branch_id: branch_id.to_string(),
        seq,
        role: role.to_string(),
        input_mode: input_mode.to_string(),
        content: content.to_string(),
        thoughts: thoughts.map(|s| s.to_string()),
        created_at: now,
        edited_at: None,
    })
}

#[tauri::command]
pub fn list_passages(pool: State<Pool>, branch_id: String) -> AppResult<Vec<Passage>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at
         FROM passages WHERE branch_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([branch_id], row_to_passage)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// "Story" input mode: narration typed directly by the player. The authored
/// text is inserted verbatim as a narrator passage, then the model continues
/// from it in a streamed passage, same as any other generation path.
#[tauri::command]
pub fn submit_story(app: AppHandle, pool: State<Pool>, branch_id: String, content: String) -> AppResult<SubmitTurnResult> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }

    // Resolve everything that can fail before inserting: with no model
    // configured this must error without leaving a draft row the UI never saw.
    let config = resolve_text_model(&app, pool.inner())?;
    let story_id = { let conn = pool.get()?; get_story_id_for_branch(&conn, &branch_id)? };
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
    spawn_narration(app, pool.inner().clone(), config, history, STORY_CONTINUE_PROMPT.to_string(), extra_preamble, stream_id.clone(), move |app, pool, sid, visible, thoughts| {
        finish_append(app, pool, sid, branch_id_bg, "generated_story".to_string(), visible, thoughts)
    });

    Ok(SubmitTurnResult { passage: authored, stream_id })
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

/// Loads up to `HISTORY_LIMIT` prior passages for a branch, oldest first.
/// `before_seq` excludes the passage being regenerated (retry re-queries
/// after deleting it, so it never needs this; swipe keeps the passage in
/// place and needs the cutoff).
///
/// Story-mode drafts are filtered by `drop_stale_story_drafts` before the
/// window is returned.
fn load_history(pool: &Pool, branch_id: &str, before_seq: Option<i64>) -> AppResult<Vec<HistoryTurn>> {
    let conn = pool.get()?;
    let mut rows_out: Vec<(String, HistoryTurn)> = Vec::new();
    // The most recent image generated for a passage (if any) is folded into
    // its history content, so the narrator stays consistent with what a
    // scene image already showed instead of contradicting it later.
    const SELECT: &str = "SELECT passages.role, passages.input_mode, passages.content,
        (SELECT images.prompt FROM images WHERE images.passage_id = passages.id ORDER BY images.created_at DESC LIMIT 1)
        FROM passages WHERE passages.branch_id = ?1";
    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<(String, HistoryTurn)> {
        let role: String = row.get(0)?;
        let input_mode: String = row.get(1)?;
        let content: String = row.get(2)?;
        let image_prompt: Option<String> = row.get(3)?;
        let content = match image_prompt {
            Some(prompt) => format!("{content}\n\n[A scene image was generated here, depicting: {prompt}]"),
            None => content,
        };
        Ok((input_mode, HistoryTurn { is_player: role == "player", content }))
    };
    if let Some(seq) = before_seq {
        let mut stmt = conn.prepare(&format!("{SELECT} AND passages.seq < ?2 ORDER BY passages.seq DESC LIMIT ?3"))?;
        let rows = stmt.query_map(rusqlite::params![branch_id, seq, HISTORY_LIMIT], map_row)?;
        for r in rows {
            rows_out.push(r?);
        }
    } else {
        let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY passages.seq DESC LIMIT ?2"))?;
        let rows = stmt.query_map(rusqlite::params![branch_id, HISTORY_LIMIT], map_row)?;
        for r in rows {
            rows_out.push(r?);
        }
    }
    rows_out.reverse();
    Ok(drop_stale_story_drafts(rows_out))
}

/// Drops Story-mode drafts from a history window (oldest first) — they are
/// player input, not assistant prose, and their `generated_story` completion
/// restates the same beat, so keeping both would double it in context. The
/// newest entry is kept even when it's a draft, so a generation can complete
/// the draft it was given; any older draft — completed or stranded — is
/// excluded.
fn drop_stale_story_drafts(rows: Vec<(String, HistoryTurn)>) -> Vec<HistoryTurn> {
    let newest = rows.len().saturating_sub(1);
    rows.into_iter()
        .enumerate()
        .filter(|(i, (input_mode, _))| input_mode != "story" || *i == newest)
        .map(|(_, (_, turn))| turn)
        .collect()
}

/// Author's Note (spec §6.6), formatted for the preamble, if the story has
/// one set. Empty string when there isn't one, so callers can always append
/// it unconditionally via `combine_preambles`.
fn author_note_preamble(pool: &Pool, story_id: &str) -> AppResult<String> {
    match crate::commands::stories::read_author_note(pool, story_id)? {
        Some(note) => Ok(format!("Author's note — keep this in mind throughout: {note}")),
        None => Ok(String::new()),
    }
}

fn combine_preambles(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join("\n\n")
}

pub(crate) fn resolve_text_model(app: &AppHandle, pool: &Pool) -> AppResult<TextModelConfig> {
    let settings = settings::read_text_model_settings(app, pool)?;
    if settings.model.trim().is_empty() {
        return Err(AppError::Invalid("no text model configured yet — set one in the Text Model panel".into()));
    }
    let api_key = settings::read_api_key(app, &settings.provider)?;
    let provider = match settings.provider.as_str() {
        "openrouter" => TextProviderKind::OpenRouter,
        other => return Err(AppError::Invalid(format!("unsupported provider: {other}"))),
    };
    Ok(TextModelConfig { provider, model: settings.model, api_key })
}

/// Inserts a fresh narrator passage and emits `narration-done` — the shared
/// terminal step for every flow that appends a new passage (turn, guide,
/// continue, retry). Swipe does not use this: it updates a passage in place.
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
    let conn = pool.get()?;
    let passage = insert_passage(&conn, branch_id, "narrator", input_mode, visible, thoughts)?;
    if let Some(roll) = roll {
        pipeline::persist_roll(pool, &passage.id, roll)?;
    }
    Ok(passage)
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
fn spawn_narration<F, Fut>(
    app: AppHandle,
    pool: Pool,
    config: TextModelConfig,
    history: Vec<HistoryTurn>,
    prompt: String,
    extra_preamble: String,
    stream_id: String,
    on_success: F,
) where
    F: FnOnce(AppHandle, Pool, String, String, Option<String>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = AppResult<()>> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let preamble =
            if extra_preamble.is_empty() { NARRATOR_PREAMBLE.to_string() } else { format!("{NARRATOR_PREAMBLE}\n\n{extra_preamble}") };
        let req = NarrateRequest { config, preamble, history, prompt };

        let app_for_chunks = app.clone();
        let stream_id_for_chunks = stream_id.clone();
        let result = narrator::stream_narration(req, move |chunk| match chunk {
            NarratorChunk::Text(text) => {
                let _ = app_for_chunks.emit(
                    "narration-delta",
                    NarrationDeltaPayload { stream_id: &stream_id_for_chunks, text: &text },
                );
            }
            NarratorChunk::Reasoning(text) => {
                let _ = app_for_chunks.emit(
                    "narration-thoughts",
                    NarrationDeltaPayload { stream_id: &stream_id_for_chunks, text: &text },
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
                        NarrationErrorPayload { stream_id: &stream_id, message: "the model returned no visible text" },
                    );
                    return;
                }
                let thoughts = thoughts.trim();
                let thoughts_opt = if thoughts.is_empty() { None } else { Some(thoughts.to_string()) };
                if let Err(e) = on_success(app.clone(), pool, stream_id.clone(), visible, thoughts_opt).await {
                    let _ = app.emit("narration-error", NarrationErrorPayload { stream_id: &stream_id, message: &e.to_string() });
                }
            }
            Err(e) => {
                let _ = app.emit("narration-error", NarrationErrorPayload { stream_id: &stream_id, message: &e.to_string() });
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
    let passage = append_narrator_passage(&pool, &branch_id, &input_mode, &visible, thoughts.as_deref(), None)?;
    let _ = app.emit("narration-done", NarrationDonePayload { stream_id, passage });
    kick_auto_title(&app, &pool, &branch_id);
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
        return Err(AppError::Invalid(format!("invalid input_mode: {input_mode}")));
    }

    let history = load_history(&pool, &branch_id, None)?;
    let config = resolve_text_model(&app, pool.inner())?;
    let story_id = { let conn = pool.get()?; get_story_id_for_branch(&conn, &branch_id)? };
    let mechanics = mechanics_settings::get_story_mechanics_settings(pool.clone(), story_id.clone())?;

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
            .map(|t| format!("{}: {}", if t.is_player { "Player" } else { "Narrator" }, t.content))
            .collect::<Vec<_>>()
            .join("\n");
        let dice_mode = DiceMode::from_str_or_default(&mechanics.dice_mode);
        match pipeline::run_classify_and_resolve(pool.inner(), &config, &story_id, dice_mode, &recent_context, &prompt).await {
            Ok(result) => (result.extra_preamble, result.roll_to_persist),
            Err(_) => (String::new(), None),
        }
    } else {
        (String::new(), None)
    };
    let extra_preamble = combine_preambles(&[author_note_preamble(pool.inner(), &story_id)?, extra_preamble]);

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();
    let attributes_enabled = mechanics.attributes_enabled;
    let story_id_bg = story_id.clone();
    let config_bg = config.clone();

    spawn_narration(app, pool.inner().clone(), config, history, prompt, extra_preamble, stream_id.clone(), move |app, pool, sid, visible, thoughts| async move {
        let passage = append_narrator_passage(&pool, &branch_id_bg, "generated", &visible, thoughts.as_deref(), roll_to_persist)?;
        let _ = app.emit("narration-done", NarrationDonePayload { stream_id: sid, passage: passage.clone() });
        kick_auto_title(&app, &pool, &branch_id_bg);
        if attributes_enabled {
            let _ = pipeline::run_update(&pool, &config_bg, &story_id_bg, &passage.id, &visible).await;
        }
        Ok(())
    });

    Ok(SubmitTurnResult { passage: player_passage, stream_id })
}

/// "Guide" mode: an ephemeral, out-of-character steering note. No player
/// passage is created — the note only shapes this one generation, then it's
/// gone (the spec's Director's Note behavior, exposed directly in the
/// composer instead of a settings field).
#[tauri::command]
pub async fn submit_guide(app: AppHandle, pool: State<'_, Pool>, branch_id: String, note: String) -> AppResult<String> {
    let note = note.trim();
    if note.is_empty() {
        return Err(AppError::Invalid("note must not be empty".into()));
    }

    let history = load_history(&pool, &branch_id, None)?;
    let config = resolve_text_model(&app, pool.inner())?;
    let story_id = { let conn = pool.get()?; get_story_id_for_branch(&conn, &branch_id)? };
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    let stream_id = Uuid::new_v4().to_string();
    let prompt = format!(
        "[Director's note — out of character, steer the story but do not narrate it directly: {note}] \
         Continue the scene, letting that note shape what happens next."
    );
    let branch_id_bg = branch_id.clone();
    spawn_narration(app, pool.inner().clone(), config, history, prompt, extra_preamble, stream_id.clone(), move |app, pool, sid, visible, thoughts| {
        finish_append(app, pool, sid, branch_id_bg, "generated_guide".to_string(), visible, thoughts)
    });

    Ok(stream_id)
}

/// "Continue": narrator advances the scene with no new player input.
#[tauri::command]
pub async fn continue_scene(app: AppHandle, pool: State<'_, Pool>, branch_id: String) -> AppResult<String> {
    let history = load_history(&pool, &branch_id, None)?;
    let config = resolve_text_model(&app, pool.inner())?;
    let story_id = { let conn = pool.get()?; get_story_id_for_branch(&conn, &branch_id)? };
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    // A trailing story draft was never rendered (its generation failed), so
    // Continue completes it into prose instead of writing past the note.
    let (prompt, input_mode) = {
        let conn = pool.get()?;
        match get_last_passage(&conn, &branch_id)? {
            Some(p) if p.input_mode == "story" => (STORY_CONTINUE_PROMPT, "generated_story".to_string()),
            _ => (CONTINUE_PROMPT, "generated_continue".to_string()),
        }
    };

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();
    spawn_narration(app, pool.inner().clone(), config, history, prompt.to_string(), extra_preamble, stream_id.clone(), move |app, pool, sid, visible, thoughts| {
        finish_append(app, pool, sid, branch_id_bg, input_mode, visible, thoughts)
    });

    Ok(stream_id)
}

/// "Retry": discards the latest passage (must be the branch's last, and a
/// narrator passage) and regenerates it. The synchronous return tells the
/// frontend which passage was removed so it can show the new stream in that
/// same slot instead of appending a visible gap.
#[tauri::command]
pub async fn retry_passage(app: AppHandle, pool: State<'_, Pool>, branch_id: String, passage_id: String) -> AppResult<RetryResult> {
    {
        let conn = pool.get()?;
        let last = get_last_passage(&conn, &branch_id)?.ok_or_else(|| AppError::Invalid("no passages to retry".into()))?;
        if last.id != passage_id {
            return Err(AppError::Invalid("only the latest passage can be retried".into()));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid("only a narrator passage can be retried".into()));
        }
        conn.execute("DELETE FROM passages WHERE id = ?1", [&passage_id])?;
    }

    let history = load_history(&pool, &branch_id, None)?;
    let config = resolve_text_model(&app, pool.inner())?;

    // Re-derive pairing from what's now the last passage: if a player action
    // (or story draft) led into this slot, the regenerated passage is paired
    // with it the same way the original was (so a later Erase still removes
    // both correctly).
    let (input_mode, prompt) = {
        let conn = pool.get()?;
        match get_last_passage(&conn, &branch_id)? {
            Some(p) if p.role == "player" => ("generated".to_string(), CONTINUE_PROMPT),
            Some(p) if p.input_mode == "story" => ("generated_story".to_string(), STORY_CONTINUE_PROMPT),
            _ => ("generated_continue".to_string(), CONTINUE_PROMPT),
        }
    };

    let story_id = { let conn = pool.get()?; get_story_id_for_branch(&conn, &branch_id)? };
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    let stream_id = Uuid::new_v4().to_string();
    let branch_id_bg = branch_id.clone();
    spawn_narration(app, pool.inner().clone(), config, history, prompt.to_string(), extra_preamble, stream_id.clone(), move |app, pool, sid, visible, thoughts| {
        finish_append(app, pool, sid, branch_id_bg, input_mode, visible, thoughts)
    });

    Ok(RetryResult { removed_passage_id: passage_id, stream_id })
}

/// "Swipe": generates an alternate variant of the current (latest) narrator
/// passage and pages between variants without destroying any — unlike Retry,
/// which replaces. Updates the passage in place and emits `swipe-done` with
/// the full variant list.
#[tauri::command]
pub async fn swipe_passage(app: AppHandle, pool: State<'_, Pool>, branch_id: String, passage_id: String) -> AppResult<String> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_passage(&conn, &branch_id)?.ok_or_else(|| AppError::Invalid("no passages to swipe".into()))?;
        if last.id != passage_id {
            return Err(AppError::Invalid("only the latest passage can be swiped".into()));
        }
        if last.role != "narrator" {
            return Err(AppError::Invalid("only a narrator passage can be swiped".into()));
        }
        last
    };

    let history = load_history(&pool, &branch_id, Some(target.seq))?;
    let config = resolve_text_model(&app, pool.inner())?;
    let story_id = { let conn = pool.get()?; get_story_id_for_branch(&conn, &branch_id)? };
    let extra_preamble = author_note_preamble(pool.inner(), &story_id)?;

    let stream_id = Uuid::new_v4().to_string();
    let prompt = if target.input_mode == "generated_story" { STORY_CONTINUE_PROMPT } else { CONTINUE_PROMPT };
    spawn_narration(app, pool.inner().clone(), config, history, prompt.to_string(), extra_preamble, stream_id.clone(), move |app, pool, sid, visible, thoughts| async move {
        let mut conn = pool.get()?;
        let now = Utc::now().to_rfc3339();
        let tx = conn.transaction()?;

        let existing_count: i64 =
            tx.query_row("SELECT COUNT(*) FROM passage_variants WHERE passage_id = ?1", [&target.id], |r| r.get(0))?;
        if existing_count == 0 {
            let orig_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO passage_variants (id, passage_id, content, is_selected, created_at) VALUES (?1, ?2, ?3, 0, ?4)",
                rusqlite::params![orig_id, target.id, target.content, target.created_at],
            )?;
        }

        tx.execute("UPDATE passage_variants SET is_selected = 0 WHERE passage_id = ?1", [&target.id])?;
        let new_variant_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO passage_variants (id, passage_id, content, is_selected, created_at) VALUES (?1, ?2, ?3, 1, ?4)",
            rusqlite::params![new_variant_id, target.id, visible, now],
        )?;
        tx.execute(
            "UPDATE passages SET content = ?1, thoughts = ?2, edited_at = ?3 WHERE id = ?4",
            rusqlite::params![visible, thoughts, now, target.id],
        )?;
        tx.commit()?;

        let passage = get_passage(&conn, &target.id)?;
        let mut stmt = conn.prepare(
            "SELECT id, passage_id, content, is_selected, created_at FROM passage_variants WHERE passage_id = ?1 ORDER BY created_at ASC",
        )?;
        let mut variants = Vec::new();
        for r in stmt.query_map([&target.id], row_to_variant)? {
            variants.push(r?);
        }

        let _ = app.emit("swipe-done", SwipeDonePayload { stream_id: sid, passage, variants });
        Ok(())
    });

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
pub fn switch_variant(pool: State<Pool>, passage_id: String, variant_id: String) -> AppResult<Passage> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let content: String = tx
        .query_row(
            "SELECT content FROM passage_variants WHERE id = ?1 AND passage_id = ?2",
            rusqlite::params![variant_id, passage_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("variant {variant_id} not found")))?;
    tx.execute("UPDATE passage_variants SET is_selected = 0 WHERE passage_id = ?1", [&passage_id])?;
    tx.execute("UPDATE passage_variants SET is_selected = 1 WHERE id = ?1", [&variant_id])?;
    let now = Utc::now().to_rfc3339();
    tx.execute("UPDATE passages SET content = ?1, edited_at = ?2 WHERE id = ?3", rusqlite::params![content, now, passage_id])?;
    tx.commit()?;

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
    let conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE passages SET content = ?1, edited_at = ?2 WHERE id = ?3",
        rusqlite::params![content, now, passage_id],
    )?;
    conn.execute(
        "UPDATE passage_variants SET content = ?1 WHERE passage_id = ?2 AND is_selected = 1",
        rusqlite::params![content, passage_id],
    )?;
    get_passage(&conn, &passage_id)
}

/// "Erase": removes the most recent exchange — the latest passage, plus the
/// player passage (or story draft) that triggered it. Returns the ids removed
/// so the frontend can splice locally.
#[tauri::command]
pub fn erase_last_exchange(pool: State<Pool>, branch_id: String) -> AppResult<Vec<String>> {
    let conn = pool.get()?;
    let Some(last) = get_last_passage(&conn, &branch_id)? else {
        return Ok(vec![]);
    };
    let mut removed = vec![last.id.clone()];
    conn.execute("DELETE FROM passages WHERE id = ?1", [&last.id])?;

    let paired = last.role == "narrator" && matches!(last.input_mode.as_str(), "generated" | "generated_story");
    if paired {
        if let Some(prev) = get_last_passage(&conn, &branch_id)? {
            if prev.role == "player" || prev.input_mode == "story" {
                removed.push(prev.id.clone());
                conn.execute("DELETE FROM passages WHERE id = ?1", [&prev.id])?;
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::drop_stale_story_drafts;
    use crate::narrator::HistoryTurn;

    fn turn(content: &str) -> HistoryTurn {
        HistoryTurn { is_player: false, content: content.to_string() }
    }

    #[test]
    fn drops_draft_once_its_completion_is_newest() {
        let rows = vec![
            ("story".to_string(), turn("terse draft")),
            ("generated_story".to_string(), turn("completed passage")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "completed passage");
    }

    #[test]
    fn keeps_trailing_draft_for_completion() {
        let rows = vec![("generated".to_string(), turn("scene")), ("story".to_string(), turn("draft"))];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].content, "draft");
    }

    #[test]
    fn keeps_draft_when_window_excludes_completion() {
        // Swipe passes before_seq, so the completion is cut off and the draft
        // is the newest row again.
        let rows = vec![("story".to_string(), turn("draft"))];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "draft");
    }

    #[test]
    fn drops_stranded_older_draft() {
        let rows = vec![
            ("story".to_string(), turn("stranded draft")),
            ("story".to_string(), turn("newer draft")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "newer draft");
    }

    #[test]
    fn leaves_non_draft_history_untouched() {
        let rows = vec![
            ("do".to_string(), turn("player action")),
            ("generated".to_string(), turn("narration")),
            ("generated_continue".to_string(), turn("continued scene")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 3);
    }
}
