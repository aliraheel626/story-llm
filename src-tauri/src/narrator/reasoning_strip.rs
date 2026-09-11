//! Best-effort removal of inline reasoning tags (`<think>`, `<thinking>`,
//! `<reasoning>`) that some models leak directly into visible content instead
//! of surfacing reasoning as a distinct field. This is a defensive second
//! layer: Rig already splits structured reasoning (e.g. OpenRouter's
//! `reasoning` field) into `StreamedAssistantContent::Reasoning` before this
//! ever runs. This layer only catches models that inline tags in plain text.

const PAIRS: [(&str, &str); 3] = [
    ("<think>", "</think>"),
    ("<thinking>", "</thinking>"),
    ("<reasoning>", "</reasoning>"),
];
const MAX_OPEN_LEN: usize = 11; // len("<thinking>")

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Scans `input` left to right, dropping any text inside a recognized tag
/// pair. When `hold_back_tail` is true (mid-stream), the last few bytes are
/// withheld in case they're the start of a tag split across a chunk boundary.
fn strip(input: &str, hold_back_tail: bool) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let mut earliest: Option<(usize, &str, &str)> = None;
        for (open, close) in PAIRS.iter() {
            if let Some(idx) = rest.find(open) {
                if earliest.is_none_or(|(e_idx, _, _)| idx < e_idx) {
                    earliest = Some((idx, open, close));
                }
            }
        }
        match earliest {
            None => {
                if hold_back_tail {
                    let safe_len = floor_char_boundary(rest, rest.len().saturating_sub(MAX_OPEN_LEN - 1));
                    out.push_str(&rest[..safe_len]);
                } else {
                    out.push_str(rest);
                }
                break;
            }
            Some((idx, open, close)) => {
                out.push_str(&rest[..idx]);
                let after_open = &rest[idx + open.len()..];
                match after_open.find(close) {
                    Some(cidx) => rest = &after_open[cidx + close.len()..],
                    None => break, // tag not yet closed; drop the remainder
                }
            }
        }
    }
    out
}

/// Stateful stripper for a live stream: feed it raw chunks as they arrive and
/// it returns the newly-visible delta to forward to the frontend.
#[derive(Default)]
pub struct ReasoningStripper {
    raw: String,
    emitted_visible_len: usize,
}

impl ReasoningStripper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> String {
        self.raw.push_str(chunk);
        let visible = strip(&self.raw, true);
        let delta = visible[self.emitted_visible_len..].to_string();
        self.emitted_visible_len = visible.len();
        delta
    }

    /// Call once the stream has ended to flush anything held back. Returns
    /// only the newly-visible tail, matching `push`'s delta semantics.
    pub fn finalize(&mut self) -> String {
        let visible = strip(&self.raw, false);
        let delta = visible[self.emitted_visible_len..].to_string();
        self.emitted_visible_len = visible.len();
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_complete_think_block() {
        let mut s = ReasoningStripper::new();
        let mut out = String::new();
        out.push_str(&s.push("Hello <think>secret plan</think> world"));
        out.push_str(&s.finalize());
        assert_eq!(out, "Hello  world");
    }

    #[test]
    fn handles_tag_split_across_chunks() {
        let mut s = ReasoningStripper::new();
        let mut out = String::new();
        out.push_str(&s.push("Hi <thi"));
        out.push_str(&s.push("nk>hidden</thi"));
        out.push_str(&s.push("nk> there"));
        out.push_str(&s.finalize());
        assert_eq!(out, "Hi  there");
    }

    #[test]
    fn passes_through_plain_text() {
        let mut s = ReasoningStripper::new();
        let mut out = String::new();
        out.push_str(&s.push("Just a normal "));
        out.push_str(&s.push("sentence."));
        out.push_str(&s.finalize());
        assert_eq!(out, "Just a normal sentence.");
    }
}
