use rig_core::completion::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::shared::db::Pool;

use super::boundary_for;

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
