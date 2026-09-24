use r2d2_sqlite::SqliteConnectionManager;

use crate::shared::db::{database_path, Pool};
use crate::shared::error::AppResult;

use super::projection;

pub fn excluding_turn(live: &Pool, story_id: &str, turn_id: &str) -> AppResult<Pool> {
    let path = database_path(live)?;
    let story_id = story_id.to_string();
    let turn_id = turn_id.to_string();
    let manager = SqliteConnectionManager::file(path).with_init(move |conn| {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TEMP TABLE entities (
                 id TEXT PRIMARY KEY,
                 story_id TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE TEMP TABLE story_entity_state (
                 story_id TEXT NOT NULL,
                 entity_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 appearance_anchor TEXT,
                 is_present INTEGER NOT NULL DEFAULT 1,
                 updated_at TEXT NOT NULL,
                 last_event_id TEXT,
                 PRIMARY KEY (story_id, entity_id)
             );
             CREATE TEMP TABLE entity_attributes (
                 story_id TEXT NOT NULL,
                 entity_id TEXT NOT NULL,
                 attribute_id TEXT NOT NULL,
                 value REAL NOT NULL,
                 source TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 last_event_id TEXT,
                 PRIMARY KEY (story_id, entity_id, attribute_id)
             );",
        )?;
        projection::rebuild_story(conn, &story_id, Some(&turn_id))
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    });
    let pool = r2d2::Pool::builder().max_size(4).build(manager)?;
    drop(pool.get()?);
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        entities::{attributes, events::EntityEvent},
        ledger::{model::kind as ledger_kind, turns},
    };

    fn health(
        conn: &rusqlite::Connection,
    ) -> crate::features::entities::model::AttributeRegistryEntry {
        let id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Health'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        attributes::find_attribute_by_id(conn, &id).unwrap()
    }

    #[test]
    fn view_excludes_retried_turn_effects() {
        let live = crate::shared::db::test_pool();
        let conn = live.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('story', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let health = health(&conn);
        let turn_one = turns::create_turn(&conn, "story").unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "guard",
            "story",
            "character",
            "Guard",
            None,
            "narrator_tool",
            None,
            Some(&turn_one),
        )
        .unwrap();
        projection::record(
            &conn,
            "story",
            None,
            "Guard Health changed to 7.",
            &EntityEvent::AttributeChanged {
                entity_id: "guard".into(),
                attribute_id: health.id.clone(),
                attribute_name: health.canonical_name.clone(),
                before: None,
                after: 7.0,
                source: "inferred".into(),
                delta: None,
                cause: None,
            },
            Some(&turn_one),
        )
        .unwrap();
        turns::set_status(&conn, &turn_one, turns::COMPLETE).unwrap();

        let turn_two = turns::create_turn(&conn, "story").unwrap();
        projection::record(
            &conn,
            "story",
            None,
            "Guard Health changed to 2.",
            &EntityEvent::AttributeChanged {
                entity_id: "guard".into(),
                attribute_id: health.id.clone(),
                attribute_name: health.canonical_name,
                before: Some(7.0),
                after: 2.0,
                source: "inferred".into(),
                delta: Some(-5.0),
                cause: Some("wounded".into()),
            },
            Some(&turn_two),
        )
        .unwrap();
        turns::set_status(&conn, &turn_two, turns::COMPLETE).unwrap();
        let live_health: f64 = conn
            .query_row(
                "SELECT value FROM entity_attributes
                 WHERE story_id = 'story' AND entity_id = 'guard' AND attribute_id = ?1",
                [&health.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live_health, 2.0);
        drop(conn);

        let view = excluding_turn(&live, "story", &turn_two).unwrap();
        let conn = view.get().unwrap();
        let view_health: f64 = conn
            .query_row(
                "SELECT value FROM entity_attributes
                 WHERE story_id = 'story' AND entity_id = 'guard' AND attribute_id = ?1",
                [&health.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(view_health, 7.0);
        let second = view.get().unwrap();
        assert_eq!(
            second
                .query_row(
                    "SELECT value FROM entity_attributes
                     WHERE story_id = 'story' AND entity_id = 'guard' AND attribute_id = ?1",
                    [&health.id],
                    |row| row.get::<_, f64>(0),
                )
                .unwrap(),
            7.0
        );
        assert_eq!(
            live.get()
                .unwrap()
                .query_row(
                    "SELECT value FROM entity_attributes
                     WHERE story_id = 'story' AND entity_id = 'guard' AND attribute_id = ?1",
                    [&health.id],
                    |row| row.get::<_, f64>(0),
                )
                .unwrap(),
            2.0
        );
    }

    #[test]
    fn view_skips_player_attribute_event_when_entity_creation_is_excluded() {
        let live = crate::shared::db::test_pool();
        let conn = live.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('story', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let health = health(&conn);
        let turn_id = turns::create_turn(&conn, "story").unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "guard",
            "story",
            "character",
            "Guard",
            None,
            "narrator_tool",
            None,
            Some(&turn_id),
        )
        .unwrap();
        turns::set_status(&conn, &turn_id, turns::COMPLETE).unwrap();
        attributes::set_entity_attribute_sync(&conn, "story", "guard", &health.id, 7.0).unwrap();
        drop(conn);

        let view = excluding_turn(&live, "story", &turn_id).unwrap();
        let conn = view.get().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entity_attributes
                 WHERE story_id = 'story' AND entity_id = 'guard'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::ENTITY_ATTRIBUTE_CHANGED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }
}
