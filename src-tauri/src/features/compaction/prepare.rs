use rig_core::{completion::Message, memory::Compactor};
use rig_memory::{HeuristicTokenCounter, MemoryPolicy, TokenCounter, TokenWindowMemory};

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::ledger::{model::kind, repository, turn_tx::TurnTx};

use super::budget::{messages, raw_tail_boundary, FALLBACK_CONTEXT_WINDOW};
use super::compactor::NarratorCompactor;
use super::summary::{format_summary, latest_summary_artifact, SummaryArtifact};

pub(crate) struct PreparedHistory {
    pub turns: Vec<HistoryTurn>,
}

pub async fn prepare_history(
    turn: &TurnTx,
    story_id: &str,
    config: &TextModelConfig,
    preamble: &str,
    prompt: &str,
    history: Vec<HistoryTurn>,
) -> PreparedHistory {
    let compactor = NarratorCompactor::new(config.clone());
    let result = prepare_history_with_compactor(
        HistoryPreparation {
            turn,
            story_id,
            config,
            preamble,
            prompt,
        },
        history,
        &compactor,
    )
    .await;
    PreparedHistory {
        turns: result.turns,
    }
}

struct PreparationResult {
    turns: Vec<HistoryTurn>,
}

struct HistoryPreparation<'a> {
    turn: &'a TurnTx,
    story_id: &'a str,
    config: &'a TextModelConfig,
    preamble: &'a str,
    prompt: &'a str,
}

async fn prepare_history_with_compactor<C>(
    input: HistoryPreparation<'_>,
    history: Vec<HistoryTurn>,
    compactor: &C,
) -> PreparationResult
where
    C: Compactor<Artifact = SummaryArtifact>,
{
    let HistoryPreparation {
        turn,
        story_id,
        config,
        preamble,
        prompt,
    } = input;
    let context_window = if config.context_window == 0 {
        FALLBACK_CONTEXT_WINDOW
    } else {
        config.context_window
    };
    let response_reserve = 8_192usize.min(context_window / 5);
    let counter = HeuristicTokenCounter::openai();
    let fixed = counter.count(&Message::system(preamble.to_string()))
        + counter.count(&Message::user(prompt.to_string()))
        + response_reserve;
    let history_messages = messages(&history);
    let target_history_budget = (((context_window as f64) * 0.75) as usize).saturating_sub(fixed);
    let split = raw_tail_boundary(&history, config, preamble, prompt);
    if split == 0 {
        return PreparationResult { turns: history };
    }
    let keep_count = history.len() - split;
    let through_entry_id = history[..split]
        .iter()
        .rev()
        .find_map(|turn| turn.entry_id.clone());

    let carry_over = if history
        .first()
        .is_some_and(|turn| turn.marker == crate::ai::HistoryTurnMarker::Summary)
    {
        turn.with(|conn| Ok(latest_summary_artifact(conn, story_id)))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let evict_from = usize::from(carry_over.is_some());

    match compactor
        .compact(
            story_id,
            &history_messages[evict_from..split],
            carry_over.as_ref(),
        )
        .await
    {
        Ok(artifact) => {
            let summary_text = format_summary(&artifact.0);
            let _ = turn.with(|conn| {
                let boundary = through_entry_id
                    .as_deref()
                    .and_then(|id| repository::get_entry(conn, id).ok());
                if let Some(boundary) = boundary {
                    repository::append_entry(
                        conn,
                        story_id,
                        kind::CONTEXT_SUMMARY,
                        "hidden",
                        Some(&summary_text),
                        &serde_json::json!({"through_seq": boundary.seq, "through_entry_id": boundary.id, "prose": artifact.0.prose, "facts": artifact.0.facts,
                            "entity_notes": artifact.0.entity_notes, "open_threads": artifact.0.open_threads,
                            "unresolved_mechanics": artifact.0.unresolved_mechanics}),
                        None,
                        None,
                    )?;
                }
                Ok(())
            }).await;
            let mut compacted = vec![HistoryTurn {
                entry_id: None,
                is_player: false,
                content: format!("[Authoritative context summary]\n{summary_text}"),
                marker: crate::ai::HistoryTurnMarker::Summary,
            }];
            compacted.extend(history.into_iter().skip(split));
            PreparationResult { turns: compacted }
        }
        Err(_) => {
            if history
                .first()
                .is_some_and(|turn| turn.marker == crate::ai::HistoryTurnMarker::Summary)
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
                PreparationResult { turns: fallback }
            } else {
                let skip = history.len().saturating_sub(keep_count);
                PreparationResult {
                    turns: history.into_iter().skip(skip).collect(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rig_core::{
        completion::Message,
        memory::{Compactor, MemoryError},
        wasm_compat::WasmBoxedFuture,
    };

    use super::*;
    use crate::features::compaction::summary::ContextSummary;

    fn summary(prose: &str) -> ContextSummary {
        ContextSummary {
            prose: prose.into(),
            facts: Vec::new(),
            entity_notes: Vec::new(),
            open_threads: Vec::new(),
            unresolved_mechanics: Vec::new(),
        }
    }

    #[derive(Clone)]
    struct RecordingCompactor {
        carry_over: Arc<Mutex<Option<String>>>,
        evicted_count: Arc<Mutex<Option<usize>>>,
    }

    impl Compactor for RecordingCompactor {
        type Artifact = SummaryArtifact;

        fn compact<'a>(
            &'a self,
            _conversation_id: &'a str,
            evicted: &'a [Message],
            carry_over: Option<&'a Self::Artifact>,
        ) -> WasmBoxedFuture<'a, Result<Self::Artifact, MemoryError>> {
            Box::pin(async move {
                *self.carry_over.lock().unwrap() =
                    carry_over.map(|artifact| artifact.0.prose.clone());
                *self.evicted_count.lock().unwrap() = Some(evicted.len());
                Ok(SummaryArtifact(summary("new compacted context")))
            })
        }
    }

    #[tokio::test]
    async fn raw_tail_boundary_matches_the_prepare_history_split() {
        let history = (0..30)
            .map(|index| HistoryTurn {
                entry_id: Some(format!("entry-{index}")),
                is_player: index % 2 == 0,
                content: format!("Long historical turn {index}: {}", "context ".repeat(40)),
                marker: crate::ai::HistoryTurnMarker::Ledger,
            })
            .collect::<Vec<_>>();
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 256,
        };
        let expected = raw_tail_boundary(&history, &config, "preamble", "prompt");
        assert!(expected > 0);
        let evicted_count = Arc::new(Mutex::new(None));
        let compactor = RecordingCompactor {
            carry_over: Arc::new(Mutex::new(None)),
            evicted_count: evicted_count.clone(),
        };
        let pool = crate::shared::db::test_pool();
        let turn = TurnTx::begin(&pool, &Default::default(), "story").unwrap();

        let compacted = prepare_history_with_compactor(
            HistoryPreparation {
                turn: &turn,
                story_id: "story",
                config: &config,
                preamble: "preamble",
                prompt: "prompt",
            },
            history.clone(),
            &compactor,
        )
        .await;

        assert_eq!(*evicted_count.lock().unwrap(), Some(expected));
        assert_eq!(compacted.turns.len(), 1 + history.len() - expected);
    }

    #[tokio::test]
    async fn compacted_summary_is_part_of_the_turn_transaction() {
        let pool = crate::shared::db::test_pool();
        pool.get().unwrap().execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('story', 'Story', 'now', 'now', '{}')",
            [],
        ).unwrap();
        let turn = TurnTx::begin(&pool, &Default::default(), "story").unwrap();
        let history = turn
            .with(|conn| {
                (0..30)
                    .map(|index| {
                        let is_player = index % 2 == 0;
                        let content =
                            format!("Long historical turn {index}: {}", "context ".repeat(40));
                        let entry = repository::append_entry(
                            conn,
                            "story",
                            if is_player {
                                kind::PLAYER_MESSAGE
                            } else {
                                kind::NARRATION
                            },
                            "visible",
                            Some(&content),
                            &serde_json::json!({}),
                            None,
                            None,
                        )?;
                        Ok(HistoryTurn {
                            entry_id: Some(entry.id),
                            is_player,
                            content,
                            marker: crate::ai::HistoryTurnMarker::Ledger,
                        })
                    })
                    .collect::<crate::shared::error::AppResult<Vec<_>>>()
            })
            .await
            .unwrap();
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 256,
        };
        assert!(raw_tail_boundary(&history, &config, "preamble", "prompt") > 0);
        let compactor = RecordingCompactor {
            carry_over: Arc::new(Mutex::new(None)),
            evicted_count: Arc::new(Mutex::new(None)),
        };
        let result = prepare_history_with_compactor(
            HistoryPreparation {
                turn: &turn,
                story_id: "story",
                config: &config,
                preamble: "preamble",
                prompt: "prompt",
            },
            history,
            &compactor,
        )
        .await;
        assert!(result.turns[0].content.contains("new compacted context"));
        let summary_count = turn.with(|conn| Ok(conn.query_row(
            "SELECT COUNT(*) FROM ledger_entries WHERE story_id = 'story' AND kind = 'context_summary'",
            [], |row| row.get::<_, i64>(0),
        )?)).await.unwrap();
        assert_eq!(summary_count, 1);
        turn.rollback().await.unwrap();
        let committed_count: i64 = pool.get().unwrap().query_row(
            "SELECT COUNT(*) FROM ledger_entries WHERE story_id = 'story' AND kind = 'context_summary'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(committed_count, 0);
    }
}
