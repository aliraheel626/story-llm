use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OptionalExtension;
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

pub type Pool = r2d2::Pool<SqliteConnectionManager>;
pub type PooledConn = r2d2::PooledConnection<SqliteConnectionManager>;

const SCHEMA_VERSION: i64 = 3;

pub fn init_pool(app_data_dir: &Path) -> AppResult<Pool> {
    std::fs::create_dir_all(app_data_dir)?;
    let db_path = app_data_dir.join("dungeon.sqlite3");
    let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(())
    });
    let pool = r2d2::Pool::new(manager).map_err(AppError::Pool)?;
    let mut conn = pool.get().map_err(AppError::Pool)?;
    run_migrations(&mut conn, app_data_dir)?;
    seed_attribute_registry(&conn)?;
    Ok(pool)
}

fn run_migrations(conn: &mut PooledConn, app_data_dir: &Path) -> AppResult<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let has_legacy: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'passages')",
        [],
        |row| row.get(0),
    )?;

    if version < 2 && has_legacy {
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
        // Best-effort: the schema reset above already committed, so a locked
        // or permission-denied file here must not abort startup — the app
        // would be unable to launch with the story tables already gone.
        let images_dir = app_data_dir.join("images");
        if images_dir.is_dir() {
            let _ = std::fs::remove_dir_all(images_dir);
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
        "#,
    )?;

    if version < 3 {
        migrate_attribute_registry_v3(conn)?;
    }
    conn.execute_batch(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS idx_attribute_registry_canonical_ci
            ON attribute_registry(canonical_name COLLATE NOCASE);
        "#,
    )?;
    Ok(())
}

#[derive(Debug)]
struct RegistryMigrationRow {
    id: String,
    canonical_name: String,
    aliases: Vec<String>,
    entity_kinds: Vec<String>,
    created_at: String,
}

fn contains_ci(values: &[String], needle: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(needle))
}

fn push_unique_ci(values: &mut Vec<String>, value: &str) {
    if !contains_ci(values, value) {
        values.push(value.to_string());
    }
}

fn registry_rows_overlap(left: &RegistryMigrationRow, right: &RegistryMigrationRow) -> bool {
    left.canonical_name
        .eq_ignore_ascii_case(&right.canonical_name)
        || contains_ci(&left.aliases, &right.canonical_name)
        || contains_ci(&right.aliases, &left.canonical_name)
}

fn migrate_attribute_registry_v3(conn: &mut PooledConn) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rows = {
        let mut stmt = tx.prepare(
            "SELECT id, canonical_name, aliases_json, entity_kinds_json, created_at
             FROM attribute_registry ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let aliases_json: String = row.get(2)?;
            let entity_kinds_json: String = row.get(3)?;
            Ok(RegistryMigrationRow {
                id: row.get(0)?,
                canonical_name: row.get(1)?,
                aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
                entity_kinds: serde_json::from_str(&entity_kinds_json).unwrap_or_default(),
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut visited = vec![false; rows.len()];
    for start in 0..rows.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut component = vec![start];
        let mut cursor = 0;
        while cursor < component.len() {
            let current = component[cursor];
            for candidate in 0..rows.len() {
                if !visited[candidate] && registry_rows_overlap(&rows[current], &rows[candidate]) {
                    visited[candidate] = true;
                    component.push(candidate);
                }
            }
            cursor += 1;
        }
        if component.len() == 1 {
            continue;
        }
        component.sort_by(|left, right| {
            rows[*left]
                .created_at
                .cmp(&rows[*right].created_at)
                .then_with(|| rows[*left].id.cmp(&rows[*right].id))
        });
        let survivor = &rows[component[0]];
        let component_ids = component
            .iter()
            .map(|index| rows[*index].id.clone())
            .collect::<HashSet<_>>();
        let affected_entities = affected_attribute_entities_v3(&tx, &component_ids)?;
        let mut aliases = survivor.aliases.clone();
        let mut entity_kinds = survivor.entity_kinds.clone();
        for &index in &component[1..] {
            let loser = &rows[index];
            push_unique_ci(&mut aliases, &loser.canonical_name);
            for alias in &loser.aliases {
                push_unique_ci(&mut aliases, alias);
            }
            for kind in &loser.entity_kinds {
                push_unique_ci(&mut entity_kinds, kind);
            }
        }
        let aliases_json = serde_json::to_string(&aliases)
            .map_err(|error| AppError::Other(format!("failed to serialize aliases: {error}")))?;
        let entity_kinds_json = serde_json::to_string(&entity_kinds).map_err(|error| {
            AppError::Other(format!("failed to serialize entity kinds: {error}"))
        })?;
        tx.execute(
            "UPDATE attribute_registry SET aliases_json = ?1, entity_kinds_json = ?2 WHERE id = ?3",
            rusqlite::params![aliases_json, entity_kinds_json, survivor.id],
        )?;

        for &index in &component[1..] {
            remap_attribute_v3(&tx, &rows[index].id, survivor)?;
        }
        reconcile_attribute_history_v3(&tx, survivor, affected_entities)?;
    }

    tx.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_attribute_registry_canonical_ci
         ON attribute_registry(canonical_name COLLATE NOCASE)",
        [],
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn affected_attribute_entities_v3(
    tx: &rusqlite::Transaction<'_>,
    attribute_ids: &HashSet<String>,
) -> AppResult<HashSet<(String, String)>> {
    let mut affected = HashSet::new();
    {
        let mut stmt =
            tx.prepare("SELECT branch_id, entity_id, attribute_id FROM entity_attributes")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (branch_id, entity_id, attribute_id) = row?;
            if attribute_ids.contains(&attribute_id) {
                affected.insert((branch_id, entity_id));
            }
        }
    }
    {
        let mut stmt = tx.prepare(
            "SELECT branch_id, payload_json FROM timeline_entries
             WHERE kind IN ('entity_attribute_changed', 'entity_attribute_removed')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (branch_id, payload_json) = row?;
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_json) else {
                continue;
            };
            let Some(attribute_id) = payload.get("attribute_id").and_then(|value| value.as_str())
            else {
                continue;
            };
            let Some(entity_id) = payload.get("entity_id").and_then(|value| value.as_str()) else {
                continue;
            };
            if attribute_ids.contains(attribute_id) {
                affected.insert((branch_id, entity_id.to_string()));
            }
        }
    }
    Ok(affected)
}

fn reconcile_attribute_history_v3(
    tx: &rusqlite::Transaction<'_>,
    survivor: &RegistryMigrationRow,
    affected_entities: HashSet<(String, String)>,
) -> AppResult<()> {
    for (branch_id, entity_id) in affected_entities {
        let current = tx
            .query_row(
                "SELECT value, source FROM entity_attributes
                 WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
                rusqlite::params![branch_id, entity_id, survivor.id],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let existing_event_id = tx
            .query_row(
                "SELECT id FROM timeline_entries
                 WHERE branch_id = ?1
                   AND kind IN ('entity_attribute_changed', 'entity_attribute_removed')
                   AND json_extract(payload_json, '$.entity_id') = ?2
                   AND json_extract(payload_json, '$.attribute_id') = ?3
                 ORDER BY seq DESC LIMIT 1",
                rusqlite::params![branch_id, entity_id, survivor.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let (kind, content, payload) = match current.as_ref() {
            Some((value, source)) => (
                "entity_attribute_changed",
                format!("{} is {}.", survivor.canonical_name, value),
                serde_json::json!({
                    "entity_id": entity_id,
                    "attribute_id": survivor.id,
                    "attribute_name": survivor.canonical_name,
                    "before": null,
                    "after": value,
                    "source": source,
                    "migration": 3,
                }),
            ),
            None => (
                "entity_attribute_removed",
                format!("{} was removed.", survivor.canonical_name),
                serde_json::json!({
                    "entity_id": entity_id,
                    "attribute_id": survivor.id,
                    "attribute_name": survivor.canonical_name,
                    "before": null,
                    "source": "migration",
                    "migration": 3,
                }),
            ),
        };
        let payload_json = serde_json::to_string(&payload).map_err(|error| {
            AppError::Other(format!(
                "failed to serialize attribute migration event: {error}"
            ))
        })?;
        let event_id = if let Some(event_id) = existing_event_id {
            tx.execute(
                "UPDATE timeline_entries SET kind = ?1, content = ?2, payload_json = ?3
                 WHERE id = ?4",
                rusqlite::params![kind, content, payload_json, event_id],
            )?;
            event_id
        } else if current.is_some() {
            let seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM timeline_entries WHERE branch_id = ?1",
                [&branch_id],
                |row| row.get(0),
            )?;
            let event_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO timeline_entries
                 (id, branch_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'hidden', ?5, ?6, NULL, ?7)",
                rusqlite::params![event_id, branch_id, seq, kind, content, payload_json, chrono::Utc::now().to_rfc3339()],
            )?;
            event_id
        } else {
            continue;
        };
        if current.is_some() {
            tx.execute(
                "UPDATE entity_attributes SET last_event_id = ?1
                 WHERE branch_id = ?2 AND entity_id = ?3 AND attribute_id = ?4",
                rusqlite::params![event_id, branch_id, entity_id, survivor.id],
            )?;
        }
    }
    Ok(())
}

fn remap_attribute_v3(
    tx: &rusqlite::Transaction<'_>,
    loser_id: &str,
    survivor: &RegistryMigrationRow,
) -> AppResult<()> {
    let loser_values = {
        let mut stmt = tx.prepare(
            "SELECT branch_id, entity_id, value, source, updated_at, last_event_id
             FROM entity_attributes WHERE attribute_id = ?1",
        )?;
        let rows = stmt.query_map([loser_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (branch_id, entity_id, value, source, updated_at, last_event_id) in loser_values {
        let survivor_updated_at = tx
            .query_row(
                "SELECT updated_at FROM entity_attributes
                 WHERE branch_id = ?1 AND entity_id = ?2 AND attribute_id = ?3",
                rusqlite::params![branch_id, entity_id, survivor.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match survivor_updated_at {
            Some(existing_updated_at) if existing_updated_at < updated_at => {
                tx.execute(
                    "UPDATE entity_attributes
                     SET value = ?1, source = ?2, updated_at = ?3, last_event_id = ?4
                     WHERE branch_id = ?5 AND entity_id = ?6 AND attribute_id = ?7",
                    rusqlite::params![
                        value,
                        source,
                        updated_at,
                        last_event_id,
                        branch_id,
                        entity_id,
                        survivor.id
                    ],
                )?;
            }
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO entity_attributes
                     (branch_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        branch_id,
                        entity_id,
                        survivor.id,
                        value,
                        source,
                        updated_at,
                        last_event_id
                    ],
                )?;
            }
        }
    }
    tx.execute(
        "DELETE FROM entity_attributes WHERE attribute_id = ?1",
        [loser_id],
    )?;
    tx.execute(
        "UPDATE timeline_entries
         SET payload_json = json_set(payload_json, '$.attribute_id', ?1, '$.attribute_name', ?2)
         WHERE json_extract(payload_json, '$.attribute_id') = ?3",
        rusqlite::params![survivor.id, survivor.canonical_name, loser_id],
    )?;
    for field in ["actor_attribute_id", "target_attribute_id"] {
        tx.execute(
            &format!(
                "UPDATE timeline_entries SET payload_json = json_set(payload_json, '$.{field}', ?1)
                 WHERE json_extract(payload_json, '$.{field}') = ?2"
            ),
            rusqlite::params![survivor.id, loser_id],
        )?;
    }
    tx.execute("DELETE FROM attribute_registry WHERE id = ?1", [loser_id])?;
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
/// story/branch rows.
#[cfg(test)]
pub fn test_pool() -> Pool {
    let dir = std::env::temp_dir().join(format!("dungeon-test-{}", Uuid::new_v4()));
    init_pool(&dir).expect("initialize test schema")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn v3_migration_merges_case_and_alias_overlaps_and_remaps_references() {
        let dir = std::env::temp_dir().join(format!("dungeon-v3-{}", Uuid::new_v4()));
        let pool = init_pool(&dir).expect("initialize schema");
        {
            let conn = pool.get().unwrap();
            conn.execute_batch(
                "DROP INDEX idx_attribute_registry_canonical_ci;
                 PRAGMA user_version = 2;
                 INSERT INTO stories (id, title, created_at, updated_at, settings_json, default_branch_id)
                    VALUES ('story', 'Story', 'now', 'now', '{}', 'branch');
                 INSERT INTO branches (id, story_id, parent_branch_id, forked_at_entry_id, name, created_at)
                    VALUES ('branch', 'story', NULL, NULL, 'main', 'now');
                 INSERT INTO entities (id, story_id, kind, created_at)
                    VALUES ('entity', 'story', 'artifact', 'now'),
                           ('removed-entity', 'story', 'artifact', 'now');",
            )
            .unwrap();
            for (id, name, aliases, kinds, created_at) in [
                (
                    "survivor",
                    "Echo",
                    json!(["Pulse"]),
                    json!(["artifact"]),
                    "2026-01-01",
                ),
                (
                    "case-loser",
                    "echo",
                    json!(["Reverb"]),
                    json!(["character"]),
                    "2026-01-02",
                ),
                (
                    "alias-loser",
                    "Pulse",
                    json!(["Thrum"]),
                    json!(["object"]),
                    "2026-01-03",
                ),
            ] {
                conn.execute(
                    "INSERT INTO attribute_registry
                     (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, 0, 10, 'misc', 1, 'story', ?5)",
                    rusqlite::params![id, name, aliases.to_string(), kinds.to_string(), created_at],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO entity_attributes
                 (branch_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
                 VALUES ('branch', 'entity', 'survivor', 2, 'default', '2026-01-01', NULL),
                        ('branch', 'entity', 'case-loser', 9, 'user', '2026-01-04', NULL),
                        ('branch', 'removed-entity', 'survivor', 4, 'user', '2026-01-05', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO timeline_entries
                 (id, branch_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at)
                 VALUES ('attribute-event', 'branch', 0, 'entity_attribute_changed', 'hidden', 'echo changed', ?1, NULL, 'now'),
                        ('roll-event', 'branch', 1, 'diceroll', 'hidden', 'roll', ?2, NULL, 'now'),
                        ('source-exchange', 'branch', 2, 'narration', 'visible', 'source', '{}', NULL, 'now'),
                        ('survivor-change', 'branch', 3, 'entity_attribute_changed', 'hidden', 'Echo changed', ?3, 'source-exchange', 'now'),
                        ('loser-change', 'branch', 4, 'entity_attribute_changed', 'hidden', 'echo changed', ?4, 'source-exchange', 'now'),
                        ('loser-remove', 'branch', 5, 'entity_attribute_removed', 'hidden', 'echo removed', ?5, 'source-exchange', 'now')",
                rusqlite::params![
                    json!({"attribute_id":"case-loser","attribute_name":"echo","entity_id":"entity","after":9.0,"source":"inferred"}).to_string(),
                    json!({"actor_attribute_id":"case-loser","target_attribute_id":"alias-loser","modifiers":{"situational":0.25}}).to_string(),
                    json!({"attribute_id":"survivor","attribute_name":"Echo","entity_id":"removed-entity","after":4.0,"source":"user"}).to_string(),
                    json!({"attribute_id":"case-loser","attribute_name":"echo","entity_id":"removed-entity","after":7.0,"source":"inferred"}).to_string(),
                    json!({"attribute_id":"case-loser","attribute_name":"echo","entity_id":"removed-entity","before":7.0,"source":"user"}).to_string(),
                ],
            )
            .unwrap();
        }
        drop(pool);

        let pool = init_pool(&dir).expect("migrate schema");
        let conn = pool.get().unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        let (count, aliases_json, kinds_json): (i64, String, String) = conn
            .query_row(
                "SELECT COUNT(*), aliases_json, entity_kinds_json
                 FROM attribute_registry WHERE id IN ('survivor', 'case-loser', 'alias-loser')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap();
        assert!(aliases.iter().any(|alias| alias == "echo"));
        assert!(aliases.iter().any(|alias| alias == "Pulse"));
        assert!(aliases.iter().any(|alias| alias == "Reverb"));
        assert!(aliases.iter().any(|alias| alias == "Thrum"));
        let kinds: Vec<String> = serde_json::from_str(&kinds_json).unwrap();
        assert_eq!(kinds.len(), 3);

        let (attribute_id, value, source): (String, f64, String) = conn
            .query_row(
                "SELECT attribute_id, value, source FROM entity_attributes
                 WHERE branch_id = 'branch' AND entity_id = 'entity'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (attribute_id.as_str(), value, source.as_str()),
            ("survivor", 9.0, "user")
        );

        let attribute_payload: String = conn
            .query_row(
                "SELECT payload_json FROM timeline_entries WHERE id = 'attribute-event'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let attribute_payload: serde_json::Value =
            serde_json::from_str(&attribute_payload).unwrap();
        assert_eq!(attribute_payload["attribute_id"], json!("survivor"));
        assert_eq!(attribute_payload["attribute_name"], json!("Echo"));
        let roll_payload: String = conn
            .query_row(
                "SELECT payload_json FROM timeline_entries WHERE id = 'roll-event'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let roll_payload: serde_json::Value = serde_json::from_str(&roll_payload).unwrap();
        assert_eq!(roll_payload["actor_attribute_id"], json!("survivor"));
        assert_eq!(roll_payload["target_attribute_id"], json!("survivor"));
        assert_eq!(roll_payload["modifiers"]["situational"], json!(0.25));
        crate::features::timeline::projections::rebuild_branch(&conn, "branch").unwrap();
        let replayed_attribute_id: String = conn
            .query_row(
                "SELECT attribute_id FROM entity_attributes
                 WHERE branch_id = 'branch' AND entity_id = 'entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(replayed_attribute_id, "survivor");
        let replayed_removed_value: f64 = conn
            .query_row(
                "SELECT value FROM entity_attributes
                 WHERE branch_id = 'branch' AND entity_id = 'removed-entity' AND attribute_id = 'survivor'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(replayed_removed_value, 4.0);
        conn.execute(
            "DELETE FROM timeline_entries WHERE id = 'source-exchange'",
            [],
        )
        .unwrap();
        crate::features::timeline::projections::rebuild_branch(&conn, "branch").unwrap();
        let erased_attribute_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entity_attributes
                 WHERE branch_id = 'branch' AND entity_id = 'removed-entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(erased_attribute_count, 0);
        assert!(conn
            .execute(
                "INSERT INTO attribute_registry
                 (id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at)
                 VALUES ('duplicate', 'ECHO', '[]', '[]', 0, 10, 'misc', 1, NULL, 'later')",
                [],
            )
            .is_err());
        let foreign_key_errors: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_errors, 0);
        drop(conn);
        drop(pool);

        let pool = init_pool(&dir).expect("migration is idempotent");
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM attribute_registry WHERE id = 'survivor'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }
}
