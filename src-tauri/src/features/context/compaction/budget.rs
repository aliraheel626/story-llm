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

pub(super) fn kept_count(history: &[HistoryTurn], budget: usize) -> usize {
    let counter = HeuristicTokenCounter::openai();
    let mut used = 0usize;
    let mut kept = 0;
    for (turn, message) in history.iter().zip(messages(history)).rev() {
        let cost = counter.count(&message) + turn.images.len() * IMAGE_TOKEN_ESTIMATE;
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
        + history_messages
            .iter()
            .map(|message| counter.count(message))
            .sum::<usize>()
        + history
            .iter()
            .map(|turn| turn.images.len() * IMAGE_TOKEN_ESTIMATE)
            .sum::<usize>();
    if total < trigger {
        return 0;
    }

    let target_history_budget = (((context_window as f64) * 0.75) as usize).saturating_sub(fixed);
    let keep_count =
        kept_count(history, target_history_budget).max(RAW_TAIL_MESSAGES.min(history.len()));
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
}
