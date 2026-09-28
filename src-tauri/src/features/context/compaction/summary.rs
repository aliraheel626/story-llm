use rig_core::completion::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::features::transcript::summaries;

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

pub(crate) fn latest_summary_artifact(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> Option<SummaryArtifact> {
    let value = summaries::latest_payload(conn, story_id).ok()??;
    serde_json::from_value::<ContextSummary>(value)
        .ok()
        .map(SummaryArtifact)
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
}
