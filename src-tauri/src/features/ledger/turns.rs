use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::error::AppResult;

use super::model::TurnSummary;

pub const COMPLETE: &str = "complete";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub id: String,
    pub story_id: String,
    pub seq: i64,
    pub status: String,
    pub created_at: String,
}

fn row_to_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<Turn> {
    Ok(Turn {
        id: row.get(0)?,
        story_id: row.get(1)?,
        seq: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
    })
}

pub fn create_turn(conn: &rusqlite::Connection, story_id: &str) -> AppResult<String> {
    let id = Uuid::new_v4().to_string();
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM turns WHERE story_id = ?1",
        [story_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO turns (id, story_id, seq, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, story_id, seq, COMPLETE, Utc::now().to_rfc3339()],
    )?;
    Ok(id)
}

pub fn last_turn(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Option<Turn>> {
    Ok(conn
        .query_row(
            "SELECT id, story_id, seq, status, created_at FROM turns
             WHERE story_id = ?1 ORDER BY seq DESC LIMIT 1",
            [story_id],
            row_to_turn,
        )
        .optional()?)
}

pub fn turn_of(conn: &rusqlite::Connection, entry_id: &str) -> AppResult<Option<Turn>> {
    Ok(conn
        .query_row(
            "SELECT turns.id, turns.story_id, turns.seq, turns.status, turns.created_at
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

    #[test]
    fn create_turn_writes_complete_turns_and_preserves_sequence() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let first = create_turn(&conn, "s").unwrap();
        let second = create_turn(&conn, "s").unwrap();
        let last = last_turn(&conn, "s").unwrap().unwrap();
        assert_eq!(last.id, second);
        assert_eq!(last.seq, 1);
        assert_eq!(last.status, COMPLETE);
        assert_eq!(
            list_summaries(&conn, "s").unwrap(),
            vec![
                TurnSummary {
                    id: first,
                    status: COMPLETE.into()
                },
                TurnSummary {
                    id: second,
                    status: COMPLETE.into()
                },
            ]
        );
    }
}
