use rig_core::{
    completion::Message,
    memory::{Compactor, MemoryError},
    wasm_compat::WasmBoxedFuture,
};
use rig_memory::{HeuristicTokenCounter, MemoryPolicy, TokenCounter, TokenWindowMemory};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ai::{self, HistoryTurn, TextModelConfig},
    shared::db::Pool,
};

use super::{model::kind, repository};

const FALLBACK_CONTEXT_WINDOW: usize = 32_768;
const RAW_TAIL_MESSAGES: usize = 12;

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

#[derive(Clone)]
pub struct NarratorCompactor {
    config: TextModelConfig,
}

impl NarratorCompactor {
    pub fn new(config: TextModelConfig) -> Self {
        Self { config }
    }
}

impl Compactor for NarratorCompactor {
    type Artifact = SummaryArtifact;

    fn compact<'a>(
        &'a self,
        _conversation_id: &'a str,
        evicted: &'a [Message],
        carry_over: Option<&'a Self::Artifact>,
    ) -> WasmBoxedFuture<'a, Result<Self::Artifact, MemoryError>> {
        Box::pin(async move {
            let prior = carry_over.map(|a| format_summary(&a.0)).unwrap_or_default();
            let transcript =
                serde_json::to_string(evicted).map_err(|e| MemoryError::Internal(e.to_string()))?;
            let prompt = format!(
                "Previous summary:\n{prior}\n\nOlder timeline messages to compact:\n{transcript}"
            );
            let preamble = "Summarize an interactive story's older context. Preserve concrete facts, promises, relationships, unresolved plot threads, dice-roll outcomes, and entity-relevant details. Do not invent events. Return structured output only.";
            ai::prompt_typed::<ContextSummary>(&self.config, preamble, prompt)
                .await
                .map(SummaryArtifact)
                .map_err(|e| MemoryError::Policy(e.to_string()))
        })
    }
}

fn format_summary(summary: &ContextSummary) -> String {
    format!(
        "{}\n\nFacts:\n- {}\nEntity notes:\n- {}\nOpen threads:\n- {}\nUnresolved mechanics:\n- {}",
        summary.prose,
        summary.facts.join("\n- "),
        summary.entity_notes.join("\n- "),
        summary.open_threads.join("\n- "),
        summary.unresolved_mechanics.join("\n- ")
    )
}

/// The most recently persisted summary, reconstructed for `Compactor::compact`'s
/// `carry_over` parameter so a new compaction pass is told about the prior
/// summary through its dedicated channel instead of only via the raw
/// transcript text.
fn latest_summary_artifact(pool: &Pool, branch_id: &str) -> Option<SummaryArtifact> {
    let conn = pool.get().ok()?;
    let payload_json: String = conn
        .query_row(
            "SELECT payload_json FROM timeline_entries WHERE branch_id = ?1 AND kind = ?2 ORDER BY seq DESC LIMIT 1",
            rusqlite::params![branch_id, kind::CONTEXT_SUMMARY],
            |row| row.get(0),
        )
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&payload_json).ok()?;
    serde_json::from_value::<ContextSummary>(value)
        .ok()
        .map(SummaryArtifact)
}

fn messages(history: &[HistoryTurn]) -> Vec<Message> {
    history
        .iter()
        .map(|turn| {
            if turn.is_player {
                Message::user(turn.content.clone())
            } else {
                Message::assistant(turn.content.clone())
            }
        })
        .collect()
}

pub async fn prepare_history(
    pool: &Pool,
    branch_id: &str,
    config: &TextModelConfig,
    preamble: &str,
    prompt: &str,
    history: Vec<HistoryTurn>,
) -> Vec<HistoryTurn> {
    let counter = HeuristicTokenCounter::openai();
    let context_window = if config.context_window == 0 {
        FALLBACK_CONTEXT_WINDOW
    } else {
        config.context_window
    };
    let response_reserve = 8_192usize.min(context_window / 5);
    let trigger = ((context_window as f64) * 0.90) as usize;
    let fixed = counter.count(&Message::system(preamble.to_string()))
        + counter.count(&Message::user(prompt.to_string()))
        + response_reserve;
    let history_messages = messages(&history);
    let total = fixed
        + history_messages
            .iter()
            .map(|m| counter.count(m))
            .sum::<usize>();
    if total < trigger {
        return history;
    }

    let target_history_budget = (((context_window as f64) * 0.75) as usize).saturating_sub(fixed);
    let policy = TokenWindowMemory::new(target_history_budget, counter);
    let kept = policy
        .apply(history_messages.clone())
        .unwrap_or_else(|_| history_messages.clone());
    let keep_count = kept.len().max(RAW_TAIL_MESSAGES.min(history.len()));
    let split = history.len().saturating_sub(keep_count);
    if split == 0 {
        return history;
    }
    let through_entry_id = history[..split]
        .iter()
        .rev()
        .find_map(|turn| turn.entry_id.clone());

    // When history already opens with a prior summary, route it through
    // `carry_over` instead of re-sending it as part of the evicted
    // transcript, so it isn't duplicated in the compaction prompt.
    let carry_over = history
        .first()
        .is_some_and(|turn| turn.content.starts_with("[Authoritative context summary]"))
        .then(|| latest_summary_artifact(pool, branch_id))
        .flatten();
    let evict_from = usize::from(carry_over.is_some());

    let compactor = NarratorCompactor::new(config.clone());
    match compactor
        .compact(
            branch_id,
            &history_messages[evict_from..split],
            carry_over.as_ref(),
        )
        .await
    {
        Ok(artifact) => {
            let summary_text = format_summary(&artifact.0);
            if let Ok(mut conn) = pool.get() {
                if let Ok(tx) =
                    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                {
                    let boundary = through_entry_id
                        .as_deref()
                        .and_then(|id| repository::get_entry(&tx, id).ok());
                    if let Some(boundary) = boundary {
                        let _ = repository::append_entry(
                            &tx,
                            branch_id,
                            kind::CONTEXT_SUMMARY,
                            "hidden",
                            Some(&summary_text),
                            &serde_json::json!({"through_seq": boundary.seq, "through_entry_id": boundary.id, "prose": artifact.0.prose, "facts": artifact.0.facts,
                                "entity_notes": artifact.0.entity_notes, "open_threads": artifact.0.open_threads,
                                "unresolved_mechanics": artifact.0.unresolved_mechanics}),
                            None,
                        );
                    }
                    let _ = tx.commit();
                }
            }
            let mut compacted = vec![HistoryTurn {
                entry_id: None,
                is_player: false,
                content: format!("[Authoritative context summary]\n{summary_text}"),
            }];
            compacted.extend(history.into_iter().skip(split));
            compacted
        }
        Err(_) => {
            if history
                .first()
                .is_some_and(|turn| turn.content.starts_with("[Authoritative context summary]"))
            {
                let summary = history[0].clone();
                let fallback_counter = HeuristicTokenCounter::openai();
                let remaining_budget = target_history_budget.saturating_sub(
                    fallback_counter.count(&Message::assistant(summary.content.clone())),
                );
                let fallback_policy = TokenWindowMemory::new(remaining_budget, fallback_counter);
                let recent_count = fallback_policy
                    .apply(messages(&history[1..]))
                    .map(|recent| recent.len())
                    .unwrap_or(0);
                let mut fallback = vec![summary];
                let mut recent = history
                    .into_iter()
                    .skip(1)
                    .rev()
                    .take(recent_count)
                    .collect::<Vec<_>>();
                recent.reverse();
                fallback.extend(recent);
                fallback
            } else {
                // Same floor as the success path: a failed compaction call
                // must not be able to drop below the raw tail guarantee.
                let skip = history.len().saturating_sub(keep_count);
                history.into_iter().skip(skip).collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_summary_renders_all_sections() {
        let text = format_summary(&ContextSummary {
            prose: "Earlier".into(),
            facts: vec!["fact".into()],
            entity_notes: vec!["note".into()],
            open_threads: vec!["thread".into()],
            unresolved_mechanics: vec!["roll".into()],
        });
        assert!(text.contains("Earlier") && text.contains("fact") && text.contains("thread"));
    }
}
