use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::shared::error::{AppError, AppResult};

use super::model::LedgerEntry;

pub(crate) fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<LedgerEntry> {
    let raw: String = row.get(6)?;
    Ok(LedgerEntry {
        id: row.get(0)?,
        story_id: row.get(1)?,
        seq: row.get(2)?,
        kind: row.get(3)?,
        visibility: row.get(4)?,
        content: row.get(5)?,
        payload: serde_json::from_str(&raw).unwrap_or(Value::Object(Default::default())),
        target_entry_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn next_seq(conn: &rusqlite::Connection, story_id: &str) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM ledger_entries WHERE story_id = ?1",
        [story_id],
        |row| row.get(0),
    )?)
}

pub fn append_entry(
    conn: &rusqlite::Connection,
    story_id: &str,
    kind: &str,
    visibility: &str,
    content: Option<&str>,
    payload: &Value,
    target_entry_id: Option<&str>,
) -> AppResult<LedgerEntry> {
    if visibility != "visible" && visibility != "hidden" {
        return Err(AppError::Invalid(format!(
            "invalid ledger visibility: {visibility}"
        )));
    }
    let id = Uuid::new_v4().to_string();
    if let Some(target_id) = target_entry_id {
        let target_belongs_to_story: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM ledger_entries WHERE id = ?1 AND story_id = ?2)",
            rusqlite::params![target_id, story_id],
            |row| row.get(0),
        )?;
        if !target_belongs_to_story {
            return Err(AppError::Invalid(format!(
                "target ledger entry {target_id} does not belong to story {story_id}"
            )));
        }
    }
    let seq = next_seq(conn, story_id)?;
    let now = Utc::now().to_rfc3339();
    let payload_json = serde_json::to_string(payload)
        .map_err(|e| AppError::Other(format!("ledger payload serialization failed: {e}")))?;
    conn.execute(
        "INSERT INTO ledger_entries
         (id, story_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            id,
            story_id,
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
        "UPDATE stories SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, story_id],
    )?;
    Ok(LedgerEntry {
        id,
        story_id: story_id.to_string(),
        seq,
        kind: kind.to_string(),
        visibility: visibility.to_string(),
        content: content.map(str::to_string),
        payload: payload.clone(),
        target_entry_id: target_entry_id.map(str::to_string),
        created_at: now,
    })
}

pub fn get_entry(conn: &rusqlite::Connection, id: &str) -> AppResult<LedgerEntry> {
    conn.query_row(
        "SELECT id, story_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at
         FROM ledger_entries WHERE id = ?1",
        [id],
        row_to_entry,
    )
    .map_err(|_| AppError::NotFound(format!("ledger entry {id} not found")))
}

fn entries(
    conn: &rusqlite::Connection,
    story_id: &str,
    since_seq: Option<i64>,
) -> AppResult<Vec<LedgerEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, story_id, seq, kind, visibility, content, payload_json, target_entry_id, created_at
         FROM ledger_entries WHERE story_id = ?1 AND seq >= ?2 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![story_id, since_seq.unwrap_or(i64::MIN)],
        row_to_entry,
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_logical_entries(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<LedgerEntry>> {
    entries(conn, story_id, None)
}

pub fn list_logical_entries_since(
    conn: &rusqlite::Connection,
    story_id: &str,
    since_seq: i64,
) -> AppResult<Vec<LedgerEntry>> {
    entries(conn, story_id, Some(since_seq))
}

pub fn image_paths_for_entry(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM image_assets WHERE entry_id = ?1")?;
    let rows = stmt.query_map([entry_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn active_entry(conn: &rusqlite::Connection, entry_id: &str) -> AppResult<LedgerEntry> {
    let base = get_entry(conn, entry_id)?;
    let entries = list_logical_entries(conn, &base.story_id)?;
    super::reducer::active_visible_entries(&entries)
        .into_iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| AppError::NotFound(format!("active ledger entry {entry_id} not found")))
}

#[cfg(test)]
mod tests {
    use super::super::model::kind;
    use super::*;
    use serde_json::json;

    #[test]
    fn since_seq_includes_boundary_and_excludes_earlier_entries() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE ledger_entries(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(story_id,seq));").unwrap();
        conn.execute("INSERT INTO stories VALUES ('s','now')", [])
            .unwrap();

        let one = append_entry(
            &conn,
            "s",
            "narration",
            "visible",
            Some("one"),
            &json!({}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            "narration",
            "visible",
            Some("two"),
            &json!({}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            "narration",
            "visible",
            Some("three"),
            &json!({}),
            None,
        )
        .unwrap();

        let since = list_logical_entries_since(&conn, "s", one.seq).unwrap();
        assert_eq!(
            since
                .iter()
                .map(|e| e.content.as_deref().unwrap_or(""))
                .collect::<Vec<_>>(),
            vec!["one", "two", "three"]
        );

        let since_later = list_logical_entries_since(&conn, "s", one.seq + 1).unwrap();
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
            CREATE TABLE ledger_entries(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(story_id,seq));").unwrap();
        conn.execute("INSERT INTO stories VALUES ('s','now')", [])
            .unwrap();

        append_entry(
            &conn,
            "s",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("first"),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            kind::DICEROLL,
            "hidden",
            None,
            &json!({"roll":17,"outcome":"success"}),
            None,
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            kind::NARRATION,
            "visible",
            Some("third"),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();

        let entries = list_logical_entries(&conn, "s").unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(entries[1].kind, kind::DICEROLL);
        assert_eq!(entries[1].payload["roll"], 17);
        assert_eq!(entries[1].payload["outcome"], "success");
    }

    #[test]
    fn append_rejects_a_target_owned_by_another_story() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE stories(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
            CREATE TABLE ledger_entries(id TEXT PRIMARY KEY, story_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL, visibility TEXT NOT NULL, content TEXT, payload_json TEXT NOT NULL, target_entry_id TEXT, created_at TEXT NOT NULL, UNIQUE(story_id,seq));").unwrap();
        conn.execute(
            "INSERT INTO stories VALUES ('first','now'), ('second','now')",
            [],
        )
        .unwrap();
        let target = append_entry(
            &conn,
            "first",
            kind::NARRATION,
            "visible",
            Some("target"),
            &json!({}),
            None,
        )
        .unwrap();

        let error = append_entry(
            &conn,
            "second",
            kind::CONTENT_EDITED,
            "hidden",
            Some("cross-story edit"),
            &json!({}),
            Some(&target.id),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            AppError::Invalid(message)
                if message == format!(
                     "target ledger entry {} does not belong to story second",
                    target.id
                )
        ));
    }
}
