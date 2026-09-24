use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

use super::model::TurnSummary;

pub const PENDING: &str = "pending";
pub const COMPLETE: &str = "complete";
pub const FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub id: String,
    pub story_id: String,
    pub seq: i64,
    pub status: String,
    pub attempt: i64,
    pub created_at: String,
}

fn row_to_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<Turn> {
    Ok(Turn {
        id: row.get(0)?,
        story_id: row.get(1)?,
        seq: row.get(2)?,
        status: row.get(3)?,
        attempt: row.get(4)?,
        created_at: row.get(5)?,
    })
}

pub fn create_turn(conn: &rusqlite::Connection, story_id: &str) -> AppResult<String> {
    let id = Uuid::new_v4().to_string();
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM turns WHERE story_id = ?1",
        [story_id],
        |row| row.get(0),
    )?;
    let result = conn.execute(
        "INSERT INTO turns (id, story_id, seq, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, story_id, seq, PENDING, Utc::now().to_rfc3339()],
    );
    match result {
        Ok(_) => Ok(id),
        Err(error) => {
            let has_pending = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM turns WHERE story_id = ?1 AND status = ?2)",
                    rusqlite::params![story_id, PENDING],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false);
            if has_pending {
                Err(AppError::Invalid("a turn is already generating".into()))
            } else {
                Err(error.into())
            }
        }
    }
}

pub fn set_status(conn: &rusqlite::Connection, turn_id: &str, status: &str) -> AppResult<()> {
    if !matches!(status, PENDING | COMPLETE | FAILED) {
        return Err(AppError::Invalid(format!("invalid turn status: {status}")));
    }
    if status == PENDING {
        begin_attempt(conn, turn_id)?;
        return Ok(());
    }
    let updated = conn.execute(
        "UPDATE turns SET status = ?1 WHERE id = ?2",
        rusqlite::params![status, turn_id],
    )?;
    if updated != 1 {
        return Err(AppError::NotFound(format!("turn {turn_id} not found")));
    }
    Ok(())
}

pub fn begin_attempt(conn: &rusqlite::Connection, turn_id: &str) -> AppResult<i64> {
    let current = conn
        .query_row("SELECT status FROM turns WHERE id = ?1", [turn_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    match current.as_deref() {
        None => return Err(AppError::NotFound(format!("turn {turn_id} not found"))),
        Some(PENDING) => return Err(AppError::Invalid("a turn is already generating".into())),
        Some(_) => {}
    }

    let updated = conn
        .query_row(
            "UPDATE turns SET status = ?1, attempt = attempt + 1
             WHERE id = ?2 AND status != ?1 RETURNING attempt",
            rusqlite::params![PENDING, turn_id],
            |row| row.get(0),
        )
        .optional();
    match updated {
        Ok(Some(attempt)) => Ok(attempt),
        Ok(None) => Err(AppError::Invalid("a turn is already generating".into())),
        Err(error) => {
            let has_pending = conn
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM turns AS requested
                         JOIN turns AS pending ON pending.story_id = requested.story_id
                         WHERE requested.id = ?1 AND pending.status = ?2
                     )",
                    rusqlite::params![turn_id, PENDING],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false);
            if has_pending {
                Err(AppError::Invalid("a turn is already generating".into()))
            } else {
                Err(error.into())
            }
        }
    }
}

pub fn mark_image_failed(
    conn: &rusqlite::Connection,
    turn_id: &str,
    attempt: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE turns SET status = ?1
         WHERE id = ?2 AND attempt = ?3 AND status = ?4",
        rusqlite::params![FAILED, turn_id, attempt, COMPLETE],
    )?;
    Ok(())
}

pub fn last_turn(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Option<Turn>> {
    Ok(conn
        .query_row(
            "SELECT id, story_id, seq, status, attempt, created_at FROM turns
             WHERE story_id = ?1 ORDER BY seq DESC LIMIT 1",
            [story_id],
            row_to_turn,
        )
        .optional()?)
}

pub fn turn_of(conn: &rusqlite::Connection, entry_id: &str) -> AppResult<Option<Turn>> {
    Ok(conn
        .query_row(
            "SELECT turns.id, turns.story_id, turns.seq, turns.status, turns.attempt, turns.created_at
             FROM ledger_entries JOIN turns ON turns.id = ledger_entries.turn_id
             WHERE ledger_entries.id = ?1",
            [entry_id],
            row_to_turn,
        )
        .optional()?)
}

pub fn list_summaries(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Vec<TurnSummary>> {
    let mut stmt =
        conn.prepare("SELECT id, status FROM turns WHERE story_id = ?1 ORDER BY seq ASC")?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(TurnSummary {
            id: row.get(0)?,
            status: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn unique_pending_turn_is_enforced_across_pool_connections() {
        let pool = crate::shared::db::test_pool();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
                 VALUES ('s', 'Story', 'now', 'now', '{}')",
                [],
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let pool = pool.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut conn = pool.get().unwrap();
                    barrier.wait();
                    let tx = conn
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .unwrap();
                    let result = create_turn(&tx, "s");
                    if result.is_ok() {
                        tx.commit().unwrap();
                    }
                    result
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results.iter().any(|result| matches!(
            result,
            Err(AppError::Invalid(message)) if message == "a turn is already generating"
        )));
    }

    #[test]
    fn pending_turn_cannot_be_started_twice() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = create_turn(&conn, "s").unwrap();

        let error = set_status(&conn, &turn_id, PENDING).unwrap_err();
        assert!(matches!(
            error,
            AppError::Invalid(message) if message == "a turn is already generating"
        ));
    }

    #[test]
    fn attempts_start_at_zero_and_increment_when_generation_restarts() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = create_turn(&conn, "s").unwrap();
        let created = last_turn(&conn, "s").unwrap().unwrap();
        assert_eq!(created.attempt, 0);

        set_status(&conn, &turn_id, COMPLETE).unwrap();
        assert_eq!(begin_attempt(&conn, &turn_id).unwrap(), 1);
        let error = begin_attempt(&conn, &turn_id).unwrap_err();
        assert!(matches!(
            error,
            AppError::Invalid(message) if message == "a turn is already generating"
        ));
    }
}
