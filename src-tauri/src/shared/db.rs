use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

use crate::features::ledger::{model::kind, repository};
use crate::features::stories::settings::NarratorToolSettings;
use crate::shared::error::{AppError, AppResult};

pub type Pool = r2d2::Pool<SqliteConnectionManager>;
pub type PooledConn = r2d2::PooledConnection<SqliteConnectionManager>;

pub fn with_transaction<T>(
    pool: &Pool,
    f: impl FnOnce(&rusqlite::Transaction<'_>) -> AppResult<T>,
) -> AppResult<T> {
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

pub fn init_pool(app_data_dir: &Path) -> AppResult<Pool> {
    std::fs::create_dir_all(app_data_dir)?;
    let db_path = app_data_dir.join("dungeon.sqlite3");
    let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(())
    });
    let pool = r2d2::Pool::new(manager).map_err(AppError::Pool)?;
    let mut conn = pool.get().map_err(AppError::Pool)?;
    run_migrations(&mut conn)?;
    seed_attribute_registry(&conn)?;
    Ok(pool)
}

fn run_migrations(conn: &mut PooledConn) -> AppResult<()> {
    migrate_ledger_schema(conn)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS stories (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            settings_json TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE IF NOT EXISTS turns (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('pending', 'complete', 'failed')),
            attempt INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            UNIQUE(story_id, seq)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_turns_one_pending
            ON turns(story_id) WHERE status = 'pending';

        CREATE TABLE IF NOT EXISTS ledger_entries (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            visibility TEXT NOT NULL CHECK (visibility IN ('visible', 'hidden')),
            content TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            target_entry_id TEXT REFERENCES ledger_entries(id) ON DELETE CASCADE,
            turn_id TEXT REFERENCES turns(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            UNIQUE(story_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_ledger_story_seq ON ledger_entries(story_id, seq);
        CREATE INDEX IF NOT EXISTS idx_ledger_target ON ledger_entries(target_entry_id);
        CREATE INDEX IF NOT EXISTS idx_ledger_kind ON ledger_entries(story_id, kind, seq);

        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS story_entity_state (
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            appearance_anchor TEXT,
            is_present INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL,
            last_event_id TEXT REFERENCES ledger_entries(id) ON DELETE SET NULL,
            PRIMARY KEY (story_id, entity_id)
        );
        CREATE INDEX IF NOT EXISTS idx_story_entities_name ON story_entity_state(story_id, name);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_story_entities_name_ci
            ON story_entity_state(story_id, name COLLATE NOCASE);

        CREATE TABLE IF NOT EXISTS attribute_registry (
            id TEXT PRIMARY KEY,
            canonical_name TEXT NOT NULL UNIQUE,
            aliases_json TEXT NOT NULL DEFAULT '[]',
            entity_kinds_json TEXT NOT NULL DEFAULT '[]',
            min REAL NOT NULL,
            max REAL NOT NULL,
            category TEXT NOT NULL,
            is_user_created INTEGER NOT NULL DEFAULT 0,
            created_in_story_id TEXT,
            created_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_attribute_registry_canonical_ci
            ON attribute_registry(canonical_name COLLATE NOCASE);

        CREATE TABLE IF NOT EXISTS entity_attributes (
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            attribute_id TEXT NOT NULL REFERENCES attribute_registry(id) ON DELETE CASCADE,
            value REAL NOT NULL,
            source TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_event_id TEXT REFERENCES ledger_entries(id) ON DELETE SET NULL,
            PRIMARY KEY (story_id, entity_id, attribute_id)
        );

        CREATE TABLE IF NOT EXISTS image_assets (
            id TEXT PRIMARY KEY,
            entry_id TEXT NOT NULL REFERENCES ledger_entries(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            prompt TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_image_assets_entry ON image_assets(entry_id);

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;
    migrate_roll_needed_v1(conn)?;
    ensure_ledger_turn_column(conn)?;
    ensure_turn_attempt_column(conn)?;
    migrate_ledger_retention_settings(conn)?;
    migrate_narrator_memory_settings(conn)?;
    migrate_author_notes(conn)?;
    migrate_narrator_tools(conn)?;
    migrate_turns_v1(conn)?;
    conn.execute(
        "UPDATE turns SET status = 'failed' WHERE status = 'pending'",
        [],
    )?;
    Ok(())
}

fn migrate_roll_needed_v1(conn: &mut rusqlite::Connection) -> AppResult<()> {
    const MIGRATION_KEY: &str = "migration_roll_needed_v1";
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get::<_, bool>(0),
    )? {
        tx.commit()?;
        return Ok(());
    }

    tx.execute(
        "UPDATE ledger_entries
         SET payload_json = json_set(
             payload_json,
             '$.needed',
             100 - json_extract(payload_json, '$.chance_percent')
         )
         WHERE kind = 'diceroll'
           AND json_extract(payload_json, '$.needed') IS NULL
           AND json_extract(payload_json, '$.chance_percent') IS NOT NULL",
        [],
    )?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, '1')",
        [MIGRATION_KEY],
    )?;
    tx.commit()?;
    Ok(())
}

fn ensure_turn_attempt_column(conn: &rusqlite::Connection) -> AppResult<()> {
    let has_attempt: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('turns') WHERE name = 'attempt')",
        [],
        |row| row.get(0),
    )?;
    if !has_attempt {
        conn.execute(
            "ALTER TABLE turns ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn ensure_ledger_turn_column(conn: &rusqlite::Connection) -> AppResult<()> {
    let has_turn_id: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('ledger_entries') WHERE name = 'turn_id')",
        [],
        |row| row.get(0),
    )?;
    if !has_turn_id {
        conn.execute(
            "ALTER TABLE ledger_entries
             ADD COLUMN turn_id TEXT REFERENCES turns(id) ON DELETE CASCADE",
            [],
        )?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_ledger_turn ON ledger_entries(turn_id)",
        [],
    )?;
    Ok(())
}

fn migrate_turns_v1(conn: &mut rusqlite::Connection) -> AppResult<()> {
    const MIGRATION_KEY: &str = "migration_turns_v1";

    #[derive(Debug)]
    struct LegacyEntry {
        id: String,
        kind: String,
        visibility: String,
        payload: serde_json::Value,
        target_entry_id: Option<String>,
        created_at: String,
    }

    #[derive(Debug)]
    struct BackfilledTurn {
        id: String,
        created_at: String,
        has_player: bool,
        has_narration: bool,
        has_image: bool,
        visible_count: usize,
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let migrated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get(0),
    )?;
    if migrated {
        tx.commit()?;
        return Ok(());
    }

    let story_ids = {
        let mut stmt = tx.prepare("SELECT id FROM stories ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for story_id in story_ids {
        let entries = {
            let mut stmt = tx.prepare(
                "SELECT id, kind, visibility, payload_json, target_entry_id, created_at
                 FROM ledger_entries WHERE story_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map([&story_id], |row| {
                let raw: String = row.get(3)?;
                Ok(LegacyEntry {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    visibility: row.get(2)?,
                    payload: serde_json::from_str(&raw)
                        .unwrap_or(serde_json::Value::Object(Default::default())),
                    target_entry_id: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut turns = Vec::<BackfilledTurn>::new();
        let mut mapped = HashMap::<String, usize>::new();
        let mut open_player = None::<usize>;
        for entry in entries {
            let turn_index = if entry.visibility == "visible" && entry.kind == kind::PLAYER_MESSAGE
            {
                turns.push(BackfilledTurn {
                    id: Uuid::new_v4().to_string(),
                    created_at: entry.created_at.clone(),
                    has_player: true,
                    has_narration: false,
                    has_image: false,
                    visible_count: 1,
                });
                let index = turns.len() - 1;
                open_player = Some(index);
                Some(index)
            } else if entry.visibility == "visible" && entry.kind == kind::NARRATION {
                let generated = entry
                    .payload
                    .get("input_mode")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("generated")
                    == "generated";
                let joins = open_player.filter(|index| {
                    let turn = &turns[*index];
                    generated
                        && turn.has_player
                        && !turn.has_narration
                        && !turn.has_image
                        && turn.visible_count == 1
                });
                let index = match joins {
                    Some(index) => index,
                    None => {
                        turns.push(BackfilledTurn {
                            id: Uuid::new_v4().to_string(),
                            created_at: entry.created_at.clone(),
                            has_player: false,
                            has_narration: false,
                            has_image: false,
                            visible_count: 0,
                        });
                        turns.len() - 1
                    }
                };
                turns[index].has_narration = true;
                turns[index].visible_count += 1;
                open_player = None;
                Some(index)
            } else if entry.visibility == "hidden" {
                entry
                    .target_entry_id
                    .as_ref()
                    .and_then(|target| mapped.get(target).copied())
            } else {
                None
            };

            if let Some(index) = turn_index {
                if entry.kind == kind::IMAGE_GENERATED {
                    turns[index].has_image = true;
                }
                mapped.insert(entry.id, index);
            }
        }

        for (seq, turn) in turns.iter().enumerate() {
            let status = if turn.has_narration || turn.has_image {
                "complete"
            } else {
                "failed"
            };
            tx.execute(
                "INSERT INTO turns (id, story_id, seq, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![turn.id, story_id, seq as i64, status, turn.created_at],
            )?;
        }
        for (entry_id, turn_index) in mapped {
            tx.execute(
                "UPDATE ledger_entries SET turn_id = ?1 WHERE id = ?2",
                rusqlite::params![turns[turn_index].id, entry_id],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, '1')",
        [MIGRATION_KEY],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_ledger_schema(conn: &mut rusqlite::Connection) -> AppResult<()> {
    let old_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'timeline_entries')",
        [],
        |row| row.get(0),
    )?;
    if !old_exists {
        return Ok(());
    }
    let new_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ledger_entries')",
        [],
        |row| row.get(0),
    )?;
    if new_exists {
        return Err(AppError::Other(
            "both timeline_entries and ledger_entries exist; refusing an ambiguous migration"
                .into(),
        ));
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "ALTER TABLE timeline_entries RENAME TO ledger_entries;
         DROP INDEX IF EXISTS idx_timeline_story_seq;
         DROP INDEX IF EXISTS idx_timeline_target;
         DROP INDEX IF EXISTS idx_timeline_kind;
         CREATE INDEX idx_ledger_story_seq ON ledger_entries(story_id, seq);
         CREATE INDEX idx_ledger_target ON ledger_entries(target_entry_id);
         CREATE INDEX idx_ledger_kind ON ledger_entries(story_id, kind, seq);",
    )?;
    if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(AppError::Other(
            "ledger migration failed foreign key validation".into(),
        ));
    }
    tx.commit()?;
    Ok(())
}

fn migrate_ledger_retention_settings(conn: &mut rusqlite::Connection) -> AppResult<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO settings (key, value)
         SELECT 'ledger_retention', value FROM settings WHERE key = 'timeline_retention'",
        [],
    )?;
    tx.execute("DELETE FROM settings WHERE key = 'timeline_retention'", [])?;
    tx.commit()?;
    Ok(())
}

/// Idempotent on name rather than on a migration marker: creation and upgrade
/// both call this with their existing story transaction.
pub(crate) fn seed_player_entity(conn: &rusqlite::Connection, story_id: &str) -> AppResult<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM story_entity_state
         WHERE story_id = ?1 AND name = 'You' COLLATE NOCASE)",
        [story_id],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(());
    }
    let id = Uuid::new_v4().to_string();
    crate::features::entities::create_entity_with_id_sync(
        conn,
        &id,
        story_id,
        "character",
        "You",
        None,
        "story_bootstrap",
        None,
    )?;
    Ok(())
}

fn migrate_narrator_tools(conn: &mut rusqlite::Connection) -> AppResult<()> {
    const MIGRATION_KEY: &str = "migration_narrator_tools_stateless_roll_v1";
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get::<_, bool>(0),
    )? {
        tx.commit()?;
        return Ok(());
    }

    let stories = {
        let mut stmt = tx.prepare("SELECT id, settings_json FROM stories")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (story_id, raw) in stories {
        let mut settings = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({}));
        if !settings.is_object() {
            settings = serde_json::json!({});
        }
        let object = settings.as_object_mut().unwrap();
        object.remove("attributes_enabled");
        object.remove("dice_mode");
        object.insert(
            "narrator_tools".into(),
            serde_json::json!(NarratorToolSettings::default()),
        );
        tx.execute(
            "UPDATE stories SET settings_json = ?1 WHERE id = ?2",
            rusqlite::params![settings.to_string(), story_id],
        )?;
        seed_player_entity(&tx, &story_id)?;

        let entries = repository::list_logical_entries(&tx, &story_id)?;
        for base in entries.iter().filter(|entry| entry.kind == kind::NARRATION) {
            // Mirror the legacy reducer before removing its variant records.
            let selected_id = entries
                .iter()
                .rev()
                .filter(|entry| entry.target_entry_id.as_deref() == Some(&base.id))
                .find_map(|entry| match entry.kind.as_str() {
                    "narration_variant" => Some(entry.id.as_str()),
                    "narration_selected" => entry
                        .payload
                        .get("selected_entry_id")
                        .and_then(|id| id.as_str()),
                    _ => None,
                })
                .unwrap_or(&base.id);
            let selected = entries.iter().find(|entry| {
                entry.id == selected_id
                    && entry.kind == "narration_variant"
                    && entry.target_entry_id.as_deref() == Some(&base.id)
            });
            let selected_id = selected.map(|entry| entry.id.as_str()).unwrap_or(&base.id);
            let edit = entries.iter().rev().find(|entry| {
                entry.kind == kind::CONTENT_EDITED
                    && entry.target_entry_id.as_deref() == Some(&base.id)
                    && entry
                        .payload
                        .get("applies_to")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&base.id)
                        == selected_id
                    && entry.content.is_some()
            });
            let content = edit
                .and_then(|entry| entry.content.as_ref())
                .or_else(|| selected.and_then(|entry| entry.content.as_ref()))
                .or(base.content.as_ref());
            let mut payload = base.payload.clone();
            if let Some(variant) = selected {
                if let Some(object) = payload.as_object_mut() {
                    if let Some(thoughts) = variant.payload.get("thoughts") {
                        object.insert("thoughts".into(), thoughts.clone());
                    } else {
                        object.remove("thoughts");
                    }
                }
            }
            tx.execute(
                "UPDATE ledger_entries SET content = ?1, payload_json = ?2 WHERE id = ?3",
                rusqlite::params![content, payload.to_string(), base.id],
            )?;
        }
    }

    // Narration edits are now folded into base content; player edits stay as
    // events. Cascades remove any dependents of discarded alternatives.
    tx.execute(
        "DELETE FROM ledger_entries WHERE kind = ?1 AND target_entry_id IN
         (SELECT id FROM ledger_entries WHERE kind = ?2)",
        rusqlite::params![kind::CONTENT_EDITED, kind::NARRATION],
    )?;
    tx.execute(
        "DELETE FROM ledger_entries WHERE kind IN (?1, ?2, ?3, ?4)",
        rusqlite::params![
            "narration_variant",
            "narration_selected",
            kind::DICEROLL,
            kind::DICEROLL_SETTINGS_CHANGED
        ],
    )?;

    // A summary covering a deleted roll/revision can no longer identify its
    // original cut; discard it instead of hiding still-visible history.
    let summaries = {
        let mut stmt = tx.prepare(
            "SELECT id, story_id, seq, payload_json FROM ledger_entries WHERE kind = ?1",
        )?;
        let rows = stmt.query_map([kind::CONTEXT_SUMMARY], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, story_id, seq, raw) in summaries {
        let boundary = serde_json::from_str::<serde_json::Value>(&raw).ok();
        let through_id = boundary
            .as_ref()
            .and_then(|value| value.get("through_entry_id"))
            .and_then(|value| value.as_str());
        let through_seq = boundary
            .as_ref()
            .and_then(|value| value.get("through_seq"))
            .and_then(|value| value.as_i64());
        let valid = if let (Some(through_id), Some(through_seq)) = (through_id, through_seq) {
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM ledger_entries WHERE id = ?1 AND story_id = ?2 AND seq = ?3)",
                rusqlite::params![through_id, story_id, through_seq],
                |row| row.get::<_, bool>(0),
            )? && through_seq < seq
        } else {
            false
        };
        if !valid {
            tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [id])?;
        }
    }

    let image: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = 'image_model_default'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(raw) = image {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(object) = value.as_object_mut() {
                if object.remove("narrator_images").is_some() {
                    tx.execute(
                        "UPDATE settings SET value = ?1 WHERE key = 'image_model_default'",
                        [value.to_string()],
                    )?;
                }
            }
        }
    }
    tx.execute("DELETE FROM settings WHERE key = 'narrator_images'", [])?;
    if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(AppError::Other(
            "narrator tools migration failed foreign key validation".into(),
        ));
    }
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'true')",
        [MIGRATION_KEY],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_narrator_memory_settings(conn: &mut rusqlite::Connection) -> AppResult<()> {
    const MIGRATION_KEY: &str = "migration_narrator_memory_split";
    let tx = conn.transaction()?;
    let migrated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get(0),
    )?;
    if migrated {
        tx.execute("DELETE FROM settings WHERE key = 'narrator_memory'", [])?;
        tx.commit()?;
        return Ok(());
    }

    let legacy: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = 'narrator_memory'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(legacy) = legacy {
        let legacy = serde_json::from_str::<serde_json::Value>(&legacy)
            .unwrap_or_else(|_| serde_json::json!({}));
        if let Some(entity_context_mode) = legacy
            .get("entity_context_mode")
            .and_then(serde_json::Value::as_str)
            .filter(|mode| matches!(*mode, "all" | "scoped" | "none"))
        {
            let context: Option<String> = tx
                .query_row(
                    "SELECT value FROM settings WHERE key = 'context_injection'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let mut context = context
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            if context
                .get("entity_context_mode")
                .and_then(serde_json::Value::as_str)
                != Some(entity_context_mode)
            {
                context["entity_context_mode"] = serde_json::json!(entity_context_mode);
                tx.execute(
                    "INSERT INTO settings (key, value) VALUES ('context_injection', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    [context.to_string()],
                )?;
            }
        }
        if let Some(tool_call_persistence) = legacy
            .get("tool_call_persistence")
            .and_then(serde_json::Value::as_bool)
        {
            let retention: Option<String> = tx
                .query_row(
                    "SELECT value FROM settings WHERE key = 'ledger_retention'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let retention =
                retention.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok());
            if retention
                .as_ref()
                .and_then(|value| value.get("tool_call_persistence"))
                .and_then(serde_json::Value::as_bool)
                != Some(tool_call_persistence)
            {
                tx.execute(
                    "INSERT INTO settings (key, value) VALUES ('ledger_retention', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    [
                        serde_json::json!({"tool_call_persistence": tool_call_persistence})
                            .to_string(),
                    ],
                )?;
            }
        }
    }
    tx.execute("DELETE FROM settings WHERE key = 'narrator_memory'", [])?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'true')",
        [MIGRATION_KEY],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_author_notes(conn: &rusqlite::Connection) -> AppResult<()> {
    const MIGRATION_KEY: &str = "migration_author_notes_to_story_settings";
    let migrated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get(0),
    )?;
    if migrated {
        return Ok(());
    }

    let notes = {
        let mut stmt = conn.prepare(
            "SELECT story_id, payload_json FROM ledger_entries AS note
             WHERE kind = 'context_note_updated'
               AND seq = (
                    SELECT MAX(latest.seq) FROM ledger_entries AS latest
                   WHERE latest.story_id = note.story_id
                     AND latest.kind = 'context_note_updated'
               )",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (story_id, payload_json) in notes {
        let note = serde_json::from_str::<serde_json::Value>(&payload_json)
            .ok()
            .and_then(|payload| {
                payload
                    .get("author_note")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if note.is_empty() {
            continue;
        }
        let raw: String = conn.query_row(
            "SELECT settings_json FROM stories WHERE id = ?1",
            [&story_id],
            |row| row.get(0),
        )?;
        let mut settings = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({}));
        if settings.get("author_note").is_some() {
            continue;
        }
        settings["author_note"] = serde_json::json!(note);
        conn.execute(
            "UPDATE stories SET settings_json = ?1 WHERE id = ?2",
            rusqlite::params![settings.to_string(), story_id],
        )?;
    }
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'true')",
        [MIGRATION_KEY],
    )?;
    Ok(())
}

fn seed_attribute_registry(conn: &PooledConn) -> AppResult<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let starters: &[(&str, &[&str], f64, f64, &str)] = &[
        ("Accuracy", &["character"], 0.0, 10.0, "combat"),
        ("Evasion", &["character"], 0.0, 10.0, "combat"),
        ("Resolve", &["character"], 0.0, 10.0, "mental"),
        ("Charisma", &["character"], 0.0, 10.0, "social"),
        ("Stealth", &["character"], 0.0, 10.0, "skill"),
        ("Health", &["character"], 0.0, 10.0, "vital"),
        ("Strength", &["character"], 0.0, 10.0, "physical"),
        ("Intelligence", &["character"], 0.0, 10.0, "mental"),
        ("Perception", &["character"], 0.0, 10.0, "skill"),
        ("Luck", &["character"], 0.0, 10.0, "misc"),
        ("Lock Difficulty", &["object"], 0.0, 10.0, "object"),
        ("Fragility", &["object"], 0.0, 10.0, "object"),
        ("Weight", &["object"], 0.0, 10.0, "object"),
        ("Trap Sensitivity", &["object"], 0.0, 10.0, "object"),
        ("Value", &["object"], 0.0, 10.0, "object"),
        ("Durability", &["object"], 0.0, 10.0, "object"),
        ("Visibility", &["location"], 0.0, 10.0, "location"),
        ("Terrain Difficulty", &["location"], 0.0, 10.0, "location"),
        ("Ambient Danger", &["location"], 0.0, 10.0, "location"),
        ("Shelter", &["location"], 0.0, 10.0, "location"),
        ("Trust", &["relationship"], -10.0, 10.0, "relationship"),
        ("Fear", &["relationship"], -10.0, 10.0, "relationship"),
        ("Affection", &["relationship"], -10.0, 10.0, "relationship"),
        ("Respect", &["relationship"], -10.0, 10.0, "relationship"),
        ("Difficulty", &["campaign"], 0.0, 10.0, "campaign"),
    ];

    for (name, kinds, min, max, category) in starters {
        let kinds_json = serde_json::to_string(kinds).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT OR IGNORE INTO attribute_registry
             (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
             VALUES (?1, ?2, '[]', ?3, ?4, ?5, ?6, 0, NULL, ?7)",
            rusqlite::params![Uuid::new_v4().to_string(), name, kinds_json, min, max, category, now],
        )?;
    }
    Ok(())
}

/// A real pool against the app's actual (temp-dir-backed) schema, migrations,
/// and seeded attribute registry — used by tests that need more than a
/// hand-written `CREATE TABLE` subset (e.g. narrator-tool staging, which
/// touches five-plus tables). Callers are responsible for creating their own
/// story rows.
#[cfg(test)]
pub fn test_pool() -> Pool {
    let dir = std::env::temp_dir().join(format!("dungeon-test-{}", Uuid::new_v4()));
    init_pool(&dir).expect("initialize test schema")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_needed_migration_backfills_once_without_touching_other_payloads() {
        fn payloads(conn: &rusqlite::Connection) -> Vec<(String, String)> {
            conn.prepare(
                "SELECT id, payload_json FROM ledger_entries
                 WHERE id IN ('old-roll', 'complete-roll', 'unrelated') ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        }

        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_roll_needed_v1'",
            [],
        )
        .unwrap();
        conn.execute_batch(
            r#"INSERT INTO stories (id, title, created_at, updated_at)
               VALUES ('s', 'Story', 'now', 'now');
               INSERT INTO ledger_entries
                   (id, story_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at)
               VALUES
                   ('base', 's', 0, 'narration', 'visible', 'Base', '{}', NULL, 'now'),
                   ('old-roll', 's', 1, 'diceroll', 'hidden', NULL,
                    '{"chance_percent":37,"roll":80,"outcome":"success","seed":1}', 'base', 'now'),
                   ('complete-roll', 's', 2, 'diceroll', 'hidden', NULL,
                    '{"chance_percent":60,"roll":10,"needed":7,"outcome":"failure","seed":2}', 'base', 'now'),
                   ('unrelated', 's', 3, 'entity_queried', 'hidden', NULL,
                    '{"chance_percent":25,"note":"unchanged"}', 'base', 'now');"#,
        )
        .unwrap();

        run_migrations(&mut conn).unwrap();

        let migrated = payloads(&conn);
        let payload = |id: &str| {
            serde_json::from_str::<serde_json::Value>(
                &migrated
                    .iter()
                    .find(|(entry_id, _)| entry_id == id)
                    .unwrap()
                    .1,
            )
            .unwrap()
        };
        assert_eq!(payload("old-roll")["needed"], 63);
        assert_eq!(payload("complete-roll")["needed"], 7);
        assert_eq!(
            migrated
                .iter()
                .find(|(entry_id, _)| entry_id == "complete-roll")
                .map(|(_, raw)| raw.as_str()),
            Some(r#"{"chance_percent":60,"roll":10,"needed":7,"outcome":"failure","seed":2}"#)
        );
        assert_eq!(
            payload("unrelated"),
            serde_json::json!({"chance_percent": 25, "note": "unchanged"})
        );
        assert_eq!(
            migrated
                .iter()
                .find(|(entry_id, _)| entry_id == "unrelated")
                .map(|(_, raw)| raw.as_str()),
            Some(r#"{"chance_percent":25,"note":"unchanged"}"#)
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'migration_roll_needed_v1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );

        run_migrations(&mut conn).unwrap();
        assert_eq!(payloads(&conn), migrated);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'migration_roll_needed_v1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn legacy_stories_reset_tools_once_and_new_stories_default_on() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_narrator_tools_stateless_roll_v1'",
            [],
        )
        .unwrap();
        for (id, settings) in [
            ("old", "{\"dice_mode\":\"never\",\"reasoning_effort\":\"high\",\"author_note\":\"remember\"}"),
            ("disabled", "{\"attributes_enabled\":false,\"narrator_tools\":{\"roll_check\":false}}"),
        ] {
            conn.execute(
                "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES (?1, 'story', 'now', 'now', ?2)",
                rusqlite::params![id, settings],
            )
            .unwrap();
        }
        run_migrations(&mut conn).unwrap();
        let updated = NarratorToolSettings {
            roll_check: false,
            ..NarratorToolSettings::default()
        };
        conn.execute(
            "UPDATE stories SET settings_json = ?1 WHERE id = 'old'",
            [serde_json::json!({
                "narrator_tools": updated, "reasoning_effort": "high", "author_note": "remember"
            })
            .to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('new', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        drop(conn);

        for (id, expected) in [
            ("old", updated),
            ("disabled", NarratorToolSettings::default()),
            ("new", NarratorToolSettings::default()),
        ] {
            assert_eq!(
                crate::features::stories::settings::read_story_narrator_tools(&pool, id).unwrap(),
                expected
            );
        }
        let conn = pool.get().unwrap();
        let old: String = conn
            .query_row(
                "SELECT settings_json FROM stories WHERE id = 'old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let old: serde_json::Value = serde_json::from_str(&old).unwrap();
        assert_eq!(old["reasoning_effort"], "high");
        assert_eq!(old["author_note"], "remember");
        assert!(old.get("dice_mode").is_none());
        let disabled: String = conn
            .query_row(
                "SELECT settings_json FROM stories WHERE id = 'disabled'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&disabled)
            .unwrap()
            .get("attributes_enabled")
            .is_none());
        for id in ["old", "disabled"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM story_entity_state WHERE story_id=?1 AND name='You'",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
    }

    #[test]
    fn migration_materializes_selected_edit_and_discards_legacy_rolls_without_repeating() {
        let dir = std::env::temp_dir().join(format!("dungeon-tool-migration-{}", Uuid::new_v4()));
        let pool = init_pool(&dir).unwrap();
        let conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_narrator_tools_stateless_roll_v1'",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES
             ('s','Story','now','now','{\"attributes_enabled\":false,\"reasoning_effort\":\"low\",\"extra\":9}');
             INSERT INTO ledger_entries (id,story_id,seq,kind,visibility,content,payload_json,target_entry_id,created_at) VALUES
             ('player','s',0,'player_message','visible','old player','{}',NULL,'now'),
             ('player-edit','s',1,'content_edited','hidden','edited player','{}','player','now'),
             ('base','s',2,'narration','visible','base text','{\"thoughts\":\"base thought\",\"input_mode\":\"do\"}',NULL,'now'),
             ('variant','s',3,'narration_variant','hidden','variant text','{\"thoughts\":\"variant thought\"}','base','now'),
             ('edited','s',4,'content_edited','hidden','visible edited text','{\"applies_to\":\"variant\"}','base','now'),
             ('selected','s',5,'narration_selected','hidden',NULL,'{\"selected_entry_id\":\"variant\"}','base','now'),
             ('roll','s',6,'diceroll','hidden',NULL,'{\"roll\":7}','base','now'),
             ('dice-setting','s',7,'diceroll_settings_changed','hidden',NULL,'{}',NULL,'now'),
             ('invalid-summary','s',8,'context_summary','hidden','old summary','{\"through_seq\":6,\"through_entry_id\":\"roll\"}',NULL,'now'),
             ('valid-summary','s',9,'context_summary','hidden','safe summary','{\"through_seq\":2,\"through_entry_id\":\"base\"}',NULL,'now');
             INSERT INTO settings (key,value) VALUES ('image_model_default','{\"model\":\"image-v1\",\"enabled\":true,\"style\":\"ink\",\"narrator_images\":false}');",
        ).unwrap();
        drop(conn);
        drop(pool);
        let pool = init_pool(&dir).unwrap();
        let conn = pool.get().unwrap();
        let (content, payload): (String, String) = conn
            .query_row(
                "SELECT content,payload_json FROM ledger_entries WHERE id='base'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(content, "visible edited text");
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["thoughts"], "variant thought");
        assert_eq!(payload["input_mode"], "do");
        let surviving: Vec<String> = conn
            .prepare("SELECT id FROM ledger_entries ORDER BY seq")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            &surviving[..4],
            ["player", "player-edit", "base", "valid-summary"]
        );
        assert_eq!(surviving.len(), 5);
        let (seed_kind, seed_payload): (String, String) = conn
            .query_row(
                "SELECT kind, payload_json FROM ledger_entries WHERE id = ?1",
                [&surviving[4]],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(seed_kind, kind::ENTITY_CREATED);
        let seed_payload: serde_json::Value = serde_json::from_str(&seed_payload).unwrap();
        assert_eq!(seed_payload["name"], "You");
        let image: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='image_model_default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let image: serde_json::Value = serde_json::from_str(&image).unwrap();
        assert_eq!(image["style"], "ink");
        assert!(image.get("narrator_images").is_none());
        assert!(!conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap());
        conn.execute("INSERT INTO ledger_entries (id,story_id,seq,kind,visibility,content,payload_json,target_entry_id,created_at) VALUES ('new-roll','s',(SELECT COALESCE(MAX(seq),-1)+1 FROM ledger_entries WHERE story_id='s'),'diceroll','hidden',NULL,'{\"chance_percent\":50}','base','now')", []).unwrap();
        drop(conn);
        drop(pool);
        let pool = init_pool(&dir).unwrap();
        let conn = pool.get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id='new-roll'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let you_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM story_entity_state WHERE story_id='s' AND name='You'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(you_count, 1);
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn seed_player_entity_respects_existing_case_insensitive_name_and_no_attributes() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id,title,created_at,updated_at) VALUES
             ('existing','Existing','now','now'), ('fresh','Fresh','now','now');
             INSERT INTO entities (id,story_id,kind,created_at) VALUES ('original','existing','character','now');
             INSERT INTO story_entity_state (story_id,entity_id,name,is_present,updated_at)
             VALUES ('existing','original','yOu',1,'now');",
        ).unwrap();
        for story_id in ["existing", "fresh"] {
            seed_player_entity(&conn, story_id).unwrap();
            seed_player_entity(&conn, story_id).unwrap();
            let (count, attributes): (i64, i64) = conn
                .query_row(
                    "SELECT COUNT(*), (SELECT COUNT(*) FROM entity_attributes WHERE story_id=?1)
                 FROM story_entity_state WHERE story_id=?1 AND name='You' COLLATE NOCASE",
                    [story_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!((count, attributes), (1, 0));
        }
        let original: String = conn.query_row(
            "SELECT entity_id FROM story_entity_state WHERE story_id='existing' AND name='You' COLLATE NOCASE",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(original, "original");
    }

    #[test]
    fn fresh_schema_contains_ledger_and_no_story_cards() {
        let dir = std::env::temp_dir().join(format!("dungeon-schema-{}", Uuid::new_v4()));
        let pool = init_pool(&dir).expect("initialize schema");
        let conn = pool.get().unwrap();
        let exists = |name: &str| -> bool {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(exists("ledger_entries"));
        assert!(exists("turns"));
        assert!(!exists("timeline_entries"));
        assert!(exists("story_entity_state"));
        assert!(!exists("branches"));
        assert!(exists("image_assets"));
        let image_columns = conn
            .prepare("PRAGMA table_info(image_assets)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            image_columns,
            ["id", "entry_id", "path", "prompt", "created_at"]
        );
        let ledger_columns = conn
            .prepare("PRAGMA table_info(ledger_entries)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(ledger_columns.contains(&"turn_id".to_string()));
        for index in ["idx_turns_one_pending", "idx_ledger_turn"] {
            assert!(conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                    [index],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap());
        }
        assert!(!exists("story_cards"));
        assert!(!exists("passages"));
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_ledger_table_gains_nullable_turn_reference_and_index() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE stories(id TEXT PRIMARY KEY);
             CREATE TABLE turns(id TEXT PRIMARY KEY, story_id TEXT REFERENCES stories(id));
             CREATE TABLE ledger_entries(
                 id TEXT PRIMARY KEY,
                 story_id TEXT NOT NULL REFERENCES stories(id),
                 target_entry_id TEXT REFERENCES ledger_entries(id)
             );",
        )
        .unwrap();
        ensure_ledger_turn_column(&conn).unwrap();
        let column: (String, i64) = conn
            .query_row(
                "SELECT name, \"notnull\" FROM pragma_table_info('ledger_entries') WHERE name = 'turn_id'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(column, ("turn_id".into(), 0));
        assert!(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_ledger_turn')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        let referenced_table: String = conn
            .query_row(
                "SELECT \"table\" FROM pragma_foreign_key_list('ledger_entries') WHERE \"from\" = 'turn_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(referenced_table, "turns");
    }

    #[test]
    fn existing_turns_table_gains_attempt_column_with_zero_default() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE turns(
                 id TEXT PRIMARY KEY,
                 story_id TEXT NOT NULL,
                 seq INTEGER NOT NULL,
                 status TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             INSERT INTO turns (id, story_id, seq, status, created_at)
             VALUES ('turn', 'story', 0, 'complete', 'now');",
        )
        .unwrap();

        ensure_turn_attempt_column(&conn).unwrap();

        let column: (String, i64, Option<String>) = conn
            .query_row(
                "SELECT name, \"notnull\", dflt_value
                 FROM pragma_table_info('turns') WHERE name = 'attempt'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(column, ("attempt".into(), 1, Some("0".into())));
        assert_eq!(
            conn.query_row("SELECT attempt FROM turns WHERE id = 'turn'", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn existing_ledger_data_and_foreign_keys_survive_upgrade() {
        let dir = std::env::temp_dir().join(format!("dungeon-ledger-upgrade-{}", Uuid::new_v4()));
        let pool = init_pool(&dir).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now');
             INSERT INTO ledger_entries (id, story_id, seq, kind, visibility, content, payload_json, created_at)
                 VALUES ('turn', 's', 0, 'player_message', 'visible', 'Look', '{\"input_mode\":\"do\"}', 'now');
             INSERT INTO ledger_entries (id, story_id, seq, kind, visibility, payload_json, target_entry_id, created_at)
                 VALUES ('roll', 's', 1, 'diceroll', 'hidden', '{\"roll\":7}', 'turn', 'now');
             INSERT INTO entities (id, story_id, kind, created_at) VALUES ('entity', 's', 'character', 'now');
             INSERT INTO story_entity_state (story_id, entity_id, name, is_present, updated_at, last_event_id)
                 VALUES ('s', 'entity', 'Hero', 1, 'now', 'roll');
             INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
                 VALUES ('image', 'turn', 'scene.png', 'A scene', 'now');
             ALTER TABLE ledger_entries RENAME TO timeline_entries;
             DROP INDEX idx_ledger_story_seq;
             DROP INDEX idx_ledger_target;
             DROP INDEX idx_ledger_kind;
             CREATE INDEX idx_timeline_story_seq ON timeline_entries(story_id, seq);
             CREATE INDEX idx_timeline_target ON timeline_entries(target_entry_id);
             CREATE INDEX idx_timeline_kind ON timeline_entries(story_id, kind, seq);
              INSERT INTO settings (key, value) VALUES ('timeline_retention', '{\"tool_call_persistence\":false}');
              DELETE FROM settings WHERE key = 'migration_turns_v1';",
        )
        .unwrap();
        let attribute_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Strength'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO entity_attributes
             (story_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
             VALUES ('s', 'entity', ?1, 5, 'user', 'now', 'roll')",
            [&attribute_id],
        )
        .unwrap();

        drop(conn);
        drop(pool);
        let pool = init_pool(&dir).unwrap();
        drop(pool);
        let pool = init_pool(&dir).unwrap();
        let conn = pool.get().unwrap();
        let migrated: (i64, String, String) = conn
            .query_row(
                "SELECT seq, target_entry_id, payload_json FROM ledger_entries WHERE id = 'roll'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(migrated, (1, "turn".into(), "{\"roll\":7}".into()));
        let turn_ownership: (String, String, String) = conn
            .query_row(
                "SELECT player.turn_id, roll.turn_id, turns.status
                 FROM ledger_entries AS player
                 JOIN ledger_entries AS roll ON roll.id = 'roll'
                 JOIN turns ON turns.id = player.turn_id
                 WHERE player.id = 'turn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(turn_ownership.0, turn_ownership.1);
        assert_eq!(turn_ownership.2, "failed");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ledger_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let old_table: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'timeline_entries')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!old_table);
        for table in [
            "ledger_entries",
            "story_entity_state",
            "entity_attributes",
            "image_assets",
        ] {
            let references = conn
                .prepare(&format!("PRAGMA foreign_key_list({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                references.contains(&"ledger_entries".to_string()),
                "{table}"
            );
        }
        assert!(!conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap());
        for (table, column) in [
            ("story_entity_state", "last_event_id"),
            ("entity_attributes", "last_event_id"),
            ("image_assets", "entry_id"),
        ] {
            let id: String = conn
                .query_row(&format!("SELECT {column} FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(matches!(id.as_str(), "roll" | "turn"));
        }
        let retention: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ledger_retention'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retention, "{\"tool_call_persistence\":false}");
        let old_setting: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'timeline_retention')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!old_setting);
        for index in [
            "idx_ledger_story_seq",
            "idx_ledger_target",
            "idx_ledger_kind",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "{index}");
        }
        conn.execute("DELETE FROM ledger_entries WHERE id = 'turn'", [])
            .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM ledger_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
        let images: i64 = conn
            .query_row("SELECT COUNT(*) FROM image_assets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(images, 0);
        for table in ["story_entity_state", "entity_attributes"] {
            let last_event_id: Option<String> = conn
                .query_row(&format!("SELECT last_event_id FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(last_event_id.is_none(), "{table}");
        }
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_ledger_schema_upgrade_rolls_back_the_rename() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute_batch(
            "ALTER TABLE ledger_entries RENAME TO timeline_entries;
             DROP INDEX idx_ledger_story_seq;
             DROP INDEX idx_ledger_target;
             DROP INDEX idx_ledger_kind;
             CREATE INDEX idx_ledger_story_seq ON stories(id);",
        )
        .unwrap();
        assert!(migrate_ledger_schema(&mut conn).is_err());
        let still_old: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'timeline_entries')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(still_old);
        conn.execute("DROP INDEX idx_ledger_story_seq", []).unwrap();
        run_migrations(&mut conn).unwrap();
        assert!(!conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap());
    }

    #[test]
    fn ambiguous_ledger_schema_is_not_overwritten() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE timeline_entries (id TEXT PRIMARY KEY);
             INSERT INTO timeline_entries (id) VALUES ('legacy');",
        )
        .unwrap();
        assert!(migrate_ledger_schema(&mut conn).is_err());
        let legacy: String = conn
            .query_row("SELECT id FROM timeline_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(legacy, "legacy");
    }

    #[test]
    fn legacy_retention_key_does_not_replace_newer_ledger_preference() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO settings (key, value) VALUES
                ('timeline_retention', '{\"tool_call_persistence\":false}'),
                ('ledger_retention', '{\"tool_call_persistence\":true}');",
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        let value: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ledger_retention'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "{\"tool_call_persistence\":true}");
    }

    #[test]
    fn legacy_note_and_memory_settings_migrate_once() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_author_notes_to_story_settings'",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_narrator_memory_split'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{\"dice_mode\":\"never\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_entries
             (id, story_id, seq, kind, visibility, content, payload_json, created_at)
             VALUES ('note', 's', 0, 'context_note_updated', 'hidden', NULL,
                     '{\"author_note\":\"  Keep it terse.  \"}', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('narrator_memory', ?1)",
            [serde_json::json!({
                "entity_context_mode": "none",
                "tool_call_persistence": false
            })
            .to_string()],
        )
        .unwrap();

        run_migrations(&mut conn).unwrap();

        let story_settings: String = conn
            .query_row(
                "SELECT settings_json FROM stories WHERE id = 's'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let story_settings: serde_json::Value = serde_json::from_str(&story_settings).unwrap();
        assert_eq!(story_settings["author_note"], "Keep it terse.");
        assert_eq!(story_settings["dice_mode"], "never");
        let context: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'context_injection'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&context).unwrap()["entity_context_mode"],
            "none"
        );
        let retention: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ledger_retention'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&retention).unwrap()["tool_call_persistence"],
            false
        );
        let legacy_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'narrator_memory')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!legacy_exists);
        let migrated: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'migration_narrator_memory_split')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(migrated);

        conn.execute("UPDATE stories SET settings_json = '{}' WHERE id = 's'", [])
            .unwrap();
        run_migrations(&mut conn).unwrap();
        let story_settings: String = conn
            .query_row(
                "SELECT settings_json FROM stories WHERE id = 's'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&story_settings)
            .unwrap()
            .get("author_note")
            .is_none());
        assert!(!conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'narrator_memory')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    }

    #[test]
    fn empty_narrator_memory_migration_is_recorded() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        let has_marker = |conn: &PooledConn| -> bool {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'migration_narrator_memory_split')",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert!(has_marker(&conn));
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_narrator_memory_split'",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        assert!(has_marker(&conn));
    }

    #[test]
    fn empty_author_note_migration_is_recorded() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_author_notes_to_story_settings'",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        let migrated: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'migration_author_notes_to_story_settings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(migrated);
    }

    #[test]
    fn legacy_memory_imports_once_without_overwriting_newer_preferences() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_narrator_memory_split'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('context_injection', ?1)",
            [r#"{"entity_context_mode":"scoped","dice_rolls_in_context":false}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ledger_retention', ?1)",
            [r#"{"tool_call_persistence":true}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('narrator_memory', ?1)",
            [r#"{"entity_context_mode":"none","tool_call_persistence":false}"#],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        let context: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'context_injection'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let context: serde_json::Value = serde_json::from_str(&context).unwrap();
        assert_eq!(context["entity_context_mode"], "none");
        assert_eq!(context["dice_rolls_in_context"], false);
        let retention: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ledger_retention'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&retention).unwrap()["tool_call_persistence"],
            false
        );
        assert!(!conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'narrator_memory')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());

        conn.execute(
            "UPDATE settings SET value = '{\"entity_context_mode\":\"scoped\",\"dice_rolls_in_context\":false}' WHERE key = 'context_injection'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE settings SET value = '{\"tool_call_persistence\":true}' WHERE key = 'ledger_retention'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('narrator_memory', '{\"entity_context_mode\":\"none\",\"tool_call_persistence\":false}')",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        let context: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'context_injection'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&context).unwrap(),
            serde_json::json!({"entity_context_mode": "scoped", "dice_rolls_in_context": false})
        );
        let retention: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ledger_retention'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&retention).unwrap()["tool_call_persistence"],
            true
        );
        assert!(!conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'narrator_memory')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    }

    #[test]
    fn canonical_name_uniqueness_is_enforced_case_insensitively_across_connections() {
        let pool = test_pool();
        let first = pool.get().unwrap();
        let second = pool.get().unwrap();
        let absent = |conn: &PooledConn, name: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM attribute_registry WHERE canonical_name = ?1 COLLATE NOCASE",
                [name],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(absent(&first, "Resonance"), 0);
        assert_eq!(absent(&second, "resonance"), 0);

        first
            .execute(
                "INSERT INTO attribute_registry
                 (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
                 VALUES ('first', 'Resonance', '[]', '[\"artifact\"]', 0, 10, 'misc', 1, NULL, '2026-01-01')",
                [],
            )
            .unwrap();
        let error = second
            .execute(
                "INSERT INTO attribute_registry
                 (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
                 VALUES ('second', 'resonance', '[]', '[\"artifact\"]', 0, 10, 'misc', 1, NULL, '2026-01-02')",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::ConstraintViolation,
                    ..
                },
                _
            )
        ));
    }

    #[test]
    fn entity_names_are_unique_case_insensitively_per_story() {
        let pool = test_pool();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
                 VALUES ('first', 'First', 'now', 'now', '{}'),
                        ('second', 'Second', 'now', 'now', '{}');
             INSERT INTO entities (id, story_id, kind, created_at)
                 VALUES ('one', 'first', 'character', 'now'),
                        ('two', 'first', 'character', 'now'),
                        ('three', 'second', 'character', 'now');
             INSERT INTO story_entity_state
                 (story_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
                 VALUES ('first', 'one', 'Mira', NULL, 1, 'now', NULL);",
        )
        .unwrap();

        let error = conn
            .execute(
                "INSERT INTO story_entity_state
                 (story_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
                 VALUES ('first', 'two', 'mira', NULL, 1, 'now', NULL)",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::ConstraintViolation,
                    ..
                },
                _
            )
        ));
        conn.execute(
            "INSERT INTO story_entity_state
             (story_id, entity_id, name, appearance_anchor, is_present, updated_at, last_event_id)
             VALUES ('second', 'three', 'mira', NULL, 1, 'now', NULL)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn turns_backfill_assigns_exact_ownership_and_status_once() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute_batch(
            r#"INSERT INTO stories (id, title, created_at, updated_at, settings_json)
                   VALUES ('s', 'Story', 'now', 'now', '{}');
               DELETE FROM settings WHERE key = 'migration_turns_v1';
               INSERT INTO ledger_entries
                   (id, story_id, seq, kind, visibility, content, payload_json, target_entry_id, turn_id, created_at)
               VALUES
                   ('p1','s',0,'player_message','visible','Act','{"input_mode":"do"}',NULL,NULL,'t0'),
                   ('roll1','s',1,'diceroll','hidden',NULL,'{}','p1',NULL,'t1'),
                   ('n1','s',2,'narration','visible','Result','{}',NULL,NULL,'t2'),
                   ('p2','s',3,'player_message','visible','','{"input_mode":"see"}',NULL,NULL,'t3'),
                   ('image2','s',4,'image_generated','hidden',NULL,'{"asset_id":"a"}','p2',NULL,'t4'),
                   ('n2','s',5,'narration','visible','Separate','{}',NULL,NULL,'t5'),
                   ('p3','s',6,'player_message','visible','','{"input_mode":"see"}',NULL,NULL,'t6'),
                   ('n3','s',7,'narration','visible','Authored','{"input_mode":"story"}',NULL,NULL,'t7'),
                   ('edit3','s',8,'content_edited','hidden','Edit','{}','n3',NULL,'t8'),
                   ('untargeted','s',9,'entity_updated','hidden','UI edit','{"entity_id":"entity","before":{"name":"Before"},"after":{"name":"After"},"source":"user"}',NULL,NULL,'t9'),
                   ('note','s',10,'context_note_updated','hidden',NULL,'{}',NULL,NULL,'t10'),
                   ('p4','s',11,'player_message','visible','Act','{"input_mode":"do"}',NULL,NULL,'t11'),
                   ('n4','s',12,'narration','visible','Default generated','{}',NULL,NULL,'t12');"#,
        )
        .unwrap();

        migrate_turns_v1(&mut conn).unwrap();
        let rows = {
            let mut stmt = conn
                .prepare(
                    "SELECT ledger_entries.id, ledger_entries.turn_id, turns.seq, turns.status
                     FROM ledger_entries LEFT JOIN turns ON turns.id = ledger_entries.turn_id
                     WHERE ledger_entries.story_id = 's' ORDER BY ledger_entries.seq",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        let by_id = rows
            .iter()
            .map(|(id, turn, seq, status)| {
                (id.as_str(), (turn.as_deref(), *seq, status.as_deref()))
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(by_id["p1"].0, by_id["roll1"].0);
        assert_eq!(by_id["p1"].0, by_id["n1"].0);
        assert_eq!(by_id["p1"].1, Some(0));
        assert_eq!(by_id["p1"].2, Some("complete"));
        assert_eq!(by_id["p2"].0, by_id["image2"].0);
        assert_eq!(by_id["p2"].1, Some(1));
        assert_eq!(by_id["p2"].2, Some("complete"));
        assert_ne!(by_id["p2"].0, by_id["n2"].0);
        assert_eq!(by_id["n2"].1, Some(2));
        assert_eq!(by_id["p3"].1, Some(3));
        assert_eq!(by_id["p3"].2, Some("failed"));
        assert_ne!(by_id["p3"].0, by_id["n3"].0);
        assert_eq!(by_id["n3"].0, by_id["edit3"].0);
        assert_eq!(by_id["n3"].1, Some(4));
        assert_eq!(by_id["untargeted"], (None, None, None));
        assert_eq!(by_id["note"], (None, None, None));
        assert_eq!(by_id["p4"].0, by_id["n4"].0);
        assert_eq!(by_id["p4"].1, Some(5));

        let before: Vec<(String, Option<String>)> = rows
            .iter()
            .map(|(id, turn, _, _)| (id.clone(), turn.clone()))
            .collect();
        migrate_turns_v1(&mut conn).unwrap();
        let after = conn
            .prepare("SELECT id, turn_id FROM ledger_entries WHERE story_id = 's' ORDER BY seq")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<(String, Option<String>)>, _>>()
            .unwrap();
        assert_eq!(after, before);
        assert!(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'migration_turns_v1')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM turns WHERE story_id = 's'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            6
        );
    }

    #[test]
    fn startup_recovers_pending_turns_as_failed() {
        let dir = std::env::temp_dir().join(format!("story-llm-recovery-{}", Uuid::new_v4()));
        let pool = init_pool(&dir).unwrap();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = crate::features::ledger::turns::create_turn(&conn, "s").unwrap();
        drop(conn);
        drop(pool);

        let pool = init_pool(&dir).unwrap();
        let status: String = pool
            .get()
            .unwrap()
            .query_row("SELECT status FROM turns WHERE id = ?1", [turn_id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "failed");
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn with_transaction_commits_success_and_rolls_back_errors() {
        let pool = test_pool();
        with_transaction(&pool, |tx| {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES ('kept', 'yes')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let result: AppResult<()> = with_transaction(&pool, |tx| {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES ('rolled_back', 'yes')",
                [],
            )?;
            Err(AppError::Other("stop".into()))
        });
        assert!(result.is_err());

        let conn = pool.get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key IN ('kept', 'rolled_back')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
