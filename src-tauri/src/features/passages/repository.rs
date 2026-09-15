use chrono::Utc;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

use super::model::{Passage, PassageVariant};

pub(super) fn row_to_passage(row: &rusqlite::Row) -> rusqlite::Result<Passage> {
    Ok(Passage {
        id: row.get(0)?,
        branch_id: row.get(1)?,
        seq: row.get(2)?,
        role: row.get(3)?,
        input_mode: row.get(4)?,
        content: row.get(5)?,
        thoughts: row.get(6)?,
        created_at: row.get(7)?,
        edited_at: row.get(8)?,
    })
}

pub(super) fn row_to_variant(row: &rusqlite::Row) -> rusqlite::Result<PassageVariant> {
    Ok(PassageVariant {
        id: row.get(0)?,
        passage_id: row.get(1)?,
        content: row.get(2)?,
        is_selected: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
    })
}

fn next_seq(conn: &rusqlite::Connection, branch_id: &str) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM passages WHERE branch_id = ?1",
        [branch_id],
        |row| row.get(0),
    )?)
}

pub(super) fn get_passage(conn: &rusqlite::Connection, passage_id: &str) -> AppResult<Passage> {
    conn.query_row(
        "SELECT id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at FROM passages WHERE id = ?1",
        [passage_id],
        row_to_passage,
    )
    .map_err(|_| AppError::NotFound(format!("passage {passage_id} not found")))
}

pub(super) fn get_story_id_for_branch(
    conn: &rusqlite::Connection,
    branch_id: &str,
) -> AppResult<String> {
    conn.query_row(
        "SELECT story_id FROM branches WHERE id = ?1",
        [branch_id],
        |row| row.get(0),
    )
    .map_err(|_| AppError::NotFound(format!("branch {branch_id} not found")))
}

pub(super) fn get_last_passage(
    conn: &rusqlite::Connection,
    branch_id: &str,
) -> AppResult<Option<Passage>> {
    conn.query_row(
        "SELECT id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at
         FROM passages WHERE branch_id = ?1 ORDER BY seq DESC LIMIT 1",
        [branch_id],
        row_to_passage,
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn insert_passage(
    conn: &rusqlite::Connection,
    branch_id: &str,
    role: &str,
    input_mode: &str,
    content: &str,
    thoughts: Option<&str>,
) -> AppResult<Passage> {
    let id = Uuid::new_v4().to_string();
    let seq = next_seq(conn, branch_id)?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO passages (id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        rusqlite::params![id, branch_id, seq, role, input_mode, content, thoughts, now],
    )?;
    conn.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (
             SELECT story_id FROM branches WHERE id = ?2
         )",
        rusqlite::params![now, branch_id],
    )?;
    Ok(Passage {
        id,
        branch_id: branch_id.to_string(),
        seq,
        role: role.to_string(),
        input_mode: input_mode.to_string(),
        content: content.to_string(),
        thoughts: thoughts.map(str::to_string),
        created_at: now,
        edited_at: None,
    })
}

pub(super) fn list_passages(
    conn: &rusqlite::Connection,
    branch_id: &str,
) -> AppResult<Vec<Passage>> {
    let mut stmt = conn.prepare(
        "SELECT id, branch_id, seq, role, input_mode, content, thoughts, created_at, edited_at
         FROM passages WHERE branch_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([branch_id], row_to_passage)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(super) fn image_paths_for_passage(
    conn: &rusqlite::Connection,
    passage_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM images WHERE passage_id = ?1")?;
    let rows = stmt.query_map([passage_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
