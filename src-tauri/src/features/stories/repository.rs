use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use super::model::Story;
use super::settings::{normalize_reasoning_effort, NarratorToolSettings};
use crate::features::images;
use crate::shared::db::{seed_player_entity, with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

/// Placeholder shown until the model (or the user) supplies a real title —
/// mirrors `DEFAULT_STORY_TITLE` in `src/lib/types.ts`.
pub(super) const DEFAULT_STORY_TITLE: &str = "New story";

pub(super) fn list_stories(pool: &Pool) -> AppResult<Vec<Story>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, settings_json
         FROM stories ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Story {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            settings_json: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// `settings` carries mechanics choices made while the story was still a
/// frontend draft, so they are written in the same transaction as the story
/// itself rather than a follow-up save that could fail on its own.
pub(super) fn create_story_in_pool(
    pool: &Pool,
    title: Option<String>,
    settings: Option<serde_json::Value>,
) -> AppResult<Story> {
    let title = title.unwrap_or_default();
    let title = title.trim();
    let title = if title.is_empty() {
        DEFAULT_STORY_TITLE
    } else {
        title
    };
    let mut settings = settings.unwrap_or_else(|| json!({}));
    let object = settings
        .as_object_mut()
        .ok_or_else(|| AppError::Invalid("story settings must be an object".into()))?;
    object.remove("attributes_enabled");
    object.remove("dice_mode");
    let tools = match object.get("narrator_tools") {
        Some(value) => serde_json::from_value::<NarratorToolSettings>(value.clone())
            .map_err(|error| AppError::Invalid(format!("invalid narrator tools: {error}")))?,
        None => NarratorToolSettings::default(),
    };
    object.insert("narrator_tools".into(), json!(tools));
    if let Some(value) = object.get("reasoning_effort") {
        match value {
            serde_json::Value::Null => {
                object.remove("reasoning_effort");
            }
            serde_json::Value::String(effort) if effort.trim().is_empty() => {
                object.remove("reasoning_effort");
            }
            serde_json::Value::String(effort) => {
                let normalized = normalize_reasoning_effort(effort).ok_or_else(|| {
                    AppError::Invalid(format!("unsupported reasoning effort: {effort}"))
                })?;
                object.insert("reasoning_effort".into(), json!(normalized));
            }
            _ => {
                return Err(AppError::Invalid(
                    "reasoning effort must be a string".into(),
                ))
            }
        }
    }
    let settings_json = settings.to_string();
    let now = Utc::now().to_rfc3339();
    let story_id = Uuid::new_v4().to_string();
    with_transaction(pool, |tx| {
        tx.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![story_id, title, now, settings_json],
        )?;
        seed_player_entity(tx, &story_id)
    })?;

    Ok(Story {
        id: story_id,
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
        settings_json,
    })
}

/// Click-to-edit rename from the story header. Auto-titling never overwrites
/// a title that isn't the placeholder, so any rename permanently ends it.
pub(super) fn rename_story(pool: &Pool, story_id: &str, title: &str) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Invalid("title must not be empty".into()));
    }
    let conn = pool.get()?;
    let now = Utc::now().to_rfc3339();
    let updated = conn.execute(
        "UPDATE stories SET title = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![title, now, story_id],
    )?;
    if updated == 0 {
        return Err(AppError::NotFound(format!("story {story_id} not found")));
    }
    Ok(())
}

/// Collects image file paths before the cascade delete removes their rows
/// (SQLite FK cascade cleans up every table, but can't touch files on disk).
pub(super) fn delete_story(pool: &Pool, story_id: &str) -> AppResult<()> {
    let paths = with_transaction(pool, |tx| {
        let paths = images::image_paths_for_story(tx, story_id)?;

        let deleted = tx.execute("DELETE FROM stories WHERE id = ?1", [story_id])?;
        if deleted == 0 {
            return Err(AppError::NotFound(format!("story {story_id} not found")));
        }
        Ok(paths)
    })?;

    images::delete_assets(&paths);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::db::test_pool;

    #[test]
    fn story_creation_validates_draft_and_seeds_canonical_player() {
        let pool = test_pool();
        let tools = NarratorToolSettings {
            roll_check: false,
            ..NarratorToolSettings::default()
        };
        let created = create_story_in_pool(
            &pool,
            None,
            Some(json!({
                "narrator_tools": tools,
                "reasoning_effort": "HIGH",
                "author_note": "keep",
                "attributes_enabled": false,
                "dice_mode": "never"
            })),
        )
        .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&created.settings_json).unwrap();
        assert_eq!(settings["narrator_tools"], json!(tools));
        assert_eq!(settings["reasoning_effort"], "high");
        assert_eq!(settings["author_note"], "keep");
        assert!(settings.get("attributes_enabled").is_none());
        assert!(settings.get("dice_mode").is_none());
        let conn = pool.get().unwrap();
        let (name, kind): (String, String) = conn
            .query_row(
                "SELECT story_entity_state.name, entities.kind FROM story_entity_state
             JOIN entities ON entities.id = story_entity_state.entity_id
             WHERE story_entity_state.story_id = ?1",
                [&created.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((name.as_str(), kind.as_str()), ("You", "character"));
        drop(conn);
        let fresh = create_story_in_pool(&pool, None, None).unwrap();
        let defaults: serde_json::Value = serde_json::from_str(&fresh.settings_json).unwrap();
        assert_eq!(
            defaults["narrator_tools"],
            json!(NarratorToolSettings::default())
        );
        assert!(create_story_in_pool(
            &pool,
            None,
            Some(json!({"narrator_tools":{"roll_check":true}}))
        )
        .is_err());
        let count: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM stories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
