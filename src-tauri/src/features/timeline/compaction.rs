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
fn latest_summary_artifact(
    pool: &Pool,
    branch_id: &str,
    before_seq: Option<i64>,
) -> Option<SummaryArtifact> {
    let conn = pool.get().ok()?;
    let payload_json: String = match before_seq {
        Some(before_seq) => conn
            .query_row(
                "SELECT payload_json FROM timeline_entries WHERE branch_id = ?1 AND kind = ?2 AND seq < ?3 ORDER BY seq DESC LIMIT 1",
                rusqlite::params![branch_id, kind::CONTEXT_SUMMARY, before_seq],
                |row| row.get(0),
            )
            .ok()?,
        None => conn
            .query_row(
                "SELECT payload_json FROM timeline_entries WHERE branch_id = ?1 AND kind = ?2 ORDER BY seq DESC LIMIT 1",
                rusqlite::params![branch_id, kind::CONTEXT_SUMMARY],
                |row| row.get(0),
            )
            .ok()?,
    };
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
    before_seq: Option<i64>,
) -> Vec<HistoryTurn> {
    let compactor = NarratorCompactor::new(config.clone());
    prepare_history_with_compactor(
        HistoryPreparation {
            pool,
            branch_id,
            config,
            preamble,
            prompt,
            before_seq,
        },
        history,
        &compactor,
    )
    .await
}

struct HistoryPreparation<'a> {
    pool: &'a Pool,
    branch_id: &'a str,
    config: &'a TextModelConfig,
    preamble: &'a str,
    prompt: &'a str,
    before_seq: Option<i64>,
}

async fn prepare_history_with_compactor<C>(
    input: HistoryPreparation<'_>,
    history: Vec<HistoryTurn>,
    compactor: &C,
) -> Vec<HistoryTurn>
where
    C: Compactor<Artifact = SummaryArtifact>,
{
    let HistoryPreparation {
        pool,
        branch_id,
        config,
        preamble,
        prompt,
        before_seq,
    } = input;
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
        .then(|| latest_summary_artifact(pool, branch_id, before_seq))
        .flatten();
    let evict_from = usize::from(carry_over.is_some());

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
    use crate::ai::TextProviderKind;
    use crate::features::narration::history::load_history;
    use crate::features::timeline::reducer;
    use std::sync::{Arc, Mutex};

    fn summary(prose: &str) -> ContextSummary {
        ContextSummary {
            prose: prose.into(),
            facts: Vec::new(),
            entity_notes: Vec::new(),
            open_threads: Vec::new(),
            unresolved_mechanics: Vec::new(),
        }
    }

    #[test]
    fn structured_summary_renders_all_sections() {
        let mut value = summary("Earlier");
        value.facts.push("fact".into());
        value.entity_notes.push("note".into());
        value.open_threads.push("thread".into());
        value.unresolved_mechanics.push("roll".into());
        let text = format_summary(&value);
        assert!(text.contains("Earlier") && text.contains("fact") && text.contains("thread"));
    }

    #[derive(Clone)]
    struct RecordingCompactor {
        carry_over: Arc<Mutex<Option<String>>>,
    }

    impl Compactor for RecordingCompactor {
        type Artifact = SummaryArtifact;

        fn compact<'a>(
            &'a self,
            _conversation_id: &'a str,
            _evicted: &'a [Message],
            carry_over: Option<&'a Self::Artifact>,
        ) -> WasmBoxedFuture<'a, Result<Self::Artifact, MemoryError>> {
            Box::pin(async move {
                *self.carry_over.lock().unwrap() =
                    carry_over.map(|artifact| artifact.0.prose.clone());
                Ok(SummaryArtifact(summary("new compacted context")))
            })
        }
    }

    #[tokio::test]
    async fn retry_compaction_carry_over_ignores_summaries_after_the_target() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json, default_branch_id) VALUES ('s', 'story', 'now', 'now', '{}', 'b')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO branches (id, story_id, parent_branch_id, forked_at_entry_id, name, created_at) VALUES ('b', 's', NULL, NULL, 'main', 'now')",
            [],
        )
        .unwrap();
        let first = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("Earlier narration"),
            &serde_json::json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let mut early_payload = serde_json::to_value(summary("safe context")).unwrap();
        early_payload["through_seq"] = serde_json::json!(first.seq);
        early_payload["through_entry_id"] = serde_json::json!(first.id);
        repository::append_entry(
            &conn,
            "b",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("safe context"),
            &early_payload,
            None,
        )
        .unwrap();
        let target = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("Retry target"),
            &serde_json::json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let mut late_payload = serde_json::to_value(summary("future leaked fact")).unwrap();
        late_payload["through_seq"] = serde_json::json!(target.seq);
        late_payload["through_entry_id"] = serde_json::json!(target.id);
        repository::append_entry(
            &conn,
            "b",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("future leaked fact"),
            &late_payload,
            None,
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            latest_summary_artifact(&pool, "b", None).unwrap().0.prose,
            "future leaked fact"
        );
        assert_eq!(
            latest_summary_artifact(&pool, "b", Some(target.seq))
                .unwrap()
                .0
                .prose,
            "safe context"
        );

        let carry_over = Arc::new(Mutex::new(None));
        let compactor = RecordingCompactor {
            carry_over: carry_over.clone(),
        };
        let mut history = vec![HistoryTurn {
            entry_id: None,
            is_player: false,
            content: "[Authoritative context summary]\nsafe context".into(),
        }];
        for index in 0..20 {
            history.push(HistoryTurn {
                entry_id: None,
                is_player: index % 2 == 0,
                content: format!("Long historical turn {index}: {}", "context ".repeat(40)),
            });
        }
        let config = TextModelConfig {
            provider: TextProviderKind::OpenRouter,
            model: "test".into(),
            api_key: "test".into(),
            context_window: 256,
        };
        let compacted = prepare_history_with_compactor(
            HistoryPreparation {
                pool: &pool,
                branch_id: "b",
                config: &config,
                preamble: "preamble",
                prompt: "prompt",
                before_seq: Some(target.seq),
            },
            history,
            &compactor,
        )
        .await;

        assert_eq!(carry_over.lock().unwrap().as_deref(), Some("safe context"));
        assert!(compacted[0].content.contains("new compacted context"));
        assert!(!compacted[0].content.contains("future leaked fact"));
    }

    #[tokio::test]
    async fn bounded_retry_compaction_at_tail_does_not_reorder_reconstructed_history() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json, default_branch_id) VALUES ('s', 'story', 'now', 'now', '{}', 'b')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO branches (id, story_id, parent_branch_id, forked_at_entry_id, name, created_at) VALUES ('b', 's', NULL, NULL, 'main', 'now')",
            [],
        )
        .unwrap();

        for index in 0..18 {
            repository::append_entry(
                &conn,
                "b",
                if index % 2 == 0 {
                    kind::PLAYER_MESSAGE
                } else {
                    kind::NARRATION
                },
                "visible",
                Some(&format!(
                    "Long historical turn {index}: {}",
                    "context ".repeat(40)
                )),
                &serde_json::json!({"input_mode": if index % 2 == 0 { "do" } else { "generated" }}),
                None,
            )
            .unwrap();
        }
        let player_before_target = repository::append_entry(
            &conn,
            "b",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("I open the sealed door."),
            &serde_json::json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        let target = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("The original door response."),
            &serde_json::json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        drop(conn);

        // This is the exact bounded read and compaction path used by
        // retry_narration before its replacement variant is appended.
        let history = load_history(&pool, "b", Some(target.seq)).unwrap();
        assert_eq!(
            history.last().and_then(|turn| turn.entry_id.as_deref()),
            Some(player_before_target.id.as_str())
        );
        let config = TextModelConfig {
            provider: TextProviderKind::OpenRouter,
            model: "test".into(),
            api_key: "test".into(),
            context_window: 256,
        };
        let compactor = RecordingCompactor {
            carry_over: Arc::new(Mutex::new(None)),
        };
        let compacted = prepare_history_with_compactor(
            HistoryPreparation {
                pool: &pool,
                branch_id: "b",
                config: &config,
                preamble: "preamble",
                prompt: "retry prompt",
                before_seq: Some(target.seq),
            },
            history,
            &compactor,
        )
        .await;
        assert!(compacted[0].content.contains("new compacted context"));

        let conn = pool.get().unwrap();
        let (summary_seq, summary_payload): (i64, String) = conn
            .query_row(
                "SELECT seq, payload_json FROM timeline_entries WHERE kind = ?1 ORDER BY seq DESC LIMIT 1",
                [kind::CONTEXT_SUMMARY],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let summary_payload: serde_json::Value = serde_json::from_str(&summary_payload).unwrap();
        assert!(summary_seq > target.seq);
        assert!(summary_payload["through_seq"].as_i64().unwrap() < target.seq);

        let intervening_player = repository::append_entry(
            &conn,
            "b",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("I step through."),
            &serde_json::json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        let intervening_narration = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("Dust rises beyond the threshold."),
            &serde_json::json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let variant = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION_VARIANT,
            "hidden",
            Some("The retried door response."),
            &serde_json::json!({"reason":"retry","input_mode":"generated"}),
            Some(&target.id),
        )
        .unwrap();
        let selection = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION_SELECTED,
            "hidden",
            None,
            &serde_json::json!({"selected_entry_id":variant.id,"reason":"retry"}),
            Some(&target.id),
        )
        .unwrap();
        assert!(
            target.seq < summary_seq
                && summary_seq < intervening_player.seq
                && intervening_player.seq < intervening_narration.seq
                && intervening_narration.seq < variant.seq
                && variant.seq < selection.seq
        );

        let raw = repository::list_logical_entries(&conn, "b").unwrap();
        let visible = reducer::active_visible_entries(&raw);
        let target_index = visible
            .iter()
            .position(|entry| entry.id == target.id)
            .unwrap();
        assert_eq!(
            visible[target_index].content.as_deref(),
            Some("The retried door response.")
        );
        assert_eq!(visible[target_index + 1].id, intervening_player.id);
        assert_eq!(visible[target_index + 2].id, intervening_narration.id);
        let variants = reducer::variants_for_entry(&raw, &target.id);
        assert_eq!(variants.len(), 2);
        assert!(variants
            .iter()
            .any(|item| item.id == variant.id && item.is_selected));
        drop(conn);

        let rebuilt = load_history(&pool, "b", None).unwrap();
        let rebuilt_text = rebuilt
            .iter()
            .map(|turn| turn.content.as_str())
            .collect::<Vec<_>>();
        let retry_index = rebuilt_text
            .iter()
            .position(|content| *content == "The retried door response.")
            .unwrap();
        let later_index = rebuilt_text
            .iter()
            .position(|content| *content == "I step through.")
            .unwrap();
        assert!(rebuilt[0].content.contains("new compacted context"));
        assert!(retry_index < later_index);
        assert_eq!(
            rebuilt_text
                .iter()
                .filter(|content| **content == "The retried door response.")
                .count(),
            1
        );
        assert!(!rebuilt_text.contains(&"The original door response."));

        let bounded = load_history(&pool, "b", Some(target.seq)).unwrap();
        assert!(!bounded
            .iter()
            .any(|turn| turn.content.contains("new compacted context")));
        assert_eq!(
            bounded.last().and_then(|turn| turn.entry_id.as_deref()),
            Some(player_before_target.id.as_str())
        );
    }
}
