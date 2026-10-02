use std::collections::BTreeMap;

use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::features::transcript::{filter::TranscriptSettings, model::kind, repository};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NarratorToolSettings {
    pub save_relationship: bool,
    pub create_entity: bool,
    pub update_entity: bool,
    pub adjust_entity_attribute: bool,
    pub roll_check: bool,
    pub illustrate_scene: bool,
}

impl Default for NarratorToolSettings {
    fn default() -> Self {
        Self {
            save_relationship: true,
            create_entity: true,
            update_entity: true,
            adjust_entity_attribute: true,
            roll_check: true,
            illustrate_scene: true,
        }
    }
}

pub(crate) fn story_settings(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Value> {
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

pub(crate) fn write_story_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
    settings: &Value,
) -> AppResult<()> {
    conn.execute(
        "UPDATE stories SET settings_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![settings.to_string(), Utc::now().to_rfc3339(), story_id],
    )?;
    Ok(())
}

pub fn read_transcript_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<TranscriptSettings> {
    let settings = story_settings(conn, story_id)?;
    Ok(TranscriptSettings::from_stored(settings.get("transcript")))
}

pub fn write_transcript_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
    include: BTreeMap<String, bool>,
) -> AppResult<()> {
    TranscriptSettings::validate_keys(&include)?;
    let mut settings = story_settings(conn, story_id)?;
    settings["transcript"] = json!({"include": include});
    write_story_settings(conn, story_id, &settings)
}

pub fn read_story_narrator_tools(pool: &Pool, story_id: &str) -> AppResult<NarratorToolSettings> {
    let conn = pool.get()?;
    read_story_narrator_tools_conn(&conn, story_id)
}

pub(crate) fn read_story_narrator_tools_conn(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<NarratorToolSettings> {
    let settings = story_settings(conn, story_id)?;
    match settings.get("narrator_tools") {
        Some(tools) => serde_json::from_value(tools.clone())
            .map_err(|error| AppError::Other(format!("invalid narrator tools: {error}"))),
        None => Ok(NarratorToolSettings::default()),
    }
}

pub(super) fn save_narrator_tools(
    pool: &Pool,
    story_id: &str,
    tools: NarratorToolSettings,
) -> AppResult<()> {
    with_transaction(pool, |tx| {
        let mut settings = story_settings(tx, story_id)?;
        settings["narrator_tools"] = json!(tools);
        write_story_settings(tx, story_id, &settings)
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
    read_story_reasoning_effort_conn(&conn, story_id)
}

pub(crate) fn read_story_reasoning_effort_conn(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<String> {
    let settings = story_settings(conn, story_id)?;
    Ok(settings
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
        .unwrap_or_default()
        .to_string())
}

pub(super) fn save_reasoning_effort(
    pool: &Pool,
    story_id: &str,
    reasoning_effort: &str,
) -> AppResult<()> {
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
        write_story_settings(tx, story_id, &settings)
    })
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityContext {
    #[default]
    All,
    Scoped,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ContextSettings {
    pub entities: EntityContext,
    pub author_note_enabled: bool,
    pub author_note: String,
    pub tool_instructions: bool,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            entities: EntityContext::All,
            author_note_enabled: true,
            author_note: String::new(),
            tool_instructions: true,
        }
    }
}

/// A wrong-typed field resets the whole context object to its defaults.
pub fn read_context_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<ContextSettings> {
    let settings = story_settings(conn, story_id)?;
    let mut context: ContextSettings = settings
        .get("context")
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap_or_default())
        .unwrap_or_default();
    context.author_note = context.author_note.trim().to_string();
    Ok(context)
}

pub fn write_context_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
    mut context: ContextSettings,
) -> AppResult<()> {
    context.author_note = context.author_note.trim().to_string();
    let mut settings = story_settings(conn, story_id)?;
    let previous = read_context_settings(conn, story_id)?;
    settings["context"] = json!(context);
    write_story_settings(conn, story_id, &settings)?;
    if previous.author_note != context.author_note {
        repository::append_entry(
            conn,
            story_id,
            kind::CONTEXT_NOTE_UPDATED,
            "hidden",
            Some(&format!(
                "Author's note was updated: {}",
                context.author_note
            )),
            &json!({"author_note": context.author_note}),
            None,
            None,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::db::{with_transaction, Pool};
    use crate::shared::test_support;

    #[test]
    fn stored_transcript_and_context_keys_are_read_after_reopen() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story_with_settings(&conn, "s", json!({
            "transcript": {"include": {"images": true}},
            "context": {"author_note": "n", "entities": "scoped"}
        }));
        let path = crate::shared::db::database_path(&pool).unwrap();
        drop(conn);
        drop(pool);
        let pool = crate::shared::db::init_pool(path.parent().unwrap()).unwrap();
        let conn = pool.get().unwrap();
        assert!(read_transcript_settings(&conn, "s").unwrap().includes("images"));
        let context = read_context_settings(&conn, "s").unwrap();
        assert_eq!(context.author_note, "n");
        assert_eq!(context.entities, EntityContext::Scoped);
    }

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
    fn tool_shape_defaults_missing_fields_and_rejects_unknown_fields() {
        let tools = serde_json::from_value::<NarratorToolSettings>(json!({"roll_check": false})).unwrap();
        assert!(!tools.roll_check);
        assert!(tools.save_relationship);
        let mut value = json!(NarratorToolSettings::default());
        value["unexpected"] = json!(true);
        assert!(serde_json::from_value::<NarratorToolSettings>(value).is_err());
    }

    #[test]
    fn connection_readers_see_uncommitted_story_settings() {
        let pool = crate::shared::db::test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES ('s','Story','now','now','{}')",
            [],
        ).unwrap();
        let tx = conn.transaction().unwrap();
        let tools = NarratorToolSettings {
            roll_check: false,
            ..NarratorToolSettings::default()
        };
        tx.execute(
            "UPDATE stories SET settings_json = ?1 WHERE id = 's'",
            [json!({"narrator_tools": tools, "reasoning_effort": "HIGH"}).to_string()],
        )
        .unwrap();

        assert_eq!(read_story_narrator_tools_conn(&tx, "s").unwrap(), tools);
        assert_eq!(read_story_reasoning_effort_conn(&tx, "s").unwrap(), "high");
        assert_eq!(
            read_story_narrator_tools(&pool, "s").unwrap(),
            NarratorToolSettings::default()
        );
        tx.rollback().unwrap();
    }

    fn read_story_transcript_settings(pool: &Pool, story_id: &str) -> AppResult<TranscriptSettings> {
        let conn = pool.get()?;
        read_transcript_settings(&conn, story_id)
    }

    fn save_story_transcript_settings(
        pool: &Pool,
        story_id: &str,
        context: TranscriptSettings,
    ) -> AppResult<()> {
        with_transaction(pool, |tx| {
            write_transcript_settings(tx, story_id, context.include)
        })
    }

    fn read_story_context_settings(pool: &Pool, story_id: &str) -> AppResult<ContextSettings> {
        let conn = pool.get()?;
        read_context_settings(&conn, story_id)
    }

    fn save_story_context_settings(
        pool: &Pool,
        story_id: &str,
        context: ContextSettings,
    ) -> AppResult<()> {
        with_transaction(pool, |tx| write_context_settings(tx, story_id, context))
    }

    fn stories() -> Pool {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story_with_settings(&conn, "first", json!({"custom":42}));
        test_support::story(&conn, "second");
        pool
    }

    #[test]
    fn catalog_resolves_defaults_and_rejects_unknown_saves() {
        let pool = stories();
        let defaults = read_story_transcript_settings(&pool, "first").unwrap();
        assert_eq!(defaults.include.len(), defaults.items().len());
        assert!(!defaults.includes("action.see"));
        assert!(defaults.includes("action.do"));
        assert!(defaults.includes("record.diceroll"));
        assert!(defaults.includes("record.image_captioned"));
        assert!(!defaults.includes("record.context_note_updated"));
        let caption = defaults
            .items()
            .into_iter()
            .find(|item| item.key == "record.image_captioned")
            .unwrap();
        assert_eq!(caption.label, "Image captions");
        assert!(caption.enabled);
        let record_keys: Vec<String> = defaults.items()
            .into_iter()
            .filter(|item| item.group == "Records")
            .map(|item| item.key)
            .collect();
        assert_eq!(
            record_keys,
            kind::RECORD_KINDS
                .iter()
                .map(|kind| format!("record.{kind}"))
                .collect::<Vec<_>>()
        );

        let conn = pool.get().unwrap();
        conn.execute("UPDATE stories SET settings_json = ?1 WHERE id = 'first'",
            [json!({"transcript":{"include":{"action.do":false,"unknown":true,"action.say":"invalid"}},"custom":42}).to_string()]).unwrap();
        drop(conn);
        let resolved = read_story_transcript_settings(&pool, "first").unwrap();
        assert!(!resolved.include["action.do"]);
        assert!(resolved.include["action.say"]);
        assert!(!resolved.include.contains_key("unknown"));
        let mut invalid = resolved.clone();
        invalid.include.insert("unknown".into(), true);
        assert!(matches!(
            save_story_transcript_settings(&pool, "first", invalid),
            Err(AppError::Invalid(_))
        ));
        write_transcript_settings(
            &pool.get().unwrap(),
            "first",
            BTreeMap::from([("action.do".into(), false)]),
        )
        .unwrap();
        assert_eq!(
            read_story_transcript_settings(&pool, "first").unwrap(),
            resolved
        );
        let item = read_story_transcript_settings(&pool, "first")
            .unwrap()
            .items()
            .into_iter()
            .find(|item| item.key == "action.do")
            .unwrap();
        assert_eq!(item.group, "Player actions");
        assert_eq!(item.label, "Do");
        assert!(!item.enabled);
        save_story_transcript_settings(&pool, "first", resolved.clone()).unwrap();
        assert_eq!(
            read_story_transcript_settings(&pool, "first").unwrap(),
            resolved
        );
        assert_eq!(
            read_story_transcript_settings(&pool, "second").unwrap(),
            defaults
        );
        let raw: String = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT settings_json FROM stories WHERE id = 'first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let saved: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(saved["custom"], 42);
        assert_eq!(
            saved["transcript"]["include"].as_object().unwrap().len(),
            defaults.items().len()
        );
        assert!(matches!(
            save_story_transcript_settings(&pool, "missing", defaults),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn partial_context_uses_serde_defaults() {
        let pool = stories();
        let conn = pool.get().unwrap();
        conn.execute("UPDATE stories SET settings_json = '{\"context\":{\"author_note\":\"x\"}}' WHERE id = 'first'", []).unwrap();
        assert_eq!(
            read_context_settings(&conn, "first").unwrap(),
            ContextSettings {
                author_note: "x".into(),
                ..ContextSettings::default()
            }
        );
    }

    #[test]
    fn context_changes_only_log_text_edits_and_preserve_other_story_settings() {
        let pool = stories();
        let mut context = read_story_context_settings(&pool, "first").unwrap();
        assert_eq!(context, ContextSettings::default());
        context.entities = EntityContext::Scoped;
        context.author_note_enabled = false;
        context.tool_instructions = false;
        save_story_context_settings(&pool, "first", context.clone()).unwrap();
        let count = || -> i64 {
            pool.get().unwrap().query_row("SELECT COUNT(*) FROM transcript_entries WHERE story_id = 'first' AND kind = 'context_note_updated'", [], |row| row.get(0)).unwrap()
        };
        assert_eq!(count(), 0);
        context.author_note = "  Stay quiet.  ".into();
        save_story_context_settings(&pool, "first", context.clone()).unwrap();
        assert_eq!(count(), 1);
        assert_eq!(
            read_story_context_settings(&pool, "first")
                .unwrap()
                .author_note,
            "Stay quiet."
        );
        context.author_note = "Stay quiet.".into();
        context.author_note_enabled = true;
        save_story_context_settings(&pool, "first", context.clone()).unwrap();
        assert_eq!(count(), 1);
        let entry: (String, String, String) = pool.get().unwrap().query_row(
            "SELECT visibility, content, payload_json FROM transcript_entries WHERE story_id = 'first' AND kind = 'context_note_updated'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ).unwrap();
        assert_eq!(entry.0, "hidden");
        assert_eq!(entry.1, "Author's note was updated: Stay quiet.");
        assert_eq!(
            serde_json::from_str::<Value>(&entry.2).unwrap(),
            json!({"author_note":"Stay quiet."})
        );
        context.author_note.clear();
        save_story_context_settings(&pool, "first", context).unwrap();
        assert_eq!(count(), 2);
        assert_eq!(
            read_story_context_settings(&pool, "second").unwrap(),
            ContextSettings::default()
        );
        let raw: String = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT settings_json FROM stories WHERE id = 'first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap()["custom"], 42);
        assert!(matches!(
            save_story_context_settings(&pool, "missing", ContextSettings::default()),
            Err(AppError::NotFound(_))
        ));
    }}
