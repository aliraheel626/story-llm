use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

pub type Pool = r2d2::Pool<SqliteConnectionManager>;
pub type PooledConn = r2d2::PooledConnection<SqliteConnectionManager>;

const SCHEMA_VERSION: i64 = 2;

pub fn init_pool(app_data_dir: &Path) -> AppResult<Pool> {
    std::fs::create_dir_all(app_data_dir)?;
    let db_path = app_data_dir.join("dungeon.sqlite3");
    let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(())
    });
    let pool = r2d2::Pool::new(manager).map_err(AppError::Pool)?;
    let conn = pool.get().map_err(AppError::Pool)?;
    run_migrations(&conn, app_data_dir)?;
    seed_attribute_registry(&conn)?;
    Ok(pool)
}

fn run_migrations(conn: &PooledConn, app_data_dir: &Path) -> AppResult<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let has_legacy: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'passages')",
        [],
        |row| row.get(0),
    )?;

    if version < SCHEMA_VERSION && has_legacy {
        // Timeline v2 intentionally resets story data. Global model settings
        // and Tauri's secure store (API keys) are kept.
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = OFF;
            BEGIN IMMEDIATE;
            DROP TABLE IF EXISTS card_activations;
            DROP TABLE IF EXISTS story_cards;
            DROP TABLE IF EXISTS rolls;
            DROP TABLE IF EXISTS attribute_events;
            DROP TABLE IF EXISTS entity_attributes;
            DROP TABLE IF EXISTS attribute_registry;
            DROP TABLE IF EXISTS images;
            DROP TABLE IF EXISTS passage_variants;
            DROP TABLE IF EXISTS passages;
            DROP TABLE IF EXISTS entities;
            DROP TABLE IF EXISTS branches;
            DROP TABLE IF EXISTS stories;
            DROP TABLE IF EXISTS meter_events;
            DROP TABLE IF EXISTS entity_meters;
            DROP TABLE IF EXISTS meter_registry;
            COMMIT;
            PRAGMA foreign_keys = ON;
            "#,
        )?;
        let images_dir = app_data_dir.join("images");
        if images_dir.is_dir() {
            std::fs::remove_dir_all(images_dir)?;
        }
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS stories (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            settings_json TEXT NOT NULL DEFAULT '{}',
            default_branch_id TEXT
        );

        CREATE TABLE IF NOT EXISTS branches (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            parent_branch_id TEXT REFERENCES branches(id) ON DELETE SET NULL,
            forked_at_entry_id TEXT,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS timeline_entries (
            id TEXT PRIMARY KEY,
            branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            visibility TEXT NOT NULL CHECK (visibility IN ('visible', 'hidden')),
            content TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            target_entry_id TEXT REFERENCES timeline_entries(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            UNIQUE(branch_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_timeline_branch_seq ON timeline_entries(branch_id, seq);
        CREATE INDEX IF NOT EXISTS idx_timeline_target ON timeline_entries(target_entry_id);
        CREATE INDEX IF NOT EXISTS idx_timeline_kind ON timeline_entries(branch_id, kind, seq);

        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS branch_entity_state (
            branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            appearance_anchor TEXT,
            is_present INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL,
            last_event_id TEXT REFERENCES timeline_entries(id) ON DELETE SET NULL,
            PRIMARY KEY (branch_id, entity_id)
        );
        CREATE INDEX IF NOT EXISTS idx_branch_entities_name ON branch_entity_state(branch_id, name);

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

        CREATE TABLE IF NOT EXISTS entity_attributes (
            branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            attribute_id TEXT NOT NULL REFERENCES attribute_registry(id) ON DELETE CASCADE,
            value REAL NOT NULL,
            source TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_event_id TEXT REFERENCES timeline_entries(id) ON DELETE SET NULL,
            PRIMARY KEY (branch_id, entity_id, attribute_id)
        );

        CREATE TABLE IF NOT EXISTS image_assets (
            id TEXT PRIMARY KEY,
            entry_id TEXT NOT NULL REFERENCES timeline_entries(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            prompt TEXT NOT NULL,
            seed INTEGER,
            provider TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_image_assets_entry ON image_assets(entry_id);

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        PRAGMA user_version = 2;
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
        assert!(exists("branch_entity_state"));
        assert!(exists("image_assets"));
        assert!(!exists("story_cards"));
        assert!(!exists("passages"));
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }
}
