use rig_core::completion::Message;
use rig_memory::{HeuristicTokenCounter, MemoryPolicy, TokenCounter, TokenWindowMemory};

use crate::ai::{HistoryTurn, TextModelConfig};

pub(super) const FALLBACK_CONTEXT_WINDOW: usize = 32_768;
pub(super) const RAW_TAIL_MESSAGES: usize = 12;

pub(super) fn messages(history: &[HistoryTurn]) -> Vec<Message> {
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
            .sum::<usize>();
    if total < trigger {
        return 0;
    }

    let target_history_budget = (((context_window as f64) * 0.75) as usize).saturating_sub(fixed);
    let policy = TokenWindowMemory::new(target_history_budget, counter);
    let kept = policy
        .apply(history_messages.clone())
        .unwrap_or(history_messages);
    let keep_count = kept.len().max(RAW_TAIL_MESSAGES.min(history.len()));
    history.len().saturating_sub(keep_count)
}
