use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OptionalExtension;
use std::path::Path;
use uuid::Uuid;

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

        CREATE TABLE IF NOT EXISTS ledger_entries (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            visibility TEXT NOT NULL CHECK (visibility IN ('visible', 'hidden')),
            content TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            target_entry_id TEXT REFERENCES ledger_entries(id) ON DELETE CASCADE,
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
    migrate_ledger_retention_settings(conn)?;
    migrate_existing_story_dice_settings(conn)?;
    migrate_narrator_memory_settings(conn)?;
    migrate_author_notes(conn)?;
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

fn migrate_existing_story_dice_settings(conn: &mut rusqlite::Connection) -> AppResult<()> {
    const MIGRATION_KEY: &str = "migration_existing_story_dice_settings";
    let migrated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [MIGRATION_KEY],
        |row| row.get(0),
    )?;
    if migrated {
        return Ok(());
    }
    let stories = {
        let mut stmt = conn.prepare("SELECT id, settings_json FROM stories")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let tx = conn.transaction()?;
    for (story_id, raw) in stories {
        let mut settings = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({}));
        if !settings.is_object() {
            settings = serde_json::json!({});
        }
        if settings.get("attributes_enabled").is_some() {
            continue;
        }
        settings["attributes_enabled"] = serde_json::json!(true);
        tx.execute(
            "UPDATE stories SET settings_json = ?1 WHERE id = ?2",
            rusqlite::params![settings.to_string(), story_id],
        )?;
    }
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'true')",
        [MIGRATION_KEY],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_narrator_memory_settings(conn: &rusqlite::Connection) -> AppResult<()> {
    let legacy: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'narrator_memory'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(legacy) = legacy else {
        return Ok(());
    };
    let legacy = serde_json::from_str::<serde_json::Value>(&legacy)
        .unwrap_or_else(|_| serde_json::json!({}));
    let entity_context_mode = legacy
        .get("entity_context_mode")
        .and_then(serde_json::Value::as_str)
        .filter(|mode| matches!(*mode, "all" | "scoped" | "none"))
        .unwrap_or("all");
    let tool_call_persistence = legacy
        .get("tool_call_persistence")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let context: Option<String> = conn
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
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('context_injection', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [context.to_string()],
        )?;
    }
    let retention: Option<String> = conn
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
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ledger_retention', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::json!({"tool_call_persistence": tool_call_persistence}).to_string()],
        )?;
    }
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
    fn old_stories_keep_dice_tools_and_new_stories_default_off() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = 'migration_existing_story_dice_settings'",
            [],
        )
        .unwrap();
        for (id, settings) in [
            ("old", "{}"),
            ("disabled", "{\"attributes_enabled\":false}"),
        ] {
            conn.execute(
                "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES (?1, 'story', 'now', 'now', ?2)",
                rusqlite::params![id, settings],
            )
            .unwrap();
        }
        run_migrations(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('new', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        drop(conn);

        for (id, enabled) in [("old", true), ("disabled", false), ("new", false)] {
            assert_eq!(
                crate::features::dicerolls::commands::read_story_diceroll_settings(&pool, id)
                    .unwrap()
                    .attributes_enabled,
                enabled
            );
        }
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
        assert!(!exists("story_cards"));
        assert!(!exists("passages"));
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
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
             INSERT INTO settings (key, value) VALUES ('timeline_retention', '{\"tool_call_persistence\":false}');",
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
        let legacy: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'narrator_memory'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let legacy: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        assert_eq!(legacy["entity_context_mode"], "none");
        assert_eq!(legacy["tool_call_persistence"], false);

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
    fn reupgrade_imports_preferences_changed_by_older_app() {
        let pool = test_pool();
        let mut conn = pool.get().unwrap();
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
