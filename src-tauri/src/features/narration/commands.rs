use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use chrono::Utc;
use r2d2_sqlite::SqliteConnectionManager;
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
    compaction, images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        reducer, repository as ledger_repository,
    },
    settings, stories,
};
use crate::prompts;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::context::{self, combine_context_blocks, ContextPlan};
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
    pub entry: LedgerEntry,
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
    entry: LedgerEntry,
}

#[derive(Debug, Clone, Serialize)]
struct NarrationErrorPayload<'a> {
    stream_id: &'a str,
    message: &'a str,
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
/// `turn_context_plan` supplies the complete per-message context used both for
/// compaction budgeting and for the final action sent to the model.
struct NarrationJob {
    app: AppHandle,
    pool: Pool,
    story_id: String,
    config: TextModelConfig,
    history: Vec<HistoryTurn>,
    turn_context_plan: ContextPlan,
    /// Effective tools for this action or replacement attempt.
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
        let preamble = prompts::narrator_system_prompt();
        let prepared = compaction::prepare_history(
            &pool,
            &story_id,
            &config,
            &preamble,
            &turn_context_plan.full,
            history,
            before_seq,
        )
        .await;
        let mut history = prepared.turns;
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
        action.content = combine_context_blocks(&[turn_context_plan.live, action.content]);
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
    action: LedgerEntry,
    mode: String,
    history: Vec<HistoryTurn>,
    before_seq: Option<i64>,
) -> AppResult<String> {
    let config = settings::resolve_text_model(&app, &pool)?;
    let tool_settings = stories::settings::read_story_narrator_tools(&pool, &story_id)?;
    let reasoning_effort = stories::settings::read_story_reasoning_effort(&pool, &story_id)?;
    let reasoning_effort = (!reasoning_effort.is_empty()).then_some(reasoning_effort);
    let image_settings = settings::read_image_model_settings(&app, &pool)?;
    let is_see = mode == "see";
    let image_enabled =
        image_settings.enabled && image_settings.has_api_key && tool_settings.illustrate_scene;
    let (image_tools, image_requests) = narrator_image_tools(image_enabled);
    if is_see && !image_enabled {
        return Err(AppError::Invalid(
            "image generation is disabled or has no API key".into(),
        ));
    }
    let prior_narration = {
        let conn = pool.get()?;
        let raw = ledger_repository::list_logical_entries(&conn, &story_id)?;
        reducer::active_visible_entries(&raw)
            .into_iter()
            .rev()
            .find(|entry| entry.kind == ledger_kind::NARRATION)
            .map(|entry| (entry.id, entry.content.unwrap_or_default()))
    };
    if is_see && prior_narration.is_none() {
        return Err(AppError::Invalid(
            "there is no narrated scene to illustrate".into(),
        ));
    }

    let (mut tool_set, staging) = if !is_see
        && (tool_settings.get_entities
            || tool_settings.create_entity
            || tool_settings.update_entity
            || tool_settings.adjust_entity_attribute
            || tool_settings.roll_check)
    {
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id.clone())));
        let embedding_api_key = settings::read_api_key(&app, "openrouter").unwrap_or_default();
        (
            tools::narrator_tools_for_settings(staging.clone(), embedding_api_key, &tool_settings),
            Some(staging),
        )
    } else {
        (Vec::new(), None)
    };

    if image_enabled {
        tool_set.extend(image_tools);
    }
    let turn_context_plan = context::build_message_context(&context::Inputs {
        pool: &pool,
        story_id: &story_id,
        history: &history,
        config: &config,
        tool_settings: if is_see { None } else { Some(&tool_settings) },
        image_enabled,
    })?;
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
            let action = insert_story_entry(&conn, &story_id, "player", &mode, content, None)?;
            ledger_repository::active_entry(&conn, &action.id)?
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
    )?;
    Ok(SubmitTurnResult {
        entry: action,
        stream_id,
    })
}

struct RetrySnapshot {
    pool: Pool,
    _path: SnapshotPath,
    /// Exact story ledger at snapshot time, including hidden events. A new
    /// image, edit, tool effect, or action must reject the replacement.
    original_entries: Vec<LedgerEntry>,
    original_updated_at: String,
    original_settings_json: String,
    original_registry_aliases: HashMap<String, String>,
}

struct SnapshotPath(PathBuf);

impl Drop for SnapshotPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn story_revision(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<(Vec<LedgerEntry>, String, String)> {
    let (updated_at, settings_json) = conn.query_row(
        "SELECT updated_at, settings_json FROM stories WHERE id = ?1",
        [story_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((
        ledger_repository::list_logical_entries(conn, story_id)?,
        updated_at,
        settings_json,
    ))
}

fn registry_aliases(conn: &rusqlite::Connection) -> AppResult<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, aliases_json FROM attribute_registry")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
}

fn snapshot_for_retry(pool: &Pool, story_id: &str, entry_id: &str) -> AppResult<RetrySnapshot> {
    let path = SnapshotPath(
        std::env::temp_dir().join(format!("story-llm-retry-{}.sqlite3", Uuid::new_v4())),
    );
    {
        let conn = pool.get()?;
        conn.execute("VACUUM INTO ?1", [path.0.to_string_lossy().as_ref()])?;
    }
    let manager = SqliteConnectionManager::file(&path.0).with_init(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        Ok(())
    });
    let snapshot_pool = r2d2::Pool::new(manager)?;
    let (original_entries, original_updated_at, original_settings_json, original_registry_aliases) = {
        let conn = snapshot_pool.get()?;
        let revision = story_revision(&conn, story_id)?;
        let target = get_last_story_entry(&conn, story_id)?
            .ok_or_else(|| AppError::Invalid("no narration to retry".into()))?;
        if target.id != entry_id || target.role != "narrator" {
            return Err(AppError::Invalid(
                "only the latest narration can be retried".into(),
            ));
        }
        (revision.0, revision.1, revision.2, registry_aliases(&conn)?)
    };
    with_transaction(&snapshot_pool, |tx| {
        remove_reply_in_tx(tx, story_id, entry_id)?;
        Ok(())
    })?;
    Ok(RetrySnapshot {
        pool: snapshot_pool,
        _path: path,
        original_entries,
        original_updated_at,
        original_settings_json,
        original_registry_aliases,
    })
}

fn reconcile_attribute_registry(
    tx: &rusqlite::Transaction<'_>,
    scratch: &rusqlite::Connection,
    original_aliases: &HashMap<String, String>,
) -> AppResult<()> {
    let mut stmt = scratch.prepare(
        "SELECT id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
                is_user_created, created_in_story_id, created_at FROM attribute_registry",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, f64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;
    for row in rows {
        let (id, name, aliases, kinds, min, max, category, user_created, story, created_at) = row?;
        if original_aliases
            .get(&id)
            .is_some_and(|original| original == &aliases)
        {
            continue;
        }
        let live_aliases: Option<String> = tx
            .query_row(
                "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(live_aliases) = live_aliases {
            if live_aliases != aliases {
                let mut combined: Vec<String> = serde_json::from_str(&live_aliases)
                    .map_err(|e| AppError::Other(format!("invalid attribute aliases: {e}")))?;
                for alias in serde_json::from_str::<Vec<String>>(&aliases)
                    .map_err(|e| AppError::Other(format!("invalid attribute aliases: {e}")))?
                {
                    if !combined
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(&alias))
                    {
                        combined.push(alias);
                    }
                }
                tx.execute(
                    "UPDATE attribute_registry SET aliases_json = ?1 WHERE id = ?2",
                    rusqlite::params![
                        serde_json::to_string(&combined)
                            .map_err(|e| AppError::Other(e.to_string()))?,
                        id
                    ],
                )?;
            }
        } else {
            tx.execute(
                "INSERT INTO attribute_registry
                (id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
                 is_user_created, created_in_story_id, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    id,
                    name,
                    aliases,
                    kinds,
                    min,
                    max,
                    category,
                    user_created,
                    story,
                    created_at
                ],
            )?;
        }
    }
    Ok(())
}

async fn replace_narration_entry(
    live: &Pool,
    snapshot: &RetrySnapshot,
    story_id: &str,
    target_id: &str,
    visible: &str,
    thoughts: Option<&str>,
    staging: Option<Arc<Mutex<TurnStaging>>>,
) -> AppResult<(LedgerEntry, Vec<String>)> {
    if visible.trim().is_empty() {
        return Err(AppError::Invalid(
            "retry generated no narration; original was preserved".into(),
        ));
    }
    let guard = match &staging {
        Some(staging) => Some(staging.lock().await),
        None => None,
    };
    let scratch = snapshot.pool.get()?;
    with_transaction(live, |tx| {
        let (entries, updated_at, settings_json) = story_revision(tx, story_id)?;
        if entries != snapshot.original_entries
            || updated_at != snapshot.original_updated_at
            || settings_json != snapshot.original_settings_json
            || get_last_story_entry(tx, story_id)?
                .as_ref()
                .map(|entry| entry.id.as_str())
                != Some(target_id)
        {
            return Err(AppError::Invalid(
                "story changed during retry; original narration was preserved".into(),
            ));
        }
        let paths = remove_reply_in_tx(tx, story_id, target_id)?;
        reconcile_attribute_registry(tx, &scratch, &snapshot.original_registry_aliases)?;
        let passage = insert_story_entry(tx, story_id, "narrator", "generated", visible, thoughts)?;
        if let Some(staging) = &guard {
            staging.commit(tx, &passage.id)?;
        }
        Ok((ledger_repository::active_entry(tx, &passage.id)?, paths))
    })
}

fn start_replacement_generation(
    app: AppHandle,
    pool: State<'_, Pool>,
    story_id: String,
    entry_id: String,
) -> AppResult<String> {
    let target = {
        let conn = pool.get()?;
        let last = get_last_story_entry(&conn, &story_id)?
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

    let snapshot = snapshot_for_retry(&pool, &story_id, &target.id)?;
    let history = load_history(&snapshot.pool, &story_id, None)?;
    let config = settings::resolve_text_model(&app, pool.inner())?;
    let reasoning_effort = stories::settings::read_story_reasoning_effort(pool.inner(), &story_id)?;
    let reasoning_effort = (!reasoning_effort.is_empty()).then_some(reasoning_effort);
    let tool_settings = stories::settings::read_story_narrator_tools(pool.inner(), &story_id)?;
    let image_settings = settings::read_image_model_settings(&app, pool.inner())?;
    let image_enabled =
        image_settings.enabled && image_settings.has_api_key && tool_settings.illustrate_scene;
    let (image_tools, image_requests) = narrator_image_tools(image_enabled);
    let staging = if tool_settings.get_entities
        || tool_settings.create_entity
        || tool_settings.update_entity
        || tool_settings.adjust_entity_attribute
        || tool_settings.roll_check
    {
        Some(Arc::new(Mutex::new(TurnStaging::new(
            snapshot.pool.clone(),
            story_id.clone(),
        ))))
    } else {
        None
    };
    let mut tool_set = if let Some(staging) = &staging {
        let embedding_api_key = settings::read_api_key(&app, "openrouter").unwrap_or_default();
        tools::narrator_tools_for_settings(staging.clone(), embedding_api_key, &tool_settings)
    } else {
        Vec::new()
    };
    tool_set.extend(image_tools);
    let turn_context_plan = context::build_message_context(&context::Inputs {
        pool: &snapshot.pool,
        story_id: &story_id,
        history: &history,
        config: &config,
        tool_settings: Some(&tool_settings),
        image_enabled,
    })?;

    let stream_id = Uuid::new_v4().to_string();
    let story_id_bg = story_id.clone();
    let live_pool = pool.inner().clone();
    spawn_narration(
        NarrationJob {
            app,
            pool: snapshot.pool.clone(),
            story_id,
            config,
            history,
            turn_context_plan,
            tools: tool_set,
            stop_after_tool_result: false,
            reasoning_effort,
            before_seq: None,
            stream_id: stream_id.clone(),
        },
        move |app, _scratch_pool, sid, visible, thoughts| async move {
            let (entry, image_paths) = replace_narration_entry(
                &live_pool,
                &snapshot,
                &story_id_bg,
                &target.id,
                &visible,
                thoughts.as_deref(),
                staging,
            )
            .await?;
            for path in image_paths {
                let _ = std::fs::remove_file(path);
            }
            let _ = app.emit(
                "narration-done",
                NarrationDonePayload {
                    stream_id: sid,
                    entry: entry.clone(),
                },
            );
            let requests = std::mem::take(&mut *image_requests.lock().await);
            if !requests.is_empty() {
                images::generate_from_narrator_requests(
                    &app, &live_pool, &entry.id, &visible, requests, None,
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
            ledger_repository::active_entry(&conn, &action.id)?
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
        )?
    } else {
        start_replacement_generation(app, pool, story_id, entry_id.clone())?
    };
    Ok(RetryResult {
        entry_id,
        stream_id,
    })
}

/// "Edit": append a content override for any visible ledger entry.
#[tauri::command]
pub fn edit_ledger_entry(
    pool: State<Pool>,
    entry_id: String,
    content: String,
) -> AppResult<LedgerEntry> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::Invalid("content must not be empty".into()));
    }
    let (image_paths, entry) = with_transaction(pool.inner(), |tx| {
        let image_paths = image_paths_for_entry(tx, &entry_id)?;
        let target = ledger_repository::get_entry(tx, &entry_id)?;
        ledger_repository::append_entry(
            tx,
            &target.story_id,
            ledger_kind::CONTENT_EDITED,
            "hidden",
            Some(content),
            &serde_json::json!({"reason":"user_edit"}),
            Some(&entry_id),
        )?;
        tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [&entry_id])?;
        tx.execute(
            "DELETE FROM ledger_entries WHERE kind = ?1 AND target_entry_id = ?2",
            rusqlite::params![ledger_kind::IMAGE_GENERATED, entry_id],
        )?;
        Ok((image_paths, ledger_repository::active_entry(tx, &entry_id)?))
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
             SELECT ledger_entries.id FROM ledger_entries
             JOIN doomed ON ledger_entries.target_entry_id = doomed.id
         )
         SELECT ledger_entries.id, ledger_entries.kind, ledger_entries.payload_json
         FROM ledger_entries JOIN doomed ON doomed.id = ledger_entries.id",
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
        if kind == ledger_kind::IMAGE_GENERATED {
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
            ledger_kind::ENTITY_CREATED
                | ledger_kind::ENTITY_UPDATED
                | ledger_kind::ENTITY_DELETED
                | ledger_kind::ENTITY_ATTRIBUTE_CHANGED
                | ledger_kind::ENTITY_ATTRIBUTE_REMOVED
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

fn remove_reply_in_tx(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
    reply_id: &str,
) -> AppResult<Vec<String>> {
    let mut image_paths = image_paths_for_entry(tx, reply_id)?;
    let mut doomed_ids = HashSet::new();
    let mut affected_entities = HashSet::new();
    collect_cascade_effects(
        tx,
        reply_id,
        &mut doomed_ids,
        &mut affected_entities,
        &mut image_paths,
    )?;
    let deleted = tx.execute(
        "DELETE FROM ledger_entries WHERE id = ?1 AND story_id = ?2 AND kind = ?3",
        rusqlite::params![reply_id, story_id, ledger_kind::NARRATION],
    )?;
    if deleted != 1 {
        return Err(AppError::Invalid(
            "narration to replace no longer exists".into(),
        ));
    }
    prune_covered_summaries(tx, story_id, &doomed_ids)?;
    crate::features::ledger::projections::replay_entities(tx, story_id, &affected_entities)?;
    Ok(image_paths)
}

fn prune_covered_summaries(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
    doomed_ids: &HashSet<String>,
) -> AppResult<()> {
    let mut stmt = tx
        .prepare("SELECT id, payload_json FROM ledger_entries WHERE story_id = ?1 AND kind = ?2")?;
    let summaries: Vec<(String, String)> = stmt
        .query_map(
            rusqlite::params![story_id, ledger_kind::CONTEXT_SUMMARY],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<_, _>>()?;
    for (summary_id, payload_json) in summaries {
        let through = serde_json::from_str::<serde_json::Value>(&payload_json)
            .ok()
            .and_then(|value| value.get("through_entry_id")?.as_str().map(str::to_string));
        if through.is_some_and(|id| doomed_ids.contains(&id)) {
            tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [&summary_id])?;
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
    let mut image_paths = Vec::new();
    let mut doomed_ids = HashSet::new();
    let mut affected_entities = HashSet::new();
    if last.role == "narrator" {
        image_paths = remove_reply_in_tx(tx, story_id, &last.id)?;
    } else {
        image_paths.extend(image_paths_for_entry(tx, &last.id)?);
        collect_cascade_effects(
            tx,
            &last.id,
            &mut doomed_ids,
            &mut affected_entities,
            &mut image_paths,
        )?;
        tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [&last.id])?;
    }

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
                tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [&prev.id])?;
            }
        }
    }

    prune_covered_summaries(tx, story_id, &doomed_ids)?;

    let now = Utc::now().to_rfc3339();
    crate::features::ledger::projections::replay_entities(tx, story_id, &affected_entities)?;
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
    use crate::features::ledger::repository::append_entry;
    use serde_json::json;

    #[test]
    fn image_tools_include_only_illustration_when_enabled() {
        let (enabled, _) = narrator_image_tools(true);
        let (disabled, _) = narrator_image_tools(false);

        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name(), "illustrate_scene");
        assert!(disabled.is_empty());
    }

    fn retry_fixture() -> (Pool, String, String) {
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
            "mira",
            "s",
            "character",
            "Mira",
            Some("old cloak"),
            "test",
            None,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "you",
            "s",
            "character",
            "You",
            None,
            "test",
            None,
        )
        .unwrap();
        let action = insert_story_entry(&conn, "s", "player", "do", "Open the door", None).unwrap();
        let reply =
            insert_story_entry(&conn, "s", "narrator", "generated", "Original", None).unwrap();
        crate::features::entities::update_entity_sync(
            &conn,
            "s",
            "mira",
            "Mira Changed",
            Some("new cloak"),
            "narrator_tool",
            Some(&reply.id),
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            ledger_kind::DICEROLL,
            "hidden",
            Some("Old roll"),
            &json!({"roll":99,"chance_percent":50,"seed":123,"outcome":"success"}),
            Some(&reply.id),
        )
        .unwrap();
        (pool, action.id, reply.id)
    }

    #[test]
    fn retry_snapshot_reads_pre_turn_state_without_damaging_live_reply() {
        let (pool, action, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let scratch = snapshot.pool.get().unwrap();
        let entities = crate::features::entities::list_entities_sync(&scratch, "s", None).unwrap();
        assert_eq!(
            entities.iter().find(|e| e.id == "mira").unwrap().name,
            "Mira"
        );
        assert_eq!(
            get_last_story_entry(&scratch, "s").unwrap().unwrap().id,
            action
        );
        assert!(!ledger_repository::list_logical_entries(&scratch, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        let history = load_history(&snapshot.pool, "s", None).unwrap();
        assert_eq!(
            history.last().and_then(|turn| turn.entry_id.as_deref()),
            Some(action.as_str())
        );
        assert!(!history
            .iter()
            .any(|turn| turn.content.contains("Original") || turn.content.contains("Old roll")));
        drop(scratch);
        drop(snapshot);
        let conn = pool.get().unwrap();
        assert_eq!(get_last_story_entry(&conn, "s").unwrap().unwrap().id, reply);
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|e| e.id == "mira")
                .unwrap()
                .name,
            "Mira Changed"
        );
    }

    #[test]
    fn migrated_player_bootstrap_after_reply_does_not_block_retry() {
        let (pool, action, reply) = retry_fixture();
        let conn = pool.get().unwrap();
        append_entry(
            &conn,
            "s",
            ledger_kind::ENTITY_CREATED,
            "hidden",
            Some("You was added as a character."),
            &json!({"entity_id":"you","name":"You","source":"story_bootstrap"}),
            None,
        )
        .unwrap();
        drop(conn);

        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let history = load_history(&snapshot.pool, "s", None).unwrap();
        assert_eq!(
            history.last().and_then(|turn| turn.entry_id.as_deref()),
            Some(action.as_str())
        );
    }

    #[test]
    fn abandoned_retry_removes_its_temporary_snapshot() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let path = snapshot._path.0.clone();
        assert!(path.exists());
        drop(snapshot);
        assert!(!path.exists());
        assert_eq!(
            get_last_story_entry(&pool.get().unwrap(), "s")
                .unwrap()
                .unwrap()
                .id,
            reply
        );
    }

    #[tokio::test]
    async fn retry_replays_you_creation_without_losing_canonical_player() {
        let (pool, _, reply) = retry_fixture();
        let conn = pool.get().unwrap();
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
            "you",
            &trust,
            2.0,
            "original reply",
            &reply,
            false,
        )
        .unwrap();
        drop(conn);
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let scratch = snapshot.pool.get().unwrap();
        assert!(
            crate::features::entities::list_entities_sync(&scratch, "s", None)
                .unwrap()
                .iter()
                .any(|entity| entity.id == "you" && entity.name == "You")
        );
        assert_eq!(
            scratch
                .query_row(
                    "SELECT COUNT(*) FROM entity_attributes WHERE story_id='s' AND entity_id='you'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(scratch);
        replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
            .await
            .unwrap();
        let conn = pool.get().unwrap();
        assert!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .any(|entity| entity.id == "you" && entity.name == "You")
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entity_attributes WHERE story_id='s' AND entity_id='you'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn replacement_removes_old_roll_and_effects_but_keeps_action() {
        let (pool, action, reply) = retry_fixture();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
            VALUES ('old-image', ?1, 'old-image.png', 'prompt', 'now')",
            [&reply],
        )
        .unwrap();
        drop(conn);
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let (new_reply, paths) =
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New outcome", None, None)
                .await
                .unwrap();
        assert_eq!(paths, vec!["old-image.png"]);
        assert_ne!(new_reply.id, reply);
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        assert!(entries.iter().any(|entry| entry.id == action));
        assert!(!entries
            .iter()
            .any(|entry| entry.id == reply || entry.kind == ledger_kind::DICEROLL));
        assert_eq!(new_reply.content.as_deref(), Some("New outcome"));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|e| e.id == "mira")
                .unwrap()
                .name,
            "Mira"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn retry_can_stage_fresh_roll_and_entity_changes_against_pre_turn_world() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let staging = Arc::new(Mutex::new(TurnStaging::new(
            snapshot.pool.clone(),
            "s".into(),
        )));
        let tool_set = tools::narrator_portable_tools_for_settings(
            staging.clone(),
            String::new(),
            &stories::settings::NarratorToolSettings::default(),
        );
        let named = |name: &str| tool_set.iter().find(|tool| tool.name() == name).unwrap();
        let entities = named("get_entities")
            .execute(json!({"name":"Mira"}))
            .await
            .unwrap();
        assert_eq!(
            entities.as_json().unwrap()["entities"][0]["name"],
            json!("Mira")
        );
        let roll = named("roll_check")
            .execute(json!({"chance_percent":100,"reason":"retry"}))
            .await
            .unwrap();
        assert_eq!(roll.as_json().unwrap()["outcome"], json!("success"));
        named("update_entity")
            .execute(json!({"id":"mira", "name":"Mira Retried"}))
            .await
            .unwrap();
        named("create_entity")
            .execute(json!({"kind":"location", "name":"New chamber"}))
            .await
            .unwrap();
        let (entry, _) = replace_narration_entry(
            &pool,
            &snapshot,
            "s",
            &reply,
            "Retried",
            None,
            Some(staging),
        )
        .await
        .unwrap();
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        let rolls = entries
            .iter()
            .filter(|e| e.kind == ledger_kind::DICEROLL)
            .collect::<Vec<_>>();
        assert_eq!(rolls.len(), 1);
        assert_eq!(rolls[0].target_entry_id.as_deref(), Some(entry.id.as_str()));
        assert_eq!(rolls[0].payload["chance_percent"], json!(100));
        let entities = crate::features::entities::list_entities_sync(&conn, "s", None).unwrap();
        assert_eq!(
            entities.iter().find(|e| e.id == "mira").unwrap().name,
            "Mira Retried"
        );
        assert!(entities.iter().any(|e| e.name == "New chamber"));
    }

    #[tokio::test]
    async fn scratch_registry_changes_commit_only_with_a_successful_replacement() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let scratch = snapshot.pool.get().unwrap();
        let id: String = scratch
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Trust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::features::entities::attributes::add_alias(&scratch, &id, "Confidence").unwrap();
        scratch.execute("INSERT INTO attribute_registry
            (id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
             is_user_created, created_in_story_id, created_at)
            VALUES ('new-attribute', 'Aethercraft', '[]', '[\"character\"]', 0, 10, 'user', 1, 's', 'now')", []).unwrap();
        drop(scratch);
        let live = pool.get().unwrap();
        let before: String = live
            .query_row(
                "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!before.contains("Confidence"));
        assert_eq!(
            live.query_row(
                "SELECT COUNT(*) FROM attribute_registry WHERE canonical_name = 'Aethercraft'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        drop(live);
        replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
            .await
            .unwrap();
        let live = pool.get().unwrap();
        let after: String = live
            .query_row(
                "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(after.contains("Confidence"));
        assert_eq!(
            live.query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Aethercraft'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "new-attribute"
        );
    }

    #[tokio::test]
    async fn registry_collision_rolls_back_reply_removal_and_entity_replay() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let insert_registry = "INSERT INTO attribute_registry
            (id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
             is_user_created, created_in_story_id, created_at)
            VALUES (?1, 'Aethercraft', '[]', '[\"character\"]', 0, 10, 'user', 1, 's', 'now')";
        snapshot
            .pool
            .get()
            .unwrap()
            .execute(insert_registry, ["scratch-id"])
            .unwrap();
        pool.get()
            .unwrap()
            .execute(insert_registry, ["live-id"])
            .unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
        assert_eq!(get_last_story_entry(&conn, "s").unwrap().unwrap().id, reply);
        assert!(ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|entity| entity.id == "mira")
                .unwrap()
                .name,
            "Mira Changed"
        );
    }

    #[tokio::test]
    async fn empty_generation_and_concurrent_edit_preserve_original_and_roll() {
        let (pool, action, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, " ", None, None)
                .await
                .is_err()
        );
        with_transaction(&pool, |tx| {
            ledger_repository::append_entry(
                tx,
                "s",
                ledger_kind::CONTENT_EDITED,
                "hidden",
                Some("Player edited while retry ran"),
                &json!({"reason":"user_edit"}),
                Some(&action),
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.id == reply && entry.content.as_deref() == Some("Original")));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|e| e.id == "mira")
                .unwrap()
                .name,
            "Mira Changed"
        );
    }

    #[tokio::test]
    async fn changed_story_settings_reject_retry_even_when_timestamp_is_unchanged() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        pool.get().unwrap().execute(
            "UPDATE stories SET settings_json = '{\"author_note\":\"new guidance\"}' WHERE id = 's'", [],
        ).unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
        assert_eq!(get_last_story_entry(&conn, "s").unwrap().unwrap().id, reply);
        assert!(ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
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
                ledger_kind::PLAYER_MESSAGE,
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
                ledger_kind::NARRATION,
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
            ledger_kind::NARRATION,
            "visible",
            Some("A moonlit harbor."),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let see = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
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
            ledger_kind::IMAGE_GENERATED,
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
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&image_event.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
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
            ledger_kind::NARRATION,
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
            ledger_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("older summary"),
            &json!({"through_entry_id":baseline.id}),
            None,
        )
        .unwrap();

        let player = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
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
            ledger_kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up Mira"),
            &json!({"entity_ids":["mira"]}),
            Some(&narration.id),
        )
        .unwrap();
        for kind in [
            ledger_kind::DICEROLL,
            ledger_kind::CONTENT_EDITED,
            ledger_kind::IMAGE_GENERATED,
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
            ledger_kind::CONTEXT_SUMMARY,
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
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&doomed_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
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
