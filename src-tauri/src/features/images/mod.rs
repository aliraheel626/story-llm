pub mod model;
pub(crate) mod openrouter;

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::features::ledger::{model::kind as ledger_kind, repository as ledger_repository};
use crate::features::settings;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};
use model::{ImageRequest, StoryImage};

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

fn characters_by_ids(
    conn: &rusqlite::Connection,
    story_id: &str,
    ids: &[String],
) -> AppResult<Vec<(String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT story_entity_state.name, story_entity_state.appearance_anchor
         FROM entities JOIN story_entity_state ON story_entity_state.entity_id = entities.id
         WHERE entities.story_id = ? AND story_entity_state.story_id = ?
           AND entities.kind = 'character' AND story_entity_state.is_present = 1
           AND story_entity_state.appearance_anchor IS NOT NULL
           AND entities.id IN ({placeholders})"
    ))?;
    let params = std::iter::once(story_id)
        .chain(std::iter::once(story_id))
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
    source_action_id: Option<&str>,
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
        let story_id: String = conn
            .query_row(
                "SELECT story_id FROM ledger_entries WHERE id = ?1",
                [entry_id],
                |row| row.get(0),
            )
            .map_err(|_| AppError::NotFound(format!("ledger entry {entry_id} not found")))?;
        characters_by_ids(&conn, &story_id, character_ids)?
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
        source_action_id,
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
    source_action_id: Option<&str>,
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
        let current_content = ledger_repository::active_entry(tx, entry_id)?
            .content
            .unwrap_or_default();
        if current_content != expected_content {
            return Err(AppError::Other(
                "the passage changed while its image was being generated".into(),
            ));
        }
        tx.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, entry_id, path_str, prompt, now],
        )?;
        let base = ledger_repository::get_entry(tx, entry_id)?;
        ledger_repository::append_entry(
            tx,
            &base.story_id,
            ledger_kind::IMAGE_GENERATED,
            "hidden",
            Some(&format!(
                "A scene image was generated depicting: {description}"
            )),
            &serde_json::json!({"asset_id": id, "prompt": prompt}),
            Some(source_action_id.unwrap_or(entry_id)),
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
        created_at: now,
    })
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
    source_action_id: Option<String>,
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
                source_action_id.as_deref(),
            )
            .await
            {
                Ok(image) => {
                    let _ = app.emit("scene-image-generated", image);
                }
                Err(error) => {
                    log::error!("scene image generation failed for entry {entry_id}: {error}");
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
        created_at: row.get(4)?,
    })
}

/// All images for every passage in a story, in one call — the frontend
/// groups them by `entry_id` itself rather than issuing one query per entry.
#[tauri::command]
pub fn list_images_for_story(pool: State<Pool>, story_id: String) -> AppResult<Vec<StoryImage>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT image_assets.id, image_assets.entry_id, image_assets.path, image_assets.prompt, image_assets.created_at
         FROM image_assets JOIN ledger_entries ON ledger_entries.id = image_assets.entry_id
         WHERE ledger_entries.story_id = ?1 ORDER BY image_assets.created_at ASC",
    )?;
    let rows = stmt.query_map([story_id], row_to_image)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
