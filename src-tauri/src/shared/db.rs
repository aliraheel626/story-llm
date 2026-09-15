use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

pub type Pool = r2d2::Pool<SqliteConnectionManager>;
pub type PooledConn = r2d2::PooledConnection<SqliteConnectionManager>;

pub fn init_pool(app_data_dir: &Path) -> AppResult<Pool> {
    std::fs::create_dir_all(app_data_dir)?;
    let db_path = app_data_dir.join("dungeon.sqlite3");
    let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(())
    });
    let pool = r2d2::Pool::new(manager).map_err(AppError::Pool)?;
    let conn = pool.get().map_err(AppError::Pool)?;
    run_migrations(&conn)?;
    seed_attribute_registry(&conn)?;
    Ok(pool)
}

fn run_migrations(conn: &PooledConn) -> AppResult<()> {
    // One-time cleanup: these were briefly named "meter_*" before the app's
    // terminology settled on "attribute", and were never populated. Safe
    // no-ops on every run after the first (DROP TABLE IF EXISTS).
    conn.execute_batch(
        r#"
        DROP TABLE IF EXISTS meter_events;
        DROP TABLE IF EXISTS entity_meters;
        DROP TABLE IF EXISTS meter_registry;
        "#,
    )?;

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
            forked_at_passage_id TEXT,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS passages (
            id TEXT PRIMARY KEY,
            branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            role TEXT NOT NULL,
            input_mode TEXT NOT NULL,
            content TEXT NOT NULL,
            thoughts TEXT,
            created_at TEXT NOT NULL,
            edited_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_passages_branch_seq ON passages(branch_id, seq);

        CREATE TABLE IF NOT EXISTS passage_variants (
            id TEXT PRIMARY KEY,
            passage_id TEXT NOT NULL REFERENCES passages(id) ON DELETE CASCADE,
            content TEXT NOT NULL,
            is_selected INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS images (
            id TEXT PRIMARY KEY,
            passage_id TEXT NOT NULL REFERENCES passages(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            prompt TEXT NOT NULL,
            seed INTEGER,
            provider TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            card_json TEXT NOT NULL DEFAULT '{}',
            appearance_anchor TEXT,
            created_at TEXT NOT NULL
        );

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
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            attribute_id TEXT NOT NULL REFERENCES attribute_registry(id) ON DELETE CASCADE,
            value REAL NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (entity_id, attribute_id)
        );

        CREATE TABLE IF NOT EXISTS attribute_events (
            id TEXT PRIMARY KEY,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            attribute_id TEXT NOT NULL REFERENCES attribute_registry(id) ON DELETE CASCADE,
            before REAL NOT NULL,
            after REAL NOT NULL,
            delta REAL NOT NULL,
            cause TEXT NOT NULL,
            passage_id TEXT REFERENCES passages(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_attribute_events_entity ON attribute_events(entity_id, attribute_id);

        CREATE TABLE IF NOT EXISTS rolls (
            id TEXT PRIMARY KEY,
            passage_id TEXT NOT NULL REFERENCES passages(id) ON DELETE CASCADE,
            actor_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_entity_id TEXT REFERENCES entities(id) ON DELETE SET NULL,
            actor_attribute_id TEXT REFERENCES attribute_registry(id) ON DELETE SET NULL,
            target_attribute_id TEXT REFERENCES attribute_registry(id) ON DELETE SET NULL,
            actor_value REAL,
            target_value REAL,
            p_success REAL NOT NULL,
            seed INTEGER NOT NULL,
            roll INTEGER NOT NULL,
            outcome TEXT NOT NULL,
            degree TEXT NOT NULL,
            modifiers_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS story_cards (
            id TEXT PRIMARY KEY,
            story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            keys_json TEXT NOT NULL DEFAULT '[]',
            content TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS card_activations (
            id TEXT PRIMARY KEY,
            passage_id TEXT NOT NULL REFERENCES passages(id) ON DELETE CASCADE,
            card_id TEXT NOT NULL REFERENCES story_cards(id) ON DELETE CASCADE,
            trigger_key TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;

    // Added after `rolls` already shipped: `CREATE TABLE IF NOT EXISTS` above
    // won't retrofit an existing table, so an existing database needs an
    // explicit ALTER TABLE. A brand-new database already gets these columns
    // from the CREATE TABLE statement, so this is a no-op there.
    add_column_if_missing(conn, "rolls", "actor_value", "REAL")?;
    add_column_if_missing(conn, "rolls", "target_value", "REAL")?;

    Ok(())
}

fn add_column_if_missing(
    conn: &PooledConn,
    table: &str,
    column: &str,
    decl_type: &str,
) -> AppResult<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let existing: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !existing.iter().any(|c| c == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {decl_type}"),
            [],
        )?;
    }
    Ok(())
}

/// Starter attribute vocabulary (spec §5.2: "~10 per entity kind"), seeded
/// once so the classifier has a registry to match against from the first
/// turn instead of minting everything as user-created. `INSERT OR IGNORE`
/// against the `canonical_name` UNIQUE constraint makes this idempotent.
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
        let id = Uuid::new_v4().to_string();
        let kinds_json = serde_json::to_string(kinds).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT OR IGNORE INTO attribute_registry
             (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
             VALUES (?1, ?2, '[]', ?3, ?4, ?5, ?6, 0, NULL, ?7)",
            rusqlite::params![id, name, kinds_json, min, max, category, now],
        )?;
    }
    Ok(())
}
