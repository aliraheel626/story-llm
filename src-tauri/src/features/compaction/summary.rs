use std::collections::HashSet;

use rig_core::completion::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::features::ledger::model::kind as ledger_kind;
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

#[derive(Debug, Clone)]
pub(crate) struct SummaryBoundary {
    pub summary_entry_id: String,
    pub through_seq: i64,
}

/// Latest valid summary for the requested cut. A retry may have newer summary
/// rows physically after its target, so both the summary and covered boundary
/// must precede `before_seq`.
pub(crate) fn boundary_for(
    conn: &rusqlite::Connection,
    story_id: &str,
    before_seq: Option<i64>,
) -> AppResult<Option<SummaryBoundary>> {
    let mut stmt = conn.prepare(
        "SELECT id, seq, payload_json FROM ledger_entries
         WHERE story_id = ?1 AND kind = ?2 ORDER BY seq DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![story_id, ledger_kind::CONTEXT_SUMMARY],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    for row in rows {
        let (summary_entry_id, summary_seq, payload_json) = row?;
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
            if before_seq.is_none_or(|cut| summary_seq < cut && through_seq < cut)
                && boundary_exists
            {
                return Ok(Some(SummaryBoundary {
                    summary_entry_id,
                    through_seq,
                }));
            }
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextSummary {
    pub prose: String,
    pub facts: Vec<String>,
    pub entity_notes: Vec<String>,
    pub open_threads: Vec<String>,
    pub unresolved_mechanics: Vec<String>,
}

#[derive(Clone)]
pub struct SummaryArtifact(pub ContextSummary);

impl From<SummaryArtifact> for Message {
    fn from(value: SummaryArtifact) -> Self {
        Message::system(format_summary(&value.0))
    }
}

pub(super) fn format_summary(summary: &ContextSummary) -> String {
    format!(
        "{}\n\nFacts:\n- {}\nEntity notes:\n- {}\nOpen threads:\n- {}\nUnresolved mechanics:\n- {}",
        summary.prose,
        summary.facts.join("\n- "),
        summary.entity_notes.join("\n- "),
        summary.open_threads.join("\n- "),
        summary.unresolved_mechanics.join("\n- ")
    )
}

pub(super) fn latest_summary_artifact(
    pool: &Pool,
    story_id: &str,
    before_seq: Option<i64>,
) -> Option<SummaryArtifact> {
    let conn = pool.get().ok()?;
    let boundary = boundary_for(&conn, story_id, before_seq).ok()??;
    let payload_json: String = conn
        .query_row(
            "SELECT payload_json FROM ledger_entries WHERE id = ?1",
            [boundary.summary_entry_id],
            |row| row.get(0),
        )
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&payload_json).ok()?;
    serde_json::from_value::<ContextSummary>(value)
        .ok()
        .map(SummaryArtifact)
}

pub(crate) fn prune_summaries_covering(
    conn: &rusqlite::Connection,
    story_id: &str,
    doomed_ids: &HashSet<String>,
) -> AppResult<()> {
    let mut stmt = conn
        .prepare("SELECT id, payload_json FROM ledger_entries WHERE story_id = ?1 AND kind = ?2")?;
    let summaries: Vec<(String, String)> = stmt
        .query_map(
            rusqlite::params![story_id, ledger_kind::CONTEXT_SUMMARY],
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
    fn structured_summary_renders_all_sections() {
        let value = ContextSummary {
            prose: "Earlier".into(),
            facts: vec!["fact".into()],
            entity_notes: vec!["note".into()],
            open_threads: vec!["thread".into()],
            unresolved_mechanics: vec!["roll".into()],
        };
        let text = format_summary(&value);
        assert!(text.contains("Earlier") && text.contains("fact") && text.contains("thread"));
    }

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
        prune_summaries_covering(&tx, "story-1", &HashSet::from(["doomed".into()])).unwrap();
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
