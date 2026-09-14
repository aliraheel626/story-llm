use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::commands::settings;
use crate::db::Pool;
use crate::error::{AppError, AppResult};
use crate::images;
use crate::models::StoryImage;
use crate::narrator;

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
    let mut matched: Vec<&(String, String)> =
        characters.iter().filter(|(name, _)| contains_name_ci(hint.unwrap_or(passage_content), name)).collect();
    if matched.is_empty() && hint.is_some() {
        // The hint didn't name anyone directly ("the girl you are seeing") —
        // fall back to whoever the passage itself establishes as present.
        matched = characters.iter().filter(|(name, _)| contains_name_ci(passage_content, name)).collect();
    }
    matched
}

/// Asks the text model to turn the scene (and the player's optional hint)
/// into a vivid visual description, grounded with any matched characters'
/// appearance anchors so it doesn't have to invent — and drift on — what
/// they look like.
async fn write_image_description(
    config: &narrator::TextModelConfig,
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
    let generated: GeneratedImageDescription = narrator::prompt_typed(config, IMAGE_PROMPT_PREAMBLE, input).await?;
    let description = generated.description.trim().to_string();
    if description.is_empty() {
        return Err(AppError::Other("the prompt-writing model returned an empty description".into()));
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

/// Manual scene-image trigger ("See" composer mode, and the per-passage
/// button). `prompt_hint` is the player's optional guiding text (e.g. "the
/// bucket", "the girl you are seeing"); it and the passage's own text are
/// both handed to the text model, which writes the actual image prompt (see
/// `write_image_description`) rather than either being sent to the image
/// model verbatim.
#[tauri::command]
pub async fn generate_scene_image(
    app: AppHandle,
    pool: State<'_, Pool>,
    passage_id: String,
    prompt_hint: Option<String>,
) -> AppResult<StoryImage> {
    let settings = settings::get_image_model_settings(app.clone(), pool.clone())?;
    if !settings.enabled {
        return Err(AppError::Invalid("image generation is disabled in the Image Model panel".into()));
    }
    let api_key = settings::read_api_key(&app, "openrouter")?;

    let (passage_content, characters) = {
        let conn = pool.get()?;
        let passage_content: String = conn
            .query_row("SELECT content FROM passages WHERE id = ?1", [&passage_id], |row| row.get(0))
            .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))?;
        let story_id: String = conn
            .query_row(
                "SELECT branches.story_id FROM passages JOIN branches ON branches.id = passages.branch_id WHERE passages.id = ?1",
                [&passage_id],
                |row| row.get(0),
            )
            .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))?;
        let mut stmt = conn.prepare(
            "SELECT name, appearance_anchor FROM entities
             WHERE story_id = ?1 AND kind = 'character' AND appearance_anchor IS NOT NULL",
        )?;
        let characters: Vec<(String, String)> = stmt
            .query_map([&story_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<Result<_, _>>()?;
        (passage_content, characters)
    };

    let hint = prompt_hint.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let matched = matched_characters(&characters, &passage_content, hint);

    let text_config = crate::commands::passages::resolve_text_model(&app, pool.inner())?;
    let description = write_image_description(&text_config, &passage_content, hint, &matched).await?;
    let prompt = compose_image_prompt(&settings.style, &description, &matched);

    let generated = images::generate_image(&api_key, &settings.model, &prompt).await?;

    let app_data_dir = app.path().app_data_dir().map_err(|e| AppError::Other(e.to_string()))?;
    let images_dir = app_data_dir.join("images");
    std::fs::create_dir_all(&images_dir)?;
    let file_name = format!("{}.{}", Uuid::new_v4(), images::extension_for(&generated.media_type));
    let file_path = images_dir.join(&file_name);
    std::fs::write(&file_path, &generated.bytes)?;

    let path_str = file_path.to_string_lossy().to_string();
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO images (id, passage_id, path, prompt, seed, provider, created_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
        rusqlite::params![id, passage_id, path_str, prompt, "openrouter", now],
    )?;

    Ok(StoryImage {
        id,
        passage_id,
        path: path_str,
        prompt,
        seed: None,
        provider: "openrouter".to_string(),
        created_at: now,
    })
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
pub fn list_images_for_passage(pool: State<Pool>, passage_id: String) -> AppResult<Vec<StoryImage>> {
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
