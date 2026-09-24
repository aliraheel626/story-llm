use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::features::ledger::{model::kind as ledger_kind, repository as ledger_repository};
use crate::features::settings;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::model::{ImageRequest, StoryImage};
use super::{openrouter, repository};

#[derive(Clone)]
pub(crate) struct ImageTarget {
    pub entry_id: String,
    pub expected_content: String,
    pub source_action_id: Option<String>,
    pub turn_id: String,
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
    target: &ImageTarget,
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
        let story_id: String = conn
            .query_row(
                "SELECT story_id FROM ledger_entries WHERE id = ?1",
                [&target.entry_id],
                |row| row.get(0),
            )
            .map_err(|_| {
                AppError::NotFound(format!("ledger entry {} not found", target.entry_id))
            })?;
        characters_by_ids(&conn, &story_id, character_ids)?
    };
    let matched: Vec<&(String, String)> = characters.iter().collect();
    let prompt = compose_image_prompt(&settings.style, description, &matched);
    let generated = openrouter::generate_image(&api_key, &settings.model, &prompt).await?;
    persist_and_store_image(app, pool, target, description, prompt, generated).await
}

async fn persist_and_store_image(
    app: &AppHandle,
    pool: &Pool,
    target: &ImageTarget,
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

    let image = StoryImage {
        id,
        entry_id: target.entry_id.clone(),
        path: path_str,
        prompt,
        created_at: now,
    };
    let persist_result = with_transaction(pool, |tx| {
        let current_content = ledger_repository::active_entry(tx, &target.entry_id)?
            .content
            .unwrap_or_default();
        if current_content != target.expected_content {
            return Err(AppError::Other(
                "the passage changed while its image was being generated".into(),
            ));
        }
        repository::insert_asset(tx, &image)?;
        let base = ledger_repository::get_entry(tx, &target.entry_id)?;
        ledger_repository::append_entry(
            tx,
            &base.story_id,
            ledger_kind::IMAGE_GENERATED,
            "hidden",
            Some(&format!(
                "A scene image was generated depicting: {description}"
            )),
            &serde_json::json!({"asset_id": image.id, "prompt": image.prompt}),
            Some(
                target
                    .source_action_id
                    .as_deref()
                    .unwrap_or(&target.entry_id),
            ),
            Some(&target.turn_id),
        )?;
        Ok(())
    });
    if let Err(error) = persist_result {
        let _ = std::fs::remove_file(&file_path);
        return Err(error);
    }

    Ok(image)
}

/// Starts the slow image work requested by `submit_turn`'s narrator after the
/// passage and its staged entity changes have committed. Each request keeps
/// the existing pending/generated/failed event contract used by the frontend.
pub(crate) fn generate_from_narrator_requests(
    app: &AppHandle,
    pool: &Pool,
    target: ImageTarget,
    requests: Vec<ImageRequest>,
) {
    let app = app.clone();
    let pool = pool.clone();
    tauri::async_runtime::spawn(async move {
        for request in requests {
            let _ = app.emit("scene-image-pending", &target.entry_id);
            match generate_from_description(
                &app,
                &pool,
                &target,
                &request.description,
                &request.character_ids,
            )
            .await
            {
                Ok(image) => {
                    let _ = app.emit("scene-image-generated", image);
                }
                Err(error) => {
                    log::error!(
                        "scene image generation failed for entry {}: {error}",
                        target.entry_id
                    );
                    if target.source_action_id.is_some() {
                        if let Ok(conn) = pool.get() {
                            if let Err(status_error) = crate::features::ledger::turns::set_status(
                                &conn,
                                &target.turn_id,
                                crate::features::ledger::turns::FAILED,
                            ) {
                                log::error!("failed to mark image turn failed: {status_error}");
                            }
                        }
                    }
                    let _ = app.emit("scene-image-failed", &target.entry_id);
                }
            }
        }
    });
}
