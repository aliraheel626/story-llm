use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarratorToolSettings {
    pub get_entities: bool,
    pub create_entity: bool,
    pub update_entity: bool,
    pub adjust_entity_attribute: bool,
    pub roll_check: bool,
    pub illustrate_scene: bool,
}

impl Default for NarratorToolSettings {
    fn default() -> Self {
        Self {
            get_entities: true,
            create_entity: true,
            update_entity: true,
            adjust_entity_attribute: true,
            roll_check: true,
            illustrate_scene: true,
        }
    }
}

fn story_settings(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Value> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT settings_json FROM stories WHERE id = ?1",
            [story_id],
            |row| row.get(0),
        )
        .optional()?;
    let raw = raw.ok_or_else(|| AppError::NotFound(format!("story {story_id} not found")))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| AppError::Other(format!("invalid story settings: {error}")))?;
    if !value.is_object() {
        return Err(AppError::Other("story settings must be an object".into()));
    }
    Ok(value)
}

pub fn read_story_narrator_tools(pool: &Pool, story_id: &str) -> AppResult<NarratorToolSettings> {
    let conn = pool.get()?;
    let settings = story_settings(&conn, story_id)?;
    match settings.get("narrator_tools") {
        Some(tools) => serde_json::from_value(tools.clone())
            .map_err(|error| AppError::Other(format!("invalid narrator tools: {error}"))),
        None => Ok(NarratorToolSettings::default()),
    }
}

#[tauri::command]
pub fn get_story_narrator_tools(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<NarratorToolSettings> {
    read_story_narrator_tools(pool.inner(), &story_id)
}

#[tauri::command]
pub fn save_story_narrator_tools(
    pool: State<Pool>,
    story_id: String,
    tools: NarratorToolSettings,
) -> AppResult<()> {
    save_narrator_tools(pool.inner(), &story_id, tools)
}

fn save_narrator_tools(pool: &Pool, story_id: &str, tools: NarratorToolSettings) -> AppResult<()> {
    with_transaction(pool, |tx| {
        let mut settings = story_settings(tx, story_id)?;
        settings["narrator_tools"] = json!(tools);
        tx.execute(
            "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
        )?;
        Ok(())
    })
}

pub(crate) fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        _ => None,
    }
}

pub fn read_story_reasoning_effort(pool: &Pool, story_id: &str) -> AppResult<String> {
    let conn = pool.get()?;
    let settings = story_settings(&conn, story_id)?;
    Ok(settings
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
        .unwrap_or_default()
        .to_string())
}

#[tauri::command]
pub fn get_story_reasoning_effort(pool: State<Pool>, story_id: String) -> AppResult<String> {
    read_story_reasoning_effort(pool.inner(), &story_id)
}

#[tauri::command]
pub fn save_story_reasoning_effort(
    pool: State<Pool>,
    story_id: String,
    reasoning_effort: String,
) -> AppResult<()> {
    save_reasoning_effort(pool.inner(), &story_id, &reasoning_effort)
}

fn save_reasoning_effort(pool: &Pool, story_id: &str, reasoning_effort: &str) -> AppResult<()> {
    let effort = if reasoning_effort.trim().is_empty() {
        None
    } else {
        Some(normalize_reasoning_effort(reasoning_effort).ok_or_else(|| {
            AppError::Invalid(format!("unsupported reasoning effort: {reasoning_effort}"))
        })?)
    };
    with_transaction(pool, |tx| {
        let mut settings = story_settings(tx, story_id)?;
        match effort {
            Some(effort) => settings["reasoning_effort"] = json!(effort),
            None => {
                settings.as_object_mut().unwrap().remove("reasoning_effort");
            }
        }
        tx.execute(
            "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_legacy_reasoning_effort_uses_model_default() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES ('legacy','Legacy','now','now','{\"reasoning_effort\":\"obsolete\"}')", []).unwrap();
        drop(conn);
        assert_eq!(read_story_reasoning_effort(&pool, "legacy").unwrap(), "");
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE stories SET settings_json = '{\"reasoning_effort\":42}' WHERE id = 'legacy'",
            [],
        )
        .unwrap();
        drop(conn);
        assert_eq!(read_story_reasoning_effort(&pool, "legacy").unwrap(), "");
    }

    #[test]
    fn story_defaults_and_isolated_saves_preserve_unrelated_fields() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES
             ('first','First','now','now','{\"author_note\":\"hello\",\"custom\":42}'),
             ('second','Second','now','now','{}');",
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            read_story_narrator_tools(&pool, "first").unwrap(),
            NarratorToolSettings::default()
        );
        assert_eq!(read_story_reasoning_effort(&pool, "first").unwrap(), "");
        let tools = NarratorToolSettings {
            roll_check: false,
            ..NarratorToolSettings::default()
        };
        save_narrator_tools(&pool, "first", tools).unwrap();
        save_reasoning_effort(&pool, "first", "HIGH").unwrap();
        assert_eq!(read_story_narrator_tools(&pool, "first").unwrap(), tools);
        assert_eq!(
            read_story_narrator_tools(&pool, "second").unwrap(),
            NarratorToolSettings::default()
        );
        assert_eq!(read_story_reasoning_effort(&pool, "first").unwrap(), "high");
        let conn = pool.get().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT settings_json FROM stories WHERE id='first'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let saved: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(saved["author_note"], "hello");
        assert_eq!(saved["custom"], 42);
        assert!(save_reasoning_effort(&pool, "first", "invalid").is_err());
        assert_eq!(read_story_reasoning_effort(&pool, "first").unwrap(), "high");
        save_reasoning_effort(&pool, "first", "").unwrap();
        assert_eq!(read_story_reasoning_effort(&pool, "first").unwrap(), "");
        assert_eq!(read_story_narrator_tools(&pool, "first").unwrap(), tools);
        assert!(matches!(
            save_narrator_tools(&pool, "missing", tools),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn tool_shape_rejects_partial_and_unknown_fields() {
        assert!(
            serde_json::from_value::<NarratorToolSettings>(json!({"roll_check": true})).is_err()
        );
        let mut value = json!(NarratorToolSettings::default());
        value["unexpected"] = json!(true);
        assert!(serde_json::from_value::<NarratorToolSettings>(value).is_err());
    }
}
