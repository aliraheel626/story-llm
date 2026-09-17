pub mod model;
pub(crate) mod openrouter;

use std::sync::Arc;

use chrono::Utc;
use rig_agent::tool::DynamicTool;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::ai;
use crate::features::timeline::{
    model::{kind as timeline_kind, TimelineEntry},
    repository as timeline_repository,
};
use crate::features::{narration, settings};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};
use model::{ImageRequest, StoryImage};

const MANDATORY_IMAGE_PREAMBLE: &str = "The player explicitly asked to visualize this moment. \
Call the illustrate_scene tool with a vivid, concrete description — do not skip it.";

fn illustration_decision_prompt(
    passage_content: &str,
    hint: Option<&str>,
    known_characters: &[(String, String)],
) -> String {
    let mut prompt = format!("Scene:\n{passage_content}");
    if let Some(hint) = hint {
        prompt.push_str(&format!("\n\nFocus the image on: {hint}"));
    }
    if !known_characters.is_empty() {
        prompt.push_str("\n\nKnown characters (id: name) - list only the ids actually visible in this scene when calling illustrate_scene:");
        for (id, name) in known_characters {
            prompt.push_str(&format!("\n- {id}: {name}"));
        }
    }
    prompt
}

fn finish_illustration_decision(requests: Vec<ImageRequest>) -> AppResult<ImageRequest> {
    requests
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Other("the model did not produce an image description".into()))
}

struct IllustrationDecision<'a> {
    passage_content: &'a str,
    hint: Option<&'a str>,
    known_characters: &'a [(String, String)],
}

async fn decide_illustration(
    pool: &Pool,
    branch_id: &str,
    before_seq: Option<i64>,
    config: &ai::TextModelConfig,
    decision: IllustrationDecision<'_>,
) -> AppResult<ImageRequest> {
    let preamble = MANDATORY_IMAGE_PREAMBLE.to_string();
    let prompt = illustration_decision_prompt(
        decision.passage_content,
        decision.hint,
        decision.known_characters,
    );
    let history = narration::history::load_history(pool, branch_id, before_seq)?;
    let history = crate::features::timeline::compaction::prepare_history(
        pool, branch_id, config, &preamble, &prompt, history, before_seq,
    )
    .await;
    let image_requests = Arc::new(Mutex::new(Vec::new()));
    let tools = vec![DynamicTool::from_portable(
        narration::tools::illustrate_scene_tool(image_requests.clone()),
    )];
    ai::stream_narration(
        ai::NarrateRequest {
            config: config.clone(),
            preamble,
            history,
            prompt,
            stop_after_tool_result: true,
            reasoning_effort: None,
            tools,
        },
        |_| {},
    )
    .await?;
    let requests = std::mem::take(&mut *image_requests.lock().await);
    finish_illustration_decision(requests)
}

fn known_characters(
    conn: &rusqlite::Connection,
    story_id: &str,
    branch_id: &str,
) -> AppResult<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT entities.id, branch_entity_state.name
         FROM entities JOIN branch_entity_state ON branch_entity_state.entity_id = entities.id
         WHERE entities.story_id = ?1 AND branch_entity_state.branch_id = ?2
           AND entities.kind = 'character' AND branch_entity_state.is_present = 1",
    )?;
    let characters = stmt
        .query_map(rusqlite::params![story_id, branch_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(characters)
}

fn illustration_context(
    pool: &Pool,
    entry_id: &str,
) -> AppResult<(TimelineEntry, Vec<(String, String)>)> {
    let conn = pool.get()?;
    let active = timeline_repository::active_entry(&conn, entry_id)?;
    let story_id: String = conn
        .query_row(
            "SELECT branches.story_id FROM timeline_entries
             JOIN branches ON branches.id = timeline_entries.branch_id
             WHERE timeline_entries.id = ?1",
            [entry_id],
            |row| row.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("timeline entry {entry_id} not found")))?;
    let characters = known_characters(&conn, &story_id, &active.branch_id)?;
    Ok((active, characters))
}

/// Composes the final image prompt: style prefix + the scene description, plus
/// a literal character-appearance block so the image model can't drift on a
/// look even if the description paraphrases it away.
fn compose_image_prompt(style: &str, description: &str, matched: &[&(String, String)]) -> String {
    let mut prompt = format!("{style} {description}");
    if !matched.is_empty() {
        prompt.push_str("\n\nCharacter appearance reference — keep consistent with this, do not invent a different look:");
        for (name, anchor) in matched {
            prompt.push_str(&format!("\n- {name}: {anchor}"));
        }
    }
    prompt
}

/// Turns a finalized passage plus an optional guiding `hint` into a generated,
/// stored scene image for the player's manual "See" trigger. A mandatory,
/// single-tool agent turn writes the description with branch history and
/// known-character ids as context.
pub(crate) async fn generate_for_entry(
    app: &AppHandle,
    pool: &Pool,
    entry_id: &str,
    hint: Option<&str>,
) -> AppResult<StoryImage> {
    let image_settings = settings::read_image_model_settings(app, pool)?;
    if !image_settings.enabled {
        return Err(AppError::Invalid(
            "image generation is disabled in the Image Model panel".into(),
        ));
    }
    let (active, known) = illustration_context(pool, entry_id)?;
    let passage_content = active.content.unwrap_or_default();
    let hint = hint.map(str::trim).filter(|s| !s.is_empty());
    let text_config = settings::resolve_text_model(app, pool)?;
    let request = decide_illustration(
        pool,
        &active.branch_id,
        Some(active.seq),
        &text_config,
        IllustrationDecision {
            passage_content: &passage_content,
            hint,
            known_characters: &known,
        },
    )
    .await?;
    generate_from_description(
        app,
        pool,
        entry_id,
        &passage_content,
        &request.description,
        &request.character_ids,
    )
    .await
}

fn characters_by_ids(
    conn: &rusqlite::Connection,
    story_id: &str,
    branch_id: &str,
    ids: &[String],
) -> AppResult<Vec<(String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT branch_entity_state.name, branch_entity_state.appearance_anchor
         FROM entities JOIN branch_entity_state ON branch_entity_state.entity_id = entities.id
         WHERE entities.story_id = ? AND branch_entity_state.branch_id = ?
           AND entities.kind = 'character' AND branch_entity_state.is_present = 1
           AND branch_entity_state.appearance_anchor IS NOT NULL
           AND entities.id IN ({placeholders})"
    ))?;
    let params = std::iter::once(story_id)
        .chain(std::iter::once(branch_id))
        .chain(ids.iter().map(String::as_str));
    let characters = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(characters)
}

async fn generate_from_description(
    app: &AppHandle,
    pool: &Pool,
    entry_id: &str,
    expected_content: &str,
    description: &str,
    character_ids: &[String],
) -> AppResult<StoryImage> {
    let settings = settings::read_image_model_settings(app, pool)?;
    if !settings.enabled {
        return Err(AppError::Invalid(
            "image generation is disabled in the Image Model panel".into(),
        ));
    }
    let api_key = settings::read_api_key(app, "openrouter")?;

    let characters = {
        let conn = pool.get()?;
        let (story_id, branch_id): (String, String) = conn
            .query_row(
                "SELECT branches.story_id, timeline_entries.branch_id
                 FROM timeline_entries JOIN branches ON branches.id = timeline_entries.branch_id
                 WHERE timeline_entries.id = ?1",
                [entry_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| AppError::NotFound(format!("timeline entry {entry_id} not found")))?;
        characters_by_ids(&conn, &story_id, &branch_id, character_ids)?
    };
    let matched: Vec<&(String, String)> = characters.iter().collect();
    let prompt = compose_image_prompt(&settings.style, description, &matched);
    let generated = openrouter::generate_image(&api_key, &settings.model, &prompt).await?;
    persist_and_store_image(
        app,
        pool,
        entry_id,
        expected_content,
        description,
        prompt,
        generated,
    )
    .await
}

async fn persist_and_store_image(
    app: &AppHandle,
    pool: &Pool,
    entry_id: &str,
    expected_content: &str,
    description: &str,
    prompt: String,
    generated: openrouter::GeneratedImage,
) -> AppResult<StoryImage> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(e.to_string()))?;
    let images_dir = app_data_dir.join("images");
    std::fs::create_dir_all(&images_dir)?;
    let file_name = format!(
        "{}.{}",
        Uuid::new_v4(),
        openrouter::extension_for(&generated.media_type)
    );
    let file_path = images_dir.join(&file_name);
    std::fs::write(&file_path, &generated.bytes)?;

    let path_str = file_path.to_string_lossy().to_string();
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    let persist_result = with_transaction(pool, |tx| {
        let current_content = timeline_repository::active_entry(tx, entry_id)?
            .content
            .unwrap_or_default();
        if current_content != expected_content {
            return Err(AppError::Other(
                "the passage changed while its image was being generated".into(),
            ));
        }
        tx.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, seed, provider, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
            rusqlite::params![id, entry_id, path_str, prompt, "openrouter", now],
        )?;
        let base = timeline_repository::get_entry(tx, entry_id)?;
        timeline_repository::append_entry(
            tx,
            &base.branch_id,
            timeline_kind::IMAGE_GENERATED,
            "hidden",
            Some(&format!(
                "A scene image was generated depicting: {description}"
            )),
            &serde_json::json!({"asset_id": id, "prompt": prompt, "provider": "openrouter"}),
            Some(entry_id),
        )?;
        Ok(())
    });
    if let Err(error) = persist_result {
        let _ = std::fs::remove_file(&file_path);
        return Err(error);
    }

    Ok(StoryImage {
        id,
        entry_id: entry_id.to_string(),
        path: path_str,
        prompt,
        seed: None,
        provider: "openrouter".to_string(),
        created_at: now,
    })
}

/// Manual scene-image trigger ("See" composer mode). `prompt_hint`, the target
/// passage, prior branch history, and known-character ids are handed to a
/// mandatory `illustrate_scene` agent turn before image generation.
#[tauri::command]
pub async fn generate_scene_image(
    app: AppHandle,
    pool: State<'_, Pool>,
    entry_id: String,
    prompt_hint: Option<String>,
) -> AppResult<StoryImage> {
    generate_for_entry(&app, pool.inner(), &entry_id, prompt_hint.as_deref()).await
}

/// Starts the slow image work requested by `submit_turn`'s narrator after the
/// passage and its staged entity changes have committed. Each request keeps
/// the existing pending/generated/failed event contract used by the frontend.
pub(crate) fn generate_from_narrator_requests(
    app: &AppHandle,
    pool: &Pool,
    entry_id: &str,
    narrated_text: &str,
    requests: Vec<ImageRequest>,
) {
    let app = app.clone();
    let pool = pool.clone();
    let entry_id = entry_id.to_string();
    let narrated_text = narrated_text.to_string();
    tauri::async_runtime::spawn(async move {
        for request in requests {
            let _ = app.emit("scene-image-pending", &entry_id);
            match generate_from_description(
                &app,
                &pool,
                &entry_id,
                &narrated_text,
                &request.description,
                &request.character_ids,
            )
            .await
            {
                Ok(image) => {
                    let _ = app.emit("scene-image-generated", image);
                }
                Err(_) => {
                    let _ = app.emit("scene-image-failed", &entry_id);
                }
            }
        }
    });
}

fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<StoryImage> {
    Ok(StoryImage {
        id: row.get(0)?,
        entry_id: row.get(1)?,
        path: row.get(2)?,
        prompt: row.get(3)?,
        seed: row.get(4)?,
        provider: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[tauri::command]
pub fn list_images_for_entry(pool: State<Pool>, entry_id: String) -> AppResult<Vec<StoryImage>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, entry_id, path, prompt, seed, provider, created_at
         FROM image_assets WHERE entry_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([entry_id], row_to_image)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// All images for every passage in a branch, in one call — the frontend
/// groups them by `entry_id` itself rather than issuing one query per entry.
#[tauri::command]
pub fn list_images_for_branch(pool: State<Pool>, branch_id: String) -> AppResult<Vec<StoryImage>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT image_assets.id, image_assets.entry_id, image_assets.path, image_assets.prompt, image_assets.seed, image_assets.provider, image_assets.created_at
         FROM image_assets JOIN timeline_entries ON timeline_entries.id = image_assets.entry_id
         WHERE timeline_entries.branch_id = ?1 ORDER BY image_assets.created_at ASC",
    )?;
    let rows = stmt.query_map([branch_id], row_to_image)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ImageRequest {
        ImageRequest {
            description: "Mira stands beneath a lightning-split sky.".into(),
            character_ids: vec!["mira".into()],
        }
    }

    #[test]
    fn decision_returns_the_first_queued_request() {
        let selected = finish_illustration_decision(vec![request()]).unwrap();
        assert_eq!(
            selected.description,
            "Mira stands beneath a lightning-split sky."
        );
        assert_eq!(selected.character_ids, ["mira"]);
    }

    #[test]
    fn mandatory_decision_without_a_tool_call_returns_an_error() {
        let error = finish_illustration_decision(Vec::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the model did not produce an image description"
        );
    }

    #[test]
    fn decision_prompt_includes_hint_and_known_character_ids() {
        let prompt = illustration_decision_prompt(
            "The observatory doors open.",
            Some("the brass orrery"),
            &[("mira-id".into(), "Mira".into())],
        );
        assert_eq!(
            prompt,
            "Scene:\nThe observatory doors open.\n\nFocus the image on: the brass orrery\n\nKnown characters (id: name) - list only the ids actually visible in this scene when calling illustrate_scene:\n- mira-id: Mira"
        );
    }
}
