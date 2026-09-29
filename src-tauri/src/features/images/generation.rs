use std::time::Instant;

use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use crate::features::transcript::{
    attachments, model::kind as transcript_kind, repository as transcript_repository,
};
use crate::features::settings;
use crate::features::usage::model::UsageRecord;
use crate::features::usage::repository::{record_late, LATE_REPLY_LIMIT};
use crate::features::turn::TurnTx;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::model::{ImageRequest, StoryImage};
use super::openrouter;

const IMAGE_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) struct ImageTarget {
    pub entry_id: String,
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
    settings_pool: &Pool,
    turn: &TurnTx,
    target: &ImageTarget,
    description: &str,
    character_ids: &[String],
) -> AppResult<StoryImage> {
    let settings = settings::read_image_model_settings(app, settings_pool)?;
    if !settings.enabled {
        return Err(AppError::Invalid(
            "image generation is disabled in the Image Model panel".into(),
        ));
    }
    let api_key = settings::read_api_key(app, "openrouter")?;

    let (story_id, characters) = turn
        .with(|conn| {
            let story_id = transcript_repository::get_entry(conn, &target.entry_id)?.story_id;
            let characters = characters_by_ids(conn, &story_id, character_ids)?;
            Ok((story_id, characters))
        })
        .await?;
    let matched: Vec<&(String, String)> = characters.iter().collect();
    let prompt = compose_image_prompt(&settings.style, description, &matched);
    let asset_id = Uuid::new_v4().to_string();
    let started = Instant::now();
    let mut request = tauri::async_runtime::spawn({
        let (key, model, prompt) = (api_key.clone(), settings.model.clone(), prompt.clone());
        async move { openrouter::generate_image(&key, &model, &prompt).await }
    });
    let generated = match timeout(IMAGE_TIMEOUT, &mut request).await {
        Ok(joined) => joined.map_err(|e| AppError::Other(format!("image task failed: {e}")))??,
        Err(_) => {
            let pool = settings_pool.clone();
            let model = settings.model.clone();
            let turn_id = target.turn_id.clone();
            tauri::async_runtime::spawn(async move {
                match timeout(LATE_REPLY_LIMIT, &mut request).await {
                    Ok(Ok(Ok(generated))) => {
                        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                        let mut record = UsageRecord::image(&model, generated.cost_usd, None, Some(duration_ms));
                        record.turn_id = Some(turn_id);
                        record_late(
                            pool,
                            story_id,
                            record,
                        )
                        .await;
                    }
                    Err(_) => {
                        let mut record = UsageRecord::image(&model, None, None, None);
                        record.turn_id = Some(turn_id);
                        record_late(pool, story_id, record).await;
                    }
                    Ok(Ok(Err(_)) | Err(_)) => {}
                }
            });
            return Err(AppError::Other("image generation timed out".into()));
        }
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    turn.record_usage(UsageRecord::image(&settings.model, generated.cost_usd, Some(asset_id.clone()), Some(duration_ms)));
    turn.with_savepoint(|conn| {
        persist_and_store_image(conn, &asset_id, target, description, prompt, generated)
    })
    .await
}

fn persist_and_store_image(
    conn: &rusqlite::Connection,
    asset_id: &str,
    target: &ImageTarget,
    description: &str,
    prompt: String,
    generated: openrouter::GeneratedImage,
) -> AppResult<StoryImage> {
    let now = Utc::now().to_rfc3339();

    let image = StoryImage {
        id: asset_id.to_string(),
        entry_id: target.entry_id.clone(),
        prompt,
        created_at: now,
    };
    persist_image_record(conn, target, description, &image, &generated)?;
    Ok(image)
}

fn persist_image_record(
    conn: &rusqlite::Connection,
    target: &ImageTarget,
    description: &str,
    image: &StoryImage,
    generated: &openrouter::GeneratedImage,
) -> AppResult<()> {
    attachments::insert_image(conn, image, &generated.media_type, &generated.bytes)?;
    let base = transcript_repository::get_entry(conn, &target.entry_id)?;
    transcript_repository::append_entry(
        conn,
        &base.story_id,
        transcript_kind::IMAGE_GENERATED,
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
}

pub(crate) async fn generate_in_turn(
    app: &AppHandle,
    settings_pool: &Pool,
    turn: &TurnTx,
    target: &ImageTarget,
    requests: Vec<ImageRequest>,
) -> Vec<Result<StoryImage, ()>> {
    if !requests.is_empty() {
        let _ = app.emit("scene-image-pending", &target.entry_id);
    }
    let mut results = Vec::with_capacity(requests.len());
    for request in requests {
        let result = generate_from_description(
            app,
            settings_pool,
            turn,
            target,
            &request.description,
            &request.character_ids,
        )
        .await
        .map_err(|error| {
            log::error!(
                "scene image generation failed for entry {}: {error}",
                target.entry_id
            );
        });
        results.push(result);
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::transcript::{repository as transcript_repository, turns};
    use crate::features::turn::TurnGate;

    async fn turn_fixture() -> (Pool, std::sync::Arc<TurnTx>, String, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        drop(conn);
        let turn = TurnTx::begin(&pool, &TurnGate::default(), "s").unwrap();
        let (turn_id, entry_id) = turn
            .with(|conn| {
                let turn_id = turns::create_turn(conn, "s")?;
                let entry = transcript_repository::append_story_message(
                    conn,
                    "s",
                    "narrator",
                    "generated",
                    "Scene",
                    None,
                    Some(&turn_id),
                )?;
                Ok((turn_id, entry.id))
            })
            .await
            .unwrap();
        (pool, turn, turn_id, entry_id)
    }

    #[tokio::test]
    async fn persisted_image_is_private_until_turn_commit() {
        let (pool, turn, turn_id, entry_id) = turn_fixture().await;
        let generated = openrouter::GeneratedImage {
            bytes: vec![1, 2, 3],
            media_type: "image/webp".into(),
            cost_usd: None,
        };
        let expected_bytes = generated.bytes.clone();
        let image = turn
            .with_savepoint(|conn| {
                persist_and_store_image(
                    conn,
                    "test-image",
                    &ImageTarget {
                        entry_id,
                        source_action_id: None,
                        turn_id: turn_id.clone(),
                    },
                    "the scene",
                    "the scene".into(),
                    generated,
                )
            })
            .await
            .unwrap();
        assert_eq!(image.id, "test-image");
        assert_eq!(
            turn.with(|conn| Ok(conn.query_row(
                "SELECT COUNT(*) FROM image_assets",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        turn.commit().await.unwrap();
        let conn = pool.get().unwrap();
        let (path, media_type, bytes): (String, String, Vec<u8>) = conn
            .query_row(
                "SELECT image_assets.path, image_blobs.media_type, image_blobs.bytes
             FROM image_assets JOIN image_blobs ON image_blobs.asset_id = image_assets.id
              WHERE image_assets.id = ?1",
                [&image.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(path, "");
        assert_eq!(media_type, "image/webp");
        assert_eq!(bytes, expected_bytes);
        let (event_turn_id, asset_id): (String, String) = conn
            .query_row(
                "SELECT turn_id, json_extract(payload_json, '$.asset_id')
             FROM transcript_entries WHERE kind = ?1",
                [transcript_kind::IMAGE_GENERATED],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_turn_id, turn_id);
        assert_eq!(asset_id, image.id);
    }

    #[tokio::test]
    async fn event_failure_rolls_back_image_savepoint_but_preserves_turn() {
        let (pool, turn, turn_id, entry_id) = turn_fixture().await;
        let image = StoryImage {
            id: "rolled-back-image".into(),
            entry_id: entry_id.clone(),
            prompt: "the scene".into(),
            created_at: "now".into(),
        };
        let target = ImageTarget {
            entry_id,
            source_action_id: Some("missing-action".into()),
            turn_id,
        };
        let generated = openrouter::GeneratedImage {
            bytes: vec![1, 2, 3],
            media_type: "image/png".into(),
            cost_usd: None,
        };

        assert!(turn
            .with_savepoint(|conn| persist_image_record(
                conn,
                &target,
                "the scene",
                &image,
                &generated
            ))
            .await
            .is_err());
        turn.with(|conn| {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM image_blobs", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            transcript_repository::append_entry(
                conn,
                "s",
                transcript_kind::CONTENT_EDITED,
                "hidden",
                Some("turn remains writable"),
                &serde_json::json!({}),
                None,
                Some(&target.turn_id),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        turn.commit().await.unwrap();
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
                "SELECT COUNT(*) FROM transcript_entries WHERE kind = ?1",
                [transcript_kind::IMAGE_GENERATED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            turns::last_turn(&conn, "s").unwrap().unwrap().status,
            turns::COMPLETE
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM transcript_entries WHERE id = ?1",
                [&target.entry_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }
}
