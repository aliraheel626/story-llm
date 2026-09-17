use r2d2_sqlite::SqliteConnectionManager;
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
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS stories (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            settings_json TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE IF NOT EXISTS timeline_entries (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            visibility TEXT NOT NULL CHECK (visibility IN ('visible', 'hidden')),
            content TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            target_entry_id TEXT REFERENCES timeline_entries(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            UNIQUE(story_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_timeline_story_seq ON timeline_entries(story_id, seq);
        CREATE INDEX IF NOT EXISTS idx_timeline_target ON timeline_entries(target_entry_id);
        CREATE INDEX IF NOT EXISTS idx_timeline_kind ON timeline_entries(story_id, kind, seq);

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
            last_event_id TEXT REFERENCES timeline_entries(id) ON DELETE SET NULL,
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
            last_event_id TEXT REFERENCES timeline_entries(id) ON DELETE SET NULL,
            PRIMARY KEY (story_id, entity_id, attribute_id)
        );

        CREATE TABLE IF NOT EXISTS image_assets (
            id TEXT PRIMARY KEY,
            entry_id TEXT NOT NULL REFERENCES timeline_entries(id) ON DELETE CASCADE,
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
    fn fresh_schema_contains_timeline_and_no_story_cards() {
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
        assert!(exists("timeline_entries"));
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
            .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
