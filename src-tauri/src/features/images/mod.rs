pub mod model;
pub(crate) mod openrouter;

use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::ai;
use crate::features::settings;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};
use model::StoryImage;

fn contains_name_ci(haystack: &str, name: &str) -> bool {
    haystack.to_lowercase().contains(&name.to_lowercase())
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct GeneratedImageDescription {
    /// A vivid, concrete visual description of the scene for an
    /// image-generation model — subject, setting, composition, lighting.
    /// No mention of art style or medium; that's applied separately.
    description: String,
}

const IMAGE_PROMPT_PREAMBLE: &str = "You write vivid, concrete visual descriptions for an \
image-generation model, based on a moment from an interactive story. Describe what should be \
depicted — subject, setting, composition, lighting — in a few sentences. Do not mention art \
style or medium; that is applied separately. Respond with the structured output only.";

/// Finds which known characters are "in frame": named in the player's
/// guiding hint if any, else named in the passage's own text.
fn matched_characters<'a>(
    characters: &'a [(String, String)],
    passage_content: &str,
    hint: Option<&str>,
) -> Vec<&'a (String, String)> {
    let mut matched: Vec<&(String, String)> = characters
        .iter()
        .filter(|(name, _)| contains_name_ci(hint.unwrap_or(passage_content), name))
        .collect();
    if matched.is_empty() && hint.is_some() {
        // The hint didn't name anyone directly ("the girl you are seeing") —
        // fall back to whoever the passage itself establishes as present.
        matched = characters
            .iter()
            .filter(|(name, _)| contains_name_ci(passage_content, name))
            .collect();
    }
    matched
}

/// Asks the text model to turn the scene (and the player's optional hint)
/// into a vivid visual description, grounded with any matched characters'
/// appearance anchors so it doesn't have to invent — and drift on — what
/// they look like.
async fn write_image_description(
    config: &ai::TextModelConfig,
    passage_content: &str,
    hint: Option<&str>,
    matched: &[&(String, String)],
) -> AppResult<String> {
    let mut input = format!("Scene:\n{passage_content}");
    if let Some(hint) = hint {
        input.push_str(&format!("\n\nFocus the image on: {hint}"));
    }
    if !matched.is_empty() {
        input.push_str("\n\nCharacters present — keep their appearance consistent with this:");
        for (name, anchor) in matched {
            input.push_str(&format!("\n- {name}: {anchor}"));
        }
    }
    let generated: GeneratedImageDescription =
        ai::prompt_typed(config, IMAGE_PROMPT_PREAMBLE, input).await?;
    let description = generated.description.trim().to_string();
    if description.is_empty() {
        return Err(AppError::Other(
            "the prompt-writing model returned an empty description".into(),
        ));
    }
    Ok(description)
}

/// Composes the final image prompt: style prefix + the model-written scene
/// description, plus a literal character-appearance block so the image
/// model can't drift on a look even if the description paraphrases it away.
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

/// The "See" tool itself: turns a passage — plus an optional guiding `hint`
/// naming what to focus on — into a generated, stored scene image. Both the
/// player's manual trigger and the narrator's own decision to illustrate
/// (`maybe_auto_image`) come through here, so an auto-drawn image is composed
/// exactly like a player-drawn one.
pub(crate) async fn generate_for_passage(
    app: &AppHandle,
    pool: &Pool,
    passage_id: &str,
    hint: Option<&str>,
) -> AppResult<StoryImage> {
    let settings = settings::read_image_model_settings(app, pool)?;
    if !settings.enabled {
        return Err(AppError::Invalid(
            "image generation is disabled in the Image Model panel".into(),
        ));
    }
    let api_key = settings::read_api_key(app, "openrouter")?;

    let (passage_content, characters) = {
        let conn = pool.get()?;
        let passage_content: String = conn
            .query_row(
                "SELECT content FROM passages WHERE id = ?1",
                [passage_id],
                |row| row.get(0),
            )
            .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))?;
        let story_id: String = conn
            .query_row(
                "SELECT branches.story_id FROM passages JOIN branches ON branches.id = passages.branch_id WHERE passages.id = ?1",
                [passage_id],
                |row| row.get(0),
            )
            .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))?;
        let mut stmt = conn.prepare(
            "SELECT name, appearance_anchor FROM entities
             WHERE story_id = ?1 AND kind = 'character' AND appearance_anchor IS NOT NULL",
        )?;
        let characters: Vec<(String, String)> = stmt
            .query_map([&story_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        (passage_content, characters)
    };

    let hint = hint.map(str::trim).filter(|s| !s.is_empty());
    let matched = matched_characters(&characters, &passage_content, hint);

    let text_config = settings::resolve_text_model(app, pool)?;
    let description =
        write_image_description(&text_config, &passage_content, hint, &matched).await?;
    let prompt = compose_image_prompt(&settings.style, &description, &matched);

    let generated = openrouter::generate_image(&api_key, &settings.model, &prompt).await?;

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

    let persist_result = (|| -> AppResult<()> {
        let mut conn = pool.get()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current_content: String = tx
            .query_row(
                "SELECT content FROM passages WHERE id = ?1",
                [passage_id],
                |row| row.get(0),
            )
            .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))?;
        if current_content != passage_content {
            return Err(AppError::Other(
                "the passage changed while its image was being generated".into(),
            ));
        }
        tx.execute(
            "INSERT INTO images (id, passage_id, path, prompt, seed, provider, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
            rusqlite::params![id, passage_id, path_str, prompt, "openrouter", now],
        )?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = persist_result {
        let _ = std::fs::remove_file(&file_path);
        return Err(error);
    }

    Ok(StoryImage {
        id,
        passage_id: passage_id.to_string(),
        path: path_str,
        prompt,
        seed: None,
        provider: "openrouter".to_string(),
        created_at: now,
    })
}

/// Manual scene-image trigger ("See" composer mode). `prompt_hint` is the
/// player's optional guiding text (e.g. "the bucket", "the girl you are
/// seeing"); it and the passage's own text are both handed to the text model,
/// which writes the actual image prompt (see `write_image_description`)
/// rather than either being sent to the image model verbatim.
#[tauri::command]
pub async fn generate_scene_image(
    app: AppHandle,
    pool: State<'_, Pool>,
    passage_id: String,
    prompt_hint: Option<String>,
) -> AppResult<StoryImage> {
    generate_for_passage(&app, pool.inner(), &passage_id, prompt_hint.as_deref()).await
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ImageDecision {
    /// Whether this moment is striking enough to be worth illustrating.
    should_draw: bool,
    /// What the image should focus on — a short phrase like "the girl in the
    /// doorway" or "the burning watchtower". Empty when not drawing.
    focus: String,
}

const IMAGE_DECISION_PREAMBLE: &str = "You are the narrator of an interactive story, deciding \
whether the passage you just wrote is worth illustrating with a single scene image. Be selective: \
most beats do not need one. Reserve it for a genuinely striking visual moment — the reveal of a \
new place, a character appearing for the first time, or a dramatic turn worth seeing. If you do \
draw, name in a short phrase what the image should focus on. Respond with the structured output \
only.";

/// The narrator's own call on whether to illustrate the passage it just
/// wrote — the "See" tool, invoked by the narrator instead of the player.
/// Fire-and-forget and best-effort throughout (mirroring
/// `stories::maybe_auto_title`): it never blocks or fails the passage write,
/// and reports back via the `scene-image-generated` event. The focus phrase
/// it produces is handed to `generate_for_passage` as the guiding hint, so
/// the prompt is written the same way a player-triggered image's is.
pub(crate) fn maybe_auto_image(
    app: &AppHandle,
    pool: &Pool,
    passage_id: &str,
    narrated_text: &str,
) {
    let Ok(settings) = settings::read_image_model_settings(app, pool) else {
        return;
    };
    if !settings.enabled || !settings.narrator_images {
        return;
    }
    let Ok(config) = settings::resolve_text_model(app, pool) else {
        // No text model configured yet: nothing to decide with.
        return;
    };

    let app = app.clone();
    let pool = pool.clone();
    let passage_id = passage_id.to_string();
    let narrated_text = narrated_text.to_string();
    tauri::async_runtime::spawn(async move {
        let prompt =
            format!("You just wrote this passage:\n\n{narrated_text}\n\nShould it be illustrated?");
        let Ok(decision) =
            ai::prompt_typed::<ImageDecision>(&config, IMAGE_DECISION_PREAMBLE, prompt).await
        else {
            return;
        };
        if !decision.should_draw {
            return;
        }
        let focus = decision.focus.trim();
        let focus = if focus.is_empty() { None } else { Some(focus) };
        // Announce the decision before the slow part, so the passage can show a
        // placeholder for the minute-plus the image model takes.
        let _ = app.emit("scene-image-pending", &passage_id);
        match generate_for_passage(&app, &pool, &passage_id, focus).await {
            Ok(image) => {
                let _ = app.emit("scene-image-generated", image);
            }
            // Still best-effort — but the placeholder has to be cleared.
            Err(_) => {
                let _ = app.emit("scene-image-failed", &passage_id);
            }
        }
    });
}

fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<StoryImage> {
    Ok(StoryImage {
        id: row.get(0)?,
        passage_id: row.get(1)?,
        path: row.get(2)?,
        prompt: row.get(3)?,
        seed: row.get(4)?,
        provider: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[tauri::command]
pub fn list_images_for_passage(
    pool: State<Pool>,
    passage_id: String,
) -> AppResult<Vec<StoryImage>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, passage_id, path, prompt, seed, provider, created_at
         FROM images WHERE passage_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([passage_id], row_to_image)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// All images for every passage in a branch, in one call — the frontend
/// groups them by `passage_id` itself rather than issuing one query per
/// passage.
#[tauri::command]
pub fn list_images_for_branch(pool: State<Pool>, branch_id: String) -> AppResult<Vec<StoryImage>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT images.id, images.passage_id, images.path, images.prompt, images.seed, images.provider, images.created_at
         FROM images JOIN passages ON passages.id = images.passage_id
         WHERE passages.branch_id = ?1 ORDER BY images.created_at ASC",
    )?;
    let rows = stmt.query_map([branch_id], row_to_image)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
