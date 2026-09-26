use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::features::{
    ledger::{model::kind, repository},
    stories::settings::{story_settings, write_story_settings},
};
use crate::shared::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextItem {
    pub key: String,
    pub group: String,
    pub label: String,
    pub enabled: bool,
}

const FIXED_ITEMS: &[(&str, &str, &str, bool)] = &[
    ("action.do", "Player actions", "Do", true),
    ("action.say", "Player actions", "Say", true),
    ("action.story", "Player actions", "Story", true),
    ("action.guide", "Player actions", "Guide", true),
    ("action.continue", "Player actions", "Continue", true),
    ("action.see", "Player actions", "See", false),
    ("narration", "Narration", "Narration text", true),
    (
        "narration.thoughts",
        "Narration",
        "Thoughts (reasoning)",
        false,
    ),
];

// These are the only record kinds whose history can be independently toggled.
// content_edited is folded into its target; context_summary is the compaction
// boundary and disabling it loses history; diceroll_settings_changed is legacy
// and has never been sent to the narrator.
fn record_label(kind: &str) -> (&'static str, bool) {
    match kind {
        kind::DICEROLL => ("Dice rolls", true),
        kind::IMAGE_GENERATED => ("Image prompts", false),
        kind::TOOL_CALL => ("Tool calls", false),
        kind::ENTITY_CREATED => ("Entity created", true),
        kind::ENTITY_QUERIED => ("Entity lookups", true),
        kind::ENTITY_UPDATED => ("Entity updated", true),
        kind::ENTITY_DELETED => ("Entity deleted", true),
        kind::ENTITY_ATTRIBUTE_CHANGED => ("Attribute changed", true),
        kind::ENTITY_ATTRIBUTE_REMOVED => ("Attribute removed", true),
        kind::CONTEXT_NOTE_UPDATED => ("Author's note updates", false),
        _ => ("Unknown record", false),
    }
}

fn catalog() -> Vec<ContextItem> {
    let mut items: Vec<_> = FIXED_ITEMS
        .iter()
        .map(|&(key, group, label, enabled)| ContextItem {
            key: key.into(),
            group: group.into(),
            label: label.into(),
            enabled,
        })
        .collect();
    items.extend(kind::RECORD_KINDS.iter().map(|kind| {
        let (label, enabled) = record_label(kind);
        ContextItem {
            key: format!("record.{kind}"),
            group: "Records".into(),
            label: label.into(),
            enabled,
        }
    }));
    items.push(ContextItem {
        key: "images".into(),
        group: "Images".into(),
        label: "The images themselves".into(),
        enabled: false,
    });
    items
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextSettings {
    pub include: BTreeMap<String, bool>,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            include: catalog()
                .into_iter()
                .map(|item| (item.key, item.enabled))
                .collect(),
        }
    }
}

impl ContextSettings {
    pub fn includes(&self, key: &str) -> bool {
        self.include.get(key).copied().unwrap_or(false)
    }

    pub fn items(&self) -> Vec<ContextItem> {
        catalog()
            .into_iter()
            .map(|mut item| {
                item.enabled = self.includes(&item.key);
                item
            })
            .collect()
    }

    fn resolve(value: Option<&Value>) -> Self {
        let mut settings = Self::default();
        for (key, include) in &mut settings.include {
            if let Some(value) = value
                .and_then(|v| v.get("include"))
                .and_then(|v| v.get(key))
                .and_then(Value::as_bool)
            {
                *include = value;
            }
        }
        settings
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityInjection {
    #[default]
    All,
    Scoped,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InjectionSettings {
    pub entities: EntityInjection,
    pub author_note_enabled: bool,
    pub author_note: String,
    pub tool_instructions: bool,
}

impl Default for InjectionSettings {
    fn default() -> Self {
        Self {
            entities: EntityInjection::All,
            author_note_enabled: true,
            author_note: String::new(),
            tool_instructions: true,
        }
    }
}

pub fn read_context_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<ContextSettings> {
    let settings = story_settings(conn, story_id)?;
    Ok(ContextSettings::resolve(settings.get("context")))
}

pub fn write_context_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
    include: BTreeMap<String, bool>,
) -> AppResult<()> {
    let defaults = ContextSettings::default();
    for key in include.keys() {
        if !defaults.include.contains_key(key) {
            return Err(AppError::Invalid(format!("unknown context item: {key}")));
        }
    }
    let mut settings = story_settings(conn, story_id)?;
    settings["context"] = json!({"include": include});
    write_story_settings(conn, story_id, &settings)
}

/// A wrong-typed field resets the whole injection object to its defaults.
pub fn read_injection_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<InjectionSettings> {
    let settings = story_settings(conn, story_id)?;
    let mut injection: InjectionSettings = settings
        .get("injection")
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap_or_default())
        .unwrap_or_default();
    injection.author_note = injection.author_note.trim().to_string();
    Ok(injection)
}

pub fn write_injection_settings(
    conn: &rusqlite::Connection,
    story_id: &str,
    mut injection: InjectionSettings,
) -> AppResult<()> {
    injection.author_note = injection.author_note.trim().to_string();
    let mut settings = story_settings(conn, story_id)?;
    let previous = read_injection_settings(conn, story_id)?;
    settings["injection"] = json!(injection);
    write_story_settings(conn, story_id, &settings)?;
    if previous.author_note != injection.author_note {
        repository::append_entry(
            conn,
            story_id,
            kind::CONTEXT_NOTE_UPDATED,
            "hidden",
            Some(&format!(
                "Author's note was updated: {}",
                injection.author_note
            )),
            &json!({"author_note": injection.author_note}),
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

    fn read_story_context_settings(pool: &Pool, story_id: &str) -> AppResult<ContextSettings> {
        let conn = pool.get()?;
        read_context_settings(&conn, story_id)
    }

    fn save_story_context_settings(
        pool: &Pool,
        story_id: &str,
        context: ContextSettings,
    ) -> AppResult<()> {
        with_transaction(pool, |tx| {
            write_context_settings(tx, story_id, context.include)
        })
    }

    fn read_story_injection_settings(pool: &Pool, story_id: &str) -> AppResult<InjectionSettings> {
        let conn = pool.get()?;
        read_injection_settings(&conn, story_id)
    }

    fn save_story_injection_settings(
        pool: &Pool,
        story_id: &str,
        injection: InjectionSettings,
    ) -> AppResult<()> {
        with_transaction(pool, |tx| write_injection_settings(tx, story_id, injection))
    }

    fn stories() -> Pool {
        let pool = crate::shared::db::test_pool();
        pool.get()
            .unwrap()
            .execute_batch(
                "INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES
             ('first','First','now','now','{\"custom\":42}'),
             ('second','Second','now','now','{}');",
            )
            .unwrap();
        pool
    }

    #[test]
    fn catalog_resolves_defaults_and_rejects_unknown_saves() {
        let pool = stories();
        let defaults = read_story_context_settings(&pool, "first").unwrap();
        assert_eq!(defaults.include.len(), catalog().len());
        assert!(!defaults.includes("action.see"));
        assert!(defaults.includes("action.do"));
        assert!(defaults.includes("record.diceroll"));
        assert!(!defaults.includes("record.context_note_updated"));
        let record_keys: Vec<String> = catalog()
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
            [json!({"context":{"include":{"action.do":false,"unknown":true,"action.say":"invalid"}},"custom":42}).to_string()]).unwrap();
        drop(conn);
        let resolved = read_story_context_settings(&pool, "first").unwrap();
        assert!(!resolved.include["action.do"]);
        assert!(resolved.include["action.say"]);
        assert!(!resolved.include.contains_key("unknown"));
        let mut invalid = resolved.clone();
        invalid.include.insert("unknown".into(), true);
        assert!(matches!(
            save_story_context_settings(&pool, "first", invalid),
            Err(AppError::Invalid(_))
        ));
        write_context_settings(
            &pool.get().unwrap(),
            "first",
            BTreeMap::from([("action.do".into(), false)]),
        )
        .unwrap();
        assert_eq!(
            read_story_context_settings(&pool, "first").unwrap(),
            resolved
        );
        let item = read_story_context_settings(&pool, "first")
            .unwrap()
            .items()
            .into_iter()
            .find(|item| item.key == "action.do")
            .unwrap();
        assert_eq!(item.group, "Player actions");
        assert_eq!(item.label, "Do");
        assert!(!item.enabled);
        save_story_context_settings(&pool, "first", resolved.clone()).unwrap();
        assert_eq!(
            read_story_context_settings(&pool, "first").unwrap(),
            resolved
        );
        assert_eq!(
            read_story_context_settings(&pool, "second").unwrap(),
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
            saved["context"]["include"].as_object().unwrap().len(),
            catalog().len()
        );
        assert!(matches!(
            save_story_context_settings(&pool, "missing", defaults),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn partial_injection_uses_serde_defaults() {
        let pool = stories();
        let conn = pool.get().unwrap();
        conn.execute("UPDATE stories SET settings_json = '{\"injection\":{\"author_note\":\"x\"}}' WHERE id = 'first'", []).unwrap();
        assert_eq!(
            read_injection_settings(&conn, "first").unwrap(),
            InjectionSettings {
                author_note: "x".into(),
                ..InjectionSettings::default()
            }
        );
    }

    #[test]
    fn injection_changes_only_log_text_edits_and_preserve_other_story_settings() {
        let pool = stories();
        let mut injection = read_story_injection_settings(&pool, "first").unwrap();
        assert_eq!(injection, InjectionSettings::default());
        injection.entities = EntityInjection::Scoped;
        injection.author_note_enabled = false;
        injection.tool_instructions = false;
        save_story_injection_settings(&pool, "first", injection.clone()).unwrap();
        let count = || -> i64 {
            pool.get().unwrap().query_row("SELECT COUNT(*) FROM ledger_entries WHERE story_id = 'first' AND kind = 'context_note_updated'", [], |row| row.get(0)).unwrap()
        };
        assert_eq!(count(), 0);
        injection.author_note = "  Stay quiet.  ".into();
        save_story_injection_settings(&pool, "first", injection.clone()).unwrap();
        assert_eq!(count(), 1);
        assert_eq!(
            read_story_injection_settings(&pool, "first")
                .unwrap()
                .author_note,
            "Stay quiet."
        );
        injection.author_note = "Stay quiet.".into();
        injection.author_note_enabled = true;
        save_story_injection_settings(&pool, "first", injection.clone()).unwrap();
        assert_eq!(count(), 1);
        let entry: (String, String, String) = pool.get().unwrap().query_row(
            "SELECT visibility, content, payload_json FROM ledger_entries WHERE story_id = 'first' AND kind = 'context_note_updated'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ).unwrap();
        assert_eq!(entry.0, "hidden");
        assert_eq!(entry.1, "Author's note was updated: Stay quiet.");
        assert_eq!(
            serde_json::from_str::<Value>(&entry.2).unwrap(),
            json!({"author_note":"Stay quiet."})
        );
        injection.author_note.clear();
        save_story_injection_settings(&pool, "first", injection).unwrap();
        assert_eq!(count(), 2);
        assert_eq!(
            read_story_injection_settings(&pool, "second").unwrap(),
            InjectionSettings::default()
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
            save_story_injection_settings(&pool, "missing", InjectionSettings::default()),
            Err(AppError::NotFound(_))
        ));
    }
}
