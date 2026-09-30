use r2d2_sqlite::SqliteConnectionManager;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Runs blocking database work on Tauri's blocking thread pool. Writes can wait
/// here for an open turn transaction without freezing the window or the async runtime.
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| AppError::Other(format!("database task failed: {error}")))?
}

pub fn database_path(pool: &Pool) -> AppResult<PathBuf> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare("PRAGMA database_list")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    rows.collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find_map(|(name, path)| (name == "main").then(|| PathBuf::from(path)))
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| AppError::Other("live database has no file path".into()))
}

pub fn open_connection(pool: &Pool) -> AppResult<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(database_path(pool)?)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 300000;",
    )?;
    Ok(conn)
}

pub fn init_pool(app_data_dir: &Path) -> AppResult<Pool> {
    fs::create_dir_all(app_data_dir)?;
    let db_path = app_data_dir.join("story-llm.sqlite3");
    let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
        // Other writes wait for the generating turn's SQLite write lock.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 300000;",
        )?;
        Ok(())
    });
    let pool = r2d2::Pool::new(manager).map_err(AppError::Pool)?;
    let mut conn = pool.get().map_err(AppError::Pool)?;
    create_schema(&mut conn)?;
    seed_attribute_registry(&conn)?;
    Ok(pool)
}

/// Creates the current schema on a fresh database. There are no upgrade steps:
/// a database from an older build is deleted, not migrated.
fn create_schema(conn: &mut PooledConn) -> AppResult<()> {
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
        CREATE TABLE IF NOT EXISTS transcript_entries (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            visibility TEXT NOT NULL CHECK (visibility IN ('visible', 'hidden')),
            content TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            target_entry_id TEXT REFERENCES transcript_entries(id) ON DELETE CASCADE,
            turn_id TEXT REFERENCES turns(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            UNIQUE(story_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_transcript_story_seq ON transcript_entries(story_id, seq);
        CREATE INDEX IF NOT EXISTS idx_transcript_target ON transcript_entries(target_entry_id);
        CREATE INDEX IF NOT EXISTS idx_transcript_kind ON transcript_entries(story_id, kind, seq);
        CREATE INDEX IF NOT EXISTS idx_transcript_turn ON transcript_entries(turn_id);

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
            last_event_id TEXT REFERENCES transcript_entries(id) ON DELETE SET NULL,
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
            last_event_id TEXT REFERENCES transcript_entries(id) ON DELETE SET NULL,
            PRIMARY KEY (story_id, entity_id, attribute_id)
        );

        CREATE TABLE IF NOT EXISTS image_assets (
            id TEXT PRIMARY KEY,
            entry_id TEXT NOT NULL REFERENCES transcript_entries(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            prompt TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_image_assets_entry ON image_assets(entry_id);

        CREATE TABLE IF NOT EXISTS image_blobs (
            asset_id TEXT PRIMARY KEY REFERENCES image_assets(id) ON DELETE CASCADE,
            media_type TEXT NOT NULL,
            bytes BLOB NOT NULL
        );

        CREATE TABLE IF NOT EXISTS usage_records (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK (kind IN ('narration', 'summary', 'title', 'image', 'caption')),
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            response_id TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cached_input_tokens INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL,
            created_at TEXT NOT NULL,
            turn_id TEXT,
            image_asset_id TEXT,
            duration_ms INTEGER,
            earlier_attempt INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_usage_story ON usage_records(story_id);
        CREATE INDEX IF NOT EXISTS idx_usage_turn ON usage_records(turn_id);

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;
    let auto_vacuum: i64 = conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
    if auto_vacuum != 2 {
        conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?;
    }
    conn.execute_batch("PRAGMA incremental_vacuum;")?;
    Ok(())
}

/// Idempotent on name: story creation calls this inside its own transaction.
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
        None,
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

/// A real pool against the app's actual (temp-dir-backed) schema and seeded
/// attribute registry — used by tests that need more than a
/// hand-written `CREATE TABLE` subset (e.g. narrator-tool fixtures, which
/// touches five-plus tables). Callers are responsible for creating their own
/// story rows.
#[cfg(test)]
pub fn test_pool() -> Pool {
    let dir = std::env::temp_dir().join(format!("story-llm-test-{}", Uuid::new_v4()));
    init_pool(&dir).expect("initialize test schema")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbouring_dungeon_app_data_is_never_imported() {
        for stray_files in [false, true] {
            let parent = std::env::temp_dir().join(format!("story-llm-no-import-{}", Uuid::new_v4()));
            let legacy = legacy_app_db(&parent);
            let old_dir = parent.join("com.dungeon.app");
            fs::create_dir_all(old_dir.join("images")).unwrap();
            fs::write(old_dir.join("images").join("scene.png"), b"old image").unwrap();
            fs::write(old_dir.join("secrets.json"), b"{\"placeholder\":\"old\"}").unwrap();
            drop(legacy);
            let new_dir = parent.join("com.story-llm.app");
            if stray_files {
                fs::create_dir_all(&new_dir).unwrap();
                fs::write(new_dir.join("secrets.json"), b"{\"placeholder\":\"new\"}").unwrap();
                fs::write(new_dir.join("notes.txt"), b"keep this").unwrap();
            }
            let pool = init_pool(&new_dir).unwrap();
            let conn = pool.get().unwrap();
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM stories", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'transcript_entries'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'ledger_entries'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
            assert!(!new_dir.join("images").exists());
            if stray_files {
                assert_eq!(fs::read(new_dir.join("secrets.json")).unwrap(), b"{\"placeholder\":\"new\"}");
                assert_eq!(fs::read(new_dir.join("notes.txt")).unwrap(), b"keep this");
            } else {
                assert!(!new_dir.join("secrets.json").exists());
                assert!(fs::read_dir(&new_dir).unwrap().all(|entry| entry.unwrap().file_name().to_string_lossy().starts_with("story-llm.sqlite3")));
            }
            drop(conn);
            drop(pool);
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn blocking_returns_work_value() {
        assert_eq!(blocking(|| Ok(42)).await.unwrap(), 42);
    }

    #[tokio::test]
    async fn blocking_preserves_work_error() {
        assert!(matches!(
            blocking::<()>(|| Err(AppError::Invalid("invalid test write".into()))).await,
            Err(AppError::Invalid(message)) if message == "invalid test write"
        ));
    }

    #[tokio::test]
    async fn blocking_write_finishes_after_turn_commit() {
        use crate::features::turn::{TurnGate, TurnTx};

        let pool = test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let writer_pool = pool.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut writer = tokio::spawn(async move {
            blocking(move || {
                let _ = started_tx.send(());
                with_transaction(&writer_pool, |tx| {
                    tx.execute(
                        "INSERT INTO settings (key, value) VALUES ('queued', 'saved')",
                        [],
                    )?;
                    Ok(())
                })
            })
            .await
        });
        started_rx.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(75), &mut writer)
                .await
                .is_err()
        );
        turn.commit().await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT value FROM settings WHERE key = 'queued'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "saved",
        );
    }

    fn legacy_app_db(parent: &Path) -> rusqlite::Connection {
        let legacy_dir = parent.join("com.dungeon.app");
        fs::create_dir_all(&legacy_dir).unwrap();
        let conn = rusqlite::Connection::open(legacy_dir.join("dungeon.sqlite3")).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA wal_autocheckpoint = 0;
             CREATE TABLE stories (
                 id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL, settings_json TEXT NOT NULL DEFAULT '{}'
             );
             INSERT INTO stories (id, title, created_at, updated_at)
                 VALUES ('legacy', 'Old story', 'now', 'now');",
        )
        .unwrap();
        conn
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
    fn fresh_database_has_transcript_table() {
        let dir = std::env::temp_dir().join(format!("story-llm-schema-{}", Uuid::new_v4()));
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
        assert!(exists("transcript_entries"));
        assert!(!exists("ledger_entries"));
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
        let transcript_columns = conn
            .prepare("PRAGMA table_info(transcript_entries)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(transcript_columns.contains(&"turn_id".to_string()));
        let usage_columns = conn
            .prepare("PRAGMA table_info(usage_records)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(&usage_columns[usage_columns.len() - 4..], ["turn_id", "image_asset_id", "duration_ms", "earlier_attempt"]);
        assert!(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_transcript_turn')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        assert_eq!(conn.query_row("PRAGMA auto_vacuum", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM settings WHERE key LIKE 'migration%'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert!(!exists("story_cards"));
        assert!(!exists("passages"));
        let tables: String = conn.query_row(
            "SELECT group_concat(name, '|') FROM (SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name)",
            [], |row| row.get(0),
        ).unwrap();
        drop(conn);
        drop(pool);
        let reopened = init_pool(&dir).unwrap();
        let reopened_tables: String = reopened.get().unwrap().query_row(
            "SELECT group_concat(name, '|') FROM (SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name)",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(reopened_tables, tables);
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
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
