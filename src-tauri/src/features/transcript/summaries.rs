use std::collections::HashSet;

use crate::shared::error::{AppError, AppResult};

use super::{model::kind as transcript_kind, repository};

#[derive(Debug, Clone)]
pub(crate) struct SummaryBoundary {
    pub summary_entry_id: String,
    pub through_seq: i64,
}

/// Latest valid summary with an existing covered boundary.
pub(crate) fn latest_boundary(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Option<SummaryBoundary>> {
    let mut stmt = conn.prepare(
        "SELECT id, payload_json FROM ledger_entries
         WHERE story_id = ?1 AND kind = ?2 ORDER BY seq DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![story_id, transcript_kind::CONTEXT_SUMMARY],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    for row in rows {
        let (summary_entry_id, payload_json) = row?;
        let boundary = serde_json::from_str::<serde_json::Value>(&payload_json)
            .ok()
            .and_then(|value| {
                Some((
                    value.get("through_seq")?.as_i64()?,
                    value.get("through_entry_id")?.as_str()?.to_string(),
                ))
            });
        if let Some((through_seq, through_entry_id)) = boundary {
            let boundary_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM ledger_entries WHERE id = ?1)",
                [through_entry_id],
                |row| row.get(0),
            )?;
            if boundary_exists {
                return Ok(Some(SummaryBoundary {
                    summary_entry_id,
                    through_seq,
                }));
            }
        }
    }
    Ok(None)
}

pub(crate) fn latest_payload(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Option<serde_json::Value>> {
    let Some(boundary) = latest_boundary(conn, story_id)? else {
        return Ok(None);
    };
    let payload: String = conn.query_row(
        "SELECT payload_json FROM ledger_entries WHERE id = ?1",
        [boundary.summary_entry_id],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&payload).ok())
}

pub(crate) fn append(
    conn: &rusqlite::Connection,
    story_id: &str,
    through_entry_id: Option<&str>,
    summary_text: &str,
    summary: &impl serde::Serialize,
) -> AppResult<()> {
    let boundary = through_entry_id.and_then(|id| repository::get_entry(conn, id).ok());
    if let Some(boundary) = boundary {
        let mut payload = serde_json::to_value(summary)
            .map_err(|error| AppError::Other(error.to_string()))?;
        payload["through_seq"] = serde_json::json!(boundary.seq);
        payload["through_entry_id"] = serde_json::json!(boundary.id);
        repository::append_entry(
            conn,
            story_id,
            transcript_kind::CONTEXT_SUMMARY,
            "hidden",
            Some(summary_text),
            &payload,
            None,
            None,
        )?;
    }
    Ok(())
}

pub(crate) fn prune_covering(
    conn: &rusqlite::Connection,
    story_id: &str,
    doomed_ids: &HashSet<String>,
) -> AppResult<()> {
    let mut stmt = conn
        .prepare("SELECT id, payload_json FROM ledger_entries WHERE story_id = ?1 AND kind = ?2")?;
    let summaries: Vec<(String, String)> = stmt
        .query_map(
            rusqlite::params![story_id, transcript_kind::CONTEXT_SUMMARY],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<_, _>>()?;
    for (summary_id, payload_json) in summaries {
        let through = serde_json::from_str::<serde_json::Value>(&payload_json)
            .ok()
            .and_then(|value| value.get("through_entry_id")?.as_str().map(str::to_string));
        if through.is_some_and(|id| doomed_ids.contains(&id)) {
            conn.execute("DELETE FROM ledger_entries WHERE id = ?1", [&summary_id])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_only_summaries_covering_doomed_entries_in_story() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ledger_entries(id TEXT PRIMARY KEY, story_id TEXT, kind TEXT, payload_json TEXT);
             INSERT INTO ledger_entries VALUES ('old', 'story-1', 'context_summary', '{\"through_entry_id\":\"doomed\"}');
             INSERT INTO ledger_entries VALUES ('safe', 'story-1', 'context_summary', '{\"through_entry_id\":\"survives\"}');
             INSERT INTO ledger_entries VALUES ('other-story', 'story-2', 'context_summary', '{\"through_entry_id\":\"doomed\"}');
             INSERT INTO ledger_entries VALUES ('invalid', 'story-1', 'context_summary', 'not json');",
        )
        .unwrap();

        let tx = conn.transaction().unwrap();
        prune_covering(&tx, "story-1", &HashSet::from(["doomed".into()])).unwrap();
        let ids: Vec<String> = tx
            .prepare("SELECT id FROM ledger_entries ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, vec!["invalid", "other-story", "safe"]);
    }
}
