use chrono::Utc;
use rusqlite::OptionalExtension;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::features::ledger::{model::kind as ledger_kind, repository as ledger_repository};
use crate::features::settings;
use crate::shared::db::{blocking, with_transaction, Pool};
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
    let pool = pool.clone();
    let target = target.clone();
    let description = description.to_string();
    blocking(move || persist_and_store_image(&pool, &target, &description, prompt, generated)).await
}

fn persist_and_store_image(
    pool: &Pool,
    target: &ImageTarget,
    description: &str,
    prompt: String,
    generated: openrouter::GeneratedImage,
) -> AppResult<StoryImage> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    let image = StoryImage {
        id,
        entry_id: target.entry_id.clone(),
        prompt,
        created_at: now,
    };
    persist_image_record(pool, target, description, &image, &generated)?;
    Ok(image)
}

fn persist_image_record(
    pool: &Pool,
    target: &ImageTarget,
    description: &str,
    image: &StoryImage,
    generated: &openrouter::GeneratedImage,
) -> AppResult<()> {
    with_transaction(pool, |tx| {
        let turn_exists = tx
            .query_row(
                "SELECT 1 FROM turns WHERE id = ?1",
                [&target.turn_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !turn_exists {
            return Err(AppError::Invalid(
                "the turn was regenerated while its image was being generated".into(),
            ));
        }
        let current_content = ledger_repository::active_entry(tx, &target.entry_id)?
            .content
            .unwrap_or_default();
        if current_content != target.expected_content {
            return Err(AppError::Other(
                "the passage changed while its image was being generated".into(),
            ));
        }
        repository::insert_asset(tx, image, &generated.media_type, &generated.bytes)?;
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
    })
}

fn turn_exists(pool: &Pool, turn_id: &str) -> AppResult<bool> {
    Ok(pool.get()?.query_row(
        "SELECT EXISTS(SELECT 1 FROM turns WHERE id = ?1)",
        [turn_id],
        |row| row.get(0),
    )?)
}

/// Starts the slow image work requested by `submit_turn`'s narrator after the
/// passage and its entity changes have committed. Each request keeps
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
                    match turn_exists(&pool, &target.turn_id) {
                        Ok(false) => continue,
                        Err(check_error) => log::warn!(
                            "could not check failed image turn {}: {check_error}",
                            target.turn_id
                        ),
                        Ok(true) => {}
                    }
                    let _ = app.emit("scene-image-failed", &target.entry_id);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::{repository as ledger_repository, turns};

    fn turn_fixture() -> (Pool, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        drop(conn);
        (pool, turn_id)
    }

    #[test]
    fn removed_turn_writes_no_asset_or_event() {
        let (pool, prior_turn_id) = turn_fixture();
        let conn = pool.get().unwrap();
        let entry = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "A moonlit harbor",
            None,
            Some(&prior_turn_id),
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let stale_target = ImageTarget {
            entry_id: entry.id.clone(),
            expected_content: "A moonlit harbor".into(),
            source_action_id: None,
            turn_id: turn_id.clone(),
        };
        conn.execute("DELETE FROM turns WHERE id = ?1", [&turn_id])
            .unwrap();
        drop(conn);
        assert!(!turn_exists(&pool, &turn_id).unwrap());
        let image = StoryImage {
            id: "stale-image".into(),
            entry_id: entry.id,
            prompt: "moonlit harbor".into(),
            created_at: "now".into(),
        };

        let generated = openrouter::GeneratedImage {
            bytes: vec![1, 2, 3],
            media_type: "image/png".into(),
        };
        let error =
            persist_image_record(&pool, &stale_target, "moonlit harbor", &image, &generated)
                .unwrap_err();

        assert!(matches!(
            error,
            AppError::Invalid(message)
                if message == "the turn was regenerated while its image was being generated"
        ));
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::IMAGE_GENERATED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn persisted_image_has_blob_and_empty_legacy_path() {
        let (pool, turn_id) = turn_fixture();
        let conn = pool.get().unwrap();
        let entry = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "Scene",
            None,
            Some(&turn_id),
        )
        .unwrap();
        drop(conn);
        let image = StoryImage {
            id: "new-image".into(),
            entry_id: entry.id.clone(),
            prompt: "the scene".into(),
            created_at: "now".into(),
        };
        let generated = openrouter::GeneratedImage {
            bytes: vec![1, 2, 3],
            media_type: "image/webp".into(),
        };
        persist_image_record(
            &pool,
            &ImageTarget {
                entry_id: entry.id,
                expected_content: "Scene".into(),
                source_action_id: None,
                turn_id: turn_id.clone(),
            },
            "the scene",
            &image,
            &generated,
        )
        .unwrap();
        let conn = pool.get().unwrap();
        let (path, media_type, bytes): (String, String, Vec<u8>) = conn
            .query_row(
                "SELECT image_assets.path, image_blobs.media_type, image_blobs.bytes
             FROM image_assets JOIN image_blobs ON image_blobs.asset_id = image_assets.id
             WHERE image_assets.id = 'new-image'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(path, "");
        assert_eq!(media_type, "image/webp");
        assert_eq!(bytes, generated.bytes);
        let (event_turn_id, asset_id): (String, String) = conn
            .query_row(
                "SELECT turn_id, json_extract(payload_json, '$.asset_id')
             FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::IMAGE_GENERATED],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_turn_id, turn_id);
        assert_eq!(asset_id, image.id);
    }

    #[test]
    fn event_failure_rolls_back_asset_and_blob_without_failing_turn() {
        let (pool, turn_id) = turn_fixture();
        let conn = pool.get().unwrap();
        let entry = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "Scene",
            None,
            Some(&turn_id),
        )
        .unwrap();
        drop(conn);
        let image = StoryImage {
            id: "rolled-back-image".into(),
            entry_id: entry.id.clone(),
            prompt: "the scene".into(),
            created_at: "now".into(),
        };
        let target = ImageTarget {
            entry_id: entry.id,
            expected_content: "Scene".into(),
            source_action_id: Some("missing-action".into()),
            turn_id,
        };
        let generated = openrouter::GeneratedImage {
            bytes: vec![1, 2, 3],
            media_type: "image/png".into(),
        };

        assert!(persist_image_record(&pool, &target, "the scene", &image, &generated).is_err());
        let conn = pool.get().unwrap();
        for table in ["image_assets", "image_blobs"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} must roll back with the image event");
        }
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::IMAGE_GENERATED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            turns::last_turn(&conn, "s").unwrap().unwrap().status,
            turns::COMPLETE
        );
    }
}
