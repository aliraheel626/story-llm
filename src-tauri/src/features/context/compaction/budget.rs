use rig_core::completion::Message;
use rig_memory::{HeuristicTokenCounter, TokenCounter};

use crate::ai::{HistoryRole, HistoryTurn, TextModelConfig};

pub(super) const FALLBACK_CONTEXT_WINDOW: usize = 32_768;
pub(super) const RAW_TAIL_MESSAGES: usize = 12;
pub(super) const IMAGE_TOKEN_ESTIMATE: usize = 1_000;

// Compaction receives only visible text, never image bytes or private reasoning.
pub(super) fn messages(history: &[HistoryTurn]) -> Vec<Message> {
    history
        .iter()
        .map(|turn| match turn.role {
            HistoryRole::Player | HistoryRole::Record => Message::user(turn.content.clone()),
            HistoryRole::Narrator => Message::assistant(turn.content.clone()),
        })
        .collect()
}

fn request_cost(counter: &HeuristicTokenCounter, turn: &HistoryTurn, message: &Message) -> usize {
    counter.count(message)
        + turn
            .reasoning
            .as_ref()
            .map(|reasoning| counter.count(&Message::assistant(reasoning.clone())))
            .unwrap_or(0)
        + turn.images.len() * IMAGE_TOKEN_ESTIMATE
}

pub(super) fn kept_count(history: &[HistoryTurn], budget: usize) -> usize {
    let counter = HeuristicTokenCounter::openai();
    let mut used = 0usize;
    let mut kept = 0;
    for (turn, message) in history.iter().zip(messages(history)).rev() {
        let cost = request_cost(&counter, turn, &message);
        if used.saturating_add(cost) > budget {
            break;
        }
        used += cost;
        kept += 1;
    }
    kept
}

pub(crate) fn raw_tail_boundary(
    history: &[HistoryTurn],
    config: &TextModelConfig,
    preamble: &str,
    prompt: &str,
) -> usize {
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
    let history_messages = messages(history);
    let total = fixed
        + history
            .iter()
            .zip(&history_messages)
            .map(|(turn, message)| request_cost(&counter, turn, message))
            .sum::<usize>();
    if total < trigger {
        return 0;
    }

    let target_history_budget = (((context_window as f64) * 0.75) as usize).saturating_sub(fixed);
    let affordable = kept_count(history, target_history_budget);
    let minimum = RAW_TAIL_MESSAGES.min(history.len());
    let minimum_cost = history
        .iter()
        .zip(&history_messages)
        .rev()
        .take(minimum)
        .map(|(turn, message)| request_cost(&counter, turn, message))
        .sum::<usize>();
    let keep_count = if fixed.saturating_add(minimum_cost) < trigger {
        affordable.max(minimum)
    } else {
        affordable.max(1).min(history.len())
    };
    history.len().saturating_sub(keep_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{HistoryImage, HistoryTurnMarker};

    #[test]
    fn compactor_messages_are_text_only_and_images_cost_budget() {
        let history = vec![HistoryTurn {
            entry_id: None,
            role: HistoryRole::Record,
            content: "A record".into(),
            images: vec![HistoryImage {
                media_type: "image/png".into(),
                bytes: vec![1, 2, 3],
            }],
            reasoning: Some("not for compaction".into()),
            marker: HistoryTurnMarker::Ledger,
        }];
        let wire = serde_json::to_value(&messages(&history)[0]).unwrap();
        assert_eq!(wire["role"], "user");
        assert!(!wire.to_string().contains("not for compaction"));
        assert!(!wire.to_string().contains("image"));
        assert_eq!(kept_count(&history, IMAGE_TOKEN_ESTIMATE - 1), 0);
        assert_eq!(kept_count(&history, IMAGE_TOKEN_ESTIMATE + 100), 1);
    }

    #[test]
    fn reasoning_is_budgeted_but_absent_from_compactor_messages() {
        let turn = HistoryTurn {
            entry_id: None,
            role: HistoryRole::Narrator,
            content: "Short reply".into(),
            images: vec![],
            reasoning: Some("private thought ".repeat(500)),
            marker: HistoryTurnMarker::Ledger,
        };
        let text_cost = HeuristicTokenCounter::openai().count(&messages(&[turn.clone()])[0]);
        assert_eq!(kept_count(&[turn.clone()], text_cost + 1), 0);
        assert_eq!(
            kept_count(
                &[HistoryTurn {
                    reasoning: None,
                    ..turn.clone()
                }],
                text_cost + 1
            ),
            1
        );
        assert!(!serde_json::to_string(&messages(&[turn]))
            .unwrap()
            .contains("private thought"));
    }

    #[test]
    fn oversized_reasoning_can_reduce_the_raw_tail_below_twelve_messages() {
        let mut history = (0..19)
            .map(|_| HistoryTurn {
                entry_id: None,
                role: HistoryRole::Narrator,
                content: "Scene".into(),
                images: vec![],
                reasoning: Some("thinking ".repeat(300)),
                marker: HistoryTurnMarker::Ledger,
            })
            .collect::<Vec<_>>();
        history.push(HistoryTurn {
            entry_id: None,
            role: HistoryRole::Player,
            content: "<continue/>".into(),
            images: vec![],
            reasoning: None,
            marker: HistoryTurnMarker::Ledger,
        });
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "".into(),
            context_window: 6_000,
            supports_images: false,
        };
        let split = raw_tail_boundary(&history, &config, "preamble", "prompt");
        assert!(split > history.len() - RAW_TAIL_MESSAGES);
        assert_eq!(history[split..].last().unwrap().role, HistoryRole::Player);
    }
}
