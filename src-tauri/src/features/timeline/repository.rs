use std::collections::HashSet;

use chrono::Utc;
use rusqlite::OptionalExtension;
use serde_json::Value;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

use super::model::TimelineEntry;

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<TimelineEntry> {
    let raw: String = row.get(6)?;
    Ok(TimelineEntry {
        id: row.get(0)?,
        branch_id: row.get(1)?,
        seq: row.get(2)?,
        kind: row.get(3)?,
        visibility: row.get(4)?,
        content: row.get(5)?,
        payload: serde_json::from_str(&raw).unwrap_or(Value::Object(Default::default())),
        target_entry_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn next_seq(conn: &rusqlite::Connection, branch_id: &str) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM timeline_entries WHERE branch_id = ?1",
        [branch_id],
        |row| row.get(0),
    )?)
}

pub fn append_entry(
    conn: &rusqlite::Connection,
    branch_id: &str,
    kind: &str,
    visibility: &str,
    content: Option<&str>,
    payload: &Value,
    target_entry_id: Option<&str>,
) -> AppResult<TimelineEntry> {
    if visibility != "visible" && visibility != "hidden" {
        return Err(AppError::Invalid(format!(
            "invalid timeline visibility: {visibility}"
        )));
    }
    let id = Uuid::new_v4().to_string();
    let seq = next_seq(conn, branch_id)?;
    let now = Utc::now().to_rfc3339();
    let payload_json = serde_json::to_string(payload)
        .map_err(|e| AppError::Other(format!("timeline payload serialization failed: {e}")))?;
    conn.execute(
        "INSERT INTO timeline_entries
         (id, branch_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            id,
            branch_id,
            seq,
            kind,
            visibility,
            content,
            payload_json,
            target_entry_id,
            now
        ],
    )?;
    conn.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = (SELECT story_id FROM branches WHERE id = ?2)",
        rusqlite::params![now, branch_id],
    )?;
    Ok(TimelineEntry {
        id,
        branch_id: branch_id.to_string(),
        seq,
        kind: kind.to_string(),
        visibility: visibility.to_string(),
        content: content.map(str::to_string),
        payload: payload.clone(),
        target_entry_id: target_entry_id.map(str::to_string),
        created_at: now,
    })
}

pub fn get_entry(conn: &rusqlite::Connection, id: &str) -> AppResult<TimelineEntry> {
    conn.query_row(
        "SELECT id, branch_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at
         FROM timeline_entries WHERE id = ?1",
        [id],
        row_to_entry,
    )
    .map_err(|_| AppError::NotFound(format!("timeline entry {id} not found")))
}

pub fn get_story_id_for_branch(conn: &rusqlite::Connection, branch_id: &str) -> AppResult<String> {
    conn.query_row(
        "SELECT story_id FROM branches WHERE id = ?1",
        [branch_id],
        |row| row.get(0),
    )
    .map_err(|_| AppError::NotFound(format!("branch {branch_id} not found")))
}

fn local_entries(
    conn: &rusqlite::Connection,
    branch_id: &str,
    through_entry_id: Option<&str>,
    since_seq: Option<i64>,
) -> AppResult<Vec<TimelineEntry>> {
    let through_seq = through_entry_id
        .map(|id| {
            conn.query_row(
                "SELECT seq FROM timeline_entries WHERE id = ?1 AND branch_id = ?2",
                rusqlite::params![id, branch_id],
                |row| row.get::<_, i64>(0),
            )
        })
        .transpose()?
        .unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT id, branch_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at
         FROM timeline_entries WHERE branch_id = ?1 AND seq <= ?2 AND seq >= ?3 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![branch_id, through_seq, since_seq.unwrap_or(i64::MIN)],
        row_to_entry,
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn logical_entries_inner(
    conn: &rusqlite::Connection,
    branch_id: &str,
    through_entry_id: Option<&str>,
    since_seq: Option<i64>,
    visited: &mut HashSet<String>,
) -> AppResult<Vec<TimelineEntry>> {
    if !visited.insert(branch_id.to_string()) {
        return Err(AppError::Invalid("branch parent cycle detected".into()));
    }
    let parent: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT parent_branch_id, forked_at_entry_id FROM branches WHERE id = ?1 AND parent_branch_id IS NOT NULL",
            [branch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    // `since_seq` only ever bounds the branch this call started on — an
    // ancestor branch's own sequence numbering is independent, and its
    // entries all precede the fork point anyway, so they're always walked
    // in full.
    let mut out = if let Some((parent_id, fork_id)) = parent {
        logical_entries_inner(conn, &parent_id, fork_id.as_deref(), None, visited)?
    } else {
        Vec::new()
    };
    out.extend(local_entries(conn, branch_id, through_entry_id, since_seq)?);
    visited.remove(branch_id);
    Ok(out)
}

pub fn list_logical_entries(
    conn: &rusqlite::Connection,
    branch_id: &str,
) -> AppResult<Vec<TimelineEntry>> {
    logical_entries_inner(conn, branch_id, None, None, &mut HashSet::new())
}

/// Like `list_logical_entries`, but skips entries on `branch_id` itself with
/// `seq < since_seq` — used once a durable context summary already covers
/// that prefix, so a long, already-compacted story doesn't pay to reload and
/// re-decode history that's known to be superseded on every single turn.
pub fn list_logical_entries_since(
    conn: &rusqlite::Connection,
    branch_id: &str,
    since_seq: i64,
) -> AppResult<Vec<TimelineEntry>> {
    logical_entries_inner(conn, branch_id, None, Some(since_seq), &mut HashSet::new())
}

pub fn image_paths_for_entry(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM image_assets WHERE entry_id = ?1")?;
    let rows = stmt.query_map([entry_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn active_entry(conn: &rusqlite::Connection, entry_id: &str) -> AppResult<TimelineEntry> {
    let base = get_entry(conn, entry_id)?;
    let entries = list_logical_entries(conn, &base.branch_id)?;
    super::reducer::active_visible_entries(&entries)
        .into_iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| AppError::NotFound(format!("active timeline entry {entry_id} not found")))
}

#[cfg(test)]
mod tests {
    use super::super::model::kind;
    use super::*;
    use serde_json::json;

    #[test]
    fn child_branch_inherits_only_through_fork_entry() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE branches(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, parent_branch_id TEXT, forked_at_entry_id TEXT);
            CREATE TABLE timeline_entries(id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(branch_id,seq));").unwrap();
        conn.execute("INSERT INTO stories VALUES ('s','now')", [])
            .unwrap();
        conn.execute("INSERT INTO branches VALUES ('parent','s',NULL,NULL)", [])
            .unwrap();
        let first = append_entry(
            &conn,
            "parent",
            "narration",
            "visible",
            Some("one"),
            &json!({}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "parent",
            "narration",
            "visible",
            Some("two"),
            &json!({}),
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO branches VALUES ('child','s','parent',?1)",
            [&first.id],
        )
        .unwrap();
        append_entry(
            &conn,
            "child",
            "player_message",
            "visible",
            Some("branch action"),
            &json!({}),
            None,
        )
        .unwrap();
        let logical = list_logical_entries(&conn, "child").unwrap();
        assert_eq!(
            logical
                .iter()
                .map(|e| e.content.as_deref().unwrap_or(""))
                .collect::<Vec<_>>(),
            vec!["one", "branch action"]
        );
    }

    #[test]
    fn since_seq_includes_boundary_and_excludes_earlier_entries() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE branches(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, parent_branch_id TEXT, forked_at_entry_id TEXT);
            CREATE TABLE timeline_entries(id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(branch_id,seq));").unwrap();
        conn.execute("INSERT INTO stories VALUES ('s','now')", [])
            .unwrap();
        conn.execute("INSERT INTO branches VALUES ('b','s',NULL,NULL)", [])
            .unwrap();

        let one = append_entry(
            &conn,
            "b",
            "narration",
            "visible",
            Some("one"),
            &json!({}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            "narration",
            "visible",
            Some("two"),
            &json!({}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            "narration",
            "visible",
            Some("three"),
            &json!({}),
            None,
        )
        .unwrap();

        let since = list_logical_entries_since(&conn, "b", one.seq).unwrap();
        assert_eq!(
            since
                .iter()
                .map(|e| e.content.as_deref().unwrap_or(""))
                .collect::<Vec<_>>(),
            vec!["one", "two", "three"]
        );

        let since_later = list_logical_entries_since(&conn, "b", one.seq + 1).unwrap();
        assert_eq!(
            since_later
                .iter()
                .map(|e| e.content.as_deref().unwrap_or(""))
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
    }

    #[test]
    fn append_assigns_monotonic_sequence_and_decodes_payloads() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE branches(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, parent_branch_id TEXT, forked_at_entry_id TEXT);
            CREATE TABLE timeline_entries(id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(branch_id,seq));").unwrap();
        conn.execute("INSERT INTO stories VALUES ('s','now')", [])
            .unwrap();
        conn.execute("INSERT INTO branches VALUES ('b','s',NULL,NULL)", [])
            .unwrap();

        append_entry(
            &conn,
            "b",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("first"),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            kind::DICEROLL,
            "hidden",
            None,
            &json!({"roll":17,"outcome":"success"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("third"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();

        let entries = list_logical_entries(&conn, "b").unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(entries[1].kind, kind::DICEROLL);
        assert_eq!(entries[1].payload["roll"], 17);
        assert_eq!(entries[1].payload["outcome"], "success");
    }
}
