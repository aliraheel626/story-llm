use rig_core::{completion::Message, memory::Compactor};
use rig_memory::{HeuristicTokenCounter, TokenCounter};

use crate::ai::{CallUsage, HistoryRole, HistoryTurn, TextModelConfig};
use crate::features::transcript::summaries;
use crate::shared::error::AppResult;

use super::budget::{kept_count, messages, raw_tail_boundary, FALLBACK_CONTEXT_WINDOW};
use super::compactor::NarratorCompactor;
use super::summary::{format_summary, SummaryArtifact};

pub(crate) struct PreparedHistory {
    pub turns: Vec<HistoryTurn>,
    pub summary_write: Option<DeferredSummaryWrite>,
}

pub(crate) struct DeferredSummaryWrite {
    through_entry_id: Option<String>,
    summary_text: String,
    summary: super::summary::ContextSummary,
}

impl DeferredSummaryWrite {
    pub(crate) fn persist(&self, conn: &rusqlite::Connection, story_id: &str) -> AppResult<()> {
        summaries::append(
            conn,
            story_id,
            self.through_entry_id.as_deref(),
            &self.summary_text,
            &self.summary,
        )
    }
}

pub async fn prepare_history(
    story_id: &str,
    config: &TextModelConfig,
    preamble: &str,
    prompt: &str,
    history: Vec<HistoryTurn>,
    carry_over: Option<SummaryArtifact>,
) -> (PreparedHistory, Vec<CallUsage>) {
    let compactor = NarratorCompactor::new(config.clone());
    let prepared = prepare_history_with_compactor(
        HistoryPreparation {
            story_id,
            config,
            preamble,
            prompt,
        },
        history,
        carry_over,
        &compactor,
    )
    .await;
    let usage = std::mem::take(&mut *compactor.usage.lock().unwrap_or_else(|e| e.into_inner()));
    (prepared, usage)
}

struct HistoryPreparation<'a> {
    story_id: &'a str,
    config: &'a TextModelConfig,
    preamble: &'a str,
    prompt: &'a str,
}

async fn prepare_history_with_compactor<C>(
    input: HistoryPreparation<'_>,
    history: Vec<HistoryTurn>,
    carry_over: Option<SummaryArtifact>,
    compactor: &C,
) -> PreparedHistory
where
    C: Compactor<Artifact = SummaryArtifact>,
{
    let HistoryPreparation {
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
    let mut history = history;
    for turn in &mut history[..split] {
        turn.images.clear();
    }
    if split == 0 {
        return PreparedHistory {
            turns: history,
            summary_write: None,
        };
    }
    let keep_count = history.len() - split;
    let through_entry_id = history[..split]
        .iter()
        .rev()
        .find_map(|turn| turn.entry_id.clone());

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
            let summary_write = Some(DeferredSummaryWrite {
                through_entry_id,
                summary_text: summary_text.clone(),
                summary: artifact.0,
            });
            let mut compacted = vec![HistoryTurn {
                entry_id: None,
                role: HistoryRole::Narrator,
                content: format!("[Authoritative context summary]\n{summary_text}"),
                marker: crate::ai::HistoryTurnMarker::Summary,
                images: Vec::new(),
                reasoning: None,
            }];
            compacted.extend(history.into_iter().skip(split));
            PreparedHistory {
                turns: compacted,
                summary_write,
            }
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
                let recent_count = kept_count(&history[1..], remaining_budget);
                let mut fallback = vec![summary];
                let mut recent = history
                    .into_iter()
                    .skip(1)
                    .rev()
                    .take(recent_count)
                    .collect::<Vec<_>>();
                recent.reverse();
                fallback.extend(recent);
                PreparedHistory {
                    turns: fallback,
                    summary_write: None,
                }
            } else {
                let skip = history.len().saturating_sub(keep_count);
                PreparedHistory {
                    turns: history.into_iter().skip(skip).collect(),
                    summary_write: None,
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
    use crate::ai::HistoryImage;
    use crate::features::context::compaction::summary::ContextSummary;
    use crate::features::transcript::{model::kind, repository};
    use crate::features::turn::TurnTx;

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
                role: if index % 2 == 0 {
                    HistoryRole::Player
                } else {
                    HistoryRole::Narrator
                },
                content: format!("Long historical turn {index}: {}", "context ".repeat(40)),
                marker: crate::ai::HistoryTurnMarker::Transcript,
                images: Vec::new(),
                reasoning: None,
            })
            .collect::<Vec<_>>();
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 256,
            supports_images: false,
        };
        let expected = raw_tail_boundary(&history, &config, "preamble", "prompt");
        assert!(expected > 0);
        let evicted_count = Arc::new(Mutex::new(None));
        let compactor = RecordingCompactor {
            carry_over: Arc::new(Mutex::new(None)),
            evicted_count: evicted_count.clone(),
        };
        let compacted = prepare_history_with_compactor(
            HistoryPreparation {
                story_id: "story",
                config: &config,
                preamble: "preamble",
                prompt: "prompt",
            },
            history.clone(),
            None,
            &compactor,
        )
        .await;

        assert_eq!(*evicted_count.lock().unwrap(), Some(expected));
        assert_eq!(compacted.turns.len(), 1 + history.len() - expected);
    }

    #[tokio::test]
    async fn compaction_removes_image_bytes_from_the_compacted_prefix() {
        let history = (0..30)
            .map(|index| HistoryTurn {
                entry_id: Some(format!("entry-{index}")),
                role: HistoryRole::Record,
                content: format!("image event {index}: {}", "context ".repeat(40)),
                marker: crate::ai::HistoryTurnMarker::Transcript,
                images: vec![HistoryImage {
                    media_type: "image/png".into(),
                    bytes: vec![index as u8],
                }],
                reasoning: None,
            })
            .collect::<Vec<_>>();
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 256,
            supports_images: true,
        };
        let split = raw_tail_boundary(&history, &config, "preamble", "prompt");
        assert!(split > 0);
        let compactor = RecordingCompactor {
            carry_over: Arc::new(Mutex::new(None)),
            evicted_count: Arc::new(Mutex::new(None)),
        };
        let result = prepare_history_with_compactor(
            HistoryPreparation {
                story_id: "story",
                config: &config,
                preamble: "preamble",
                prompt: "prompt",
            },
            history.clone(),
            None,
            &compactor,
        )
        .await;
        assert_eq!(result.turns[0].images.len(), 0);
        assert_eq!(
            result.turns[1].images[0].bytes,
            history[split].images[0].bytes
        );
        assert_eq!(
            result.turns.iter().flat_map(|turn| &turn.images).count(),
            history.len() - split
        );
    }

    #[tokio::test]
    async fn compacted_summary_is_part_of_the_turn_transaction() {
        let pool = crate::shared::db::test_pool();
        crate::shared::test_support::story(&pool.get().unwrap(), "story");
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
                            role: if is_player {
                                HistoryRole::Player
                            } else {
                                HistoryRole::Narrator
                            },
                            content,
                            marker: crate::ai::HistoryTurnMarker::Transcript,
                            images: Vec::new(),
                            reasoning: None,
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
            supports_images: false,
        };
        let expected_boundary = raw_tail_boundary(&history, &config, "preamble", "prompt");
        assert!(expected_boundary > 0);
        let boundary_id = history[expected_boundary - 1].entry_id.clone().unwrap();
        let boundary_seq = turn
            .with(|conn| Ok(repository::get_entry(conn, &boundary_id)?.seq))
            .await
            .unwrap();
        let compactor = RecordingCompactor {
            carry_over: Arc::new(Mutex::new(None)),
            evicted_count: Arc::new(Mutex::new(None)),
        };
        let result = prepare_history_with_compactor(
            HistoryPreparation {
                story_id: "story",
                config: &config,
                preamble: "preamble",
                prompt: "prompt",
            },
            history,
            None,
            &compactor,
        )
        .await;
        turn.with(|conn| {
            result
                .summary_write
                .as_ref()
                .unwrap()
                .persist(conn, "story")
        })
        .await
        .unwrap();
        assert!(result.turns[0].content.contains("new compacted context"));
        let summary_count = turn.with(|conn| Ok(conn.query_row(
            "SELECT COUNT(*) FROM transcript_entries WHERE story_id = 'story' AND kind = 'context_summary'",
            [], |row| row.get::<_, i64>(0),
        )?)).await.unwrap();
        assert_eq!(summary_count, 1);
        let payload: String = turn.with(|conn| Ok(conn.query_row(
            "SELECT payload_json FROM transcript_entries WHERE story_id = 'story' AND kind = 'context_summary'",
            [], |row| row.get(0),
        )?)).await.unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["prose"], "new compacted context");
        assert_eq!(payload["through_seq"], boundary_seq);
        assert_eq!(payload["through_entry_id"], boundary_id);
        assert_eq!(payload["facts"], serde_json::json!([]));
        turn.rollback().await.unwrap();
        let committed_count: i64 = pool.get().unwrap().query_row(
            "SELECT COUNT(*) FROM transcript_entries WHERE story_id = 'story' AND kind = 'context_summary'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(committed_count, 0);
    }
}
