use std::collections::HashMap;

use super::model::{kind, LedgerEntry, LedgerSnapshot};

fn edits_by_target(entries: &[LedgerEntry]) -> HashMap<String, String> {
    let mut edits = HashMap::new();
    for entry in entries {
        if entry.kind != kind::CONTENT_EDITED {
            continue;
        }
        if let (Some(target), Some(content)) = (&entry.target_entry_id, &entry.content) {
            edits.insert(target.clone(), content.clone());
        }
    }
    edits
}

pub fn active_visible_entries(entries: &[LedgerEntry]) -> Vec<LedgerEntry> {
    let edits = edits_by_target(entries);

    entries
        .iter()
        .filter(|e| e.kind == kind::PLAYER_MESSAGE || e.kind == kind::NARRATION)
        .map(|e| {
            let mut active = e.clone();
            if let Some(edited) = edits.get(&e.id) {
                active.content = Some(edited.clone());
            }
            active
        })
        .collect()
}

pub fn snapshot(entries: Vec<LedgerEntry>) -> LedgerSnapshot {
    LedgerSnapshot {
        visible: active_visible_entries(&entries),
        hidden: entries
            .into_iter()
            .filter(|entry| entry.visibility == "hidden")
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(
        id: &str,
        kind: &str,
        content: Option<&str>,
        target: Option<&str>,
        payload: serde_json::Value,
    ) -> LedgerEntry {
        LedgerEntry {
            id: id.into(),
            story_id: "s".into(),
            seq: 0,
            kind: kind.into(),
            visibility: "hidden".into(),
            content: content.map(str::to_string),
            payload,
            target_entry_id: target.map(str::to_string),
            created_at: "now".into(),
        }
    }

    #[test]
    fn edits_fold_without_mutating_original() {
        let rows = vec![
            entry("n", kind::NARRATION, Some("original"), None, json!({})),
            entry(
                "e",
                kind::CONTENT_EDITED,
                Some("edited"),
                Some("n"),
                json!({"reason":"user_edit"}),
            ),
        ];
        let visible = active_visible_entries(&rows);
        assert_eq!(visible[0].content.as_deref(), Some("edited"));
        assert_eq!(rows[0].content.as_deref(), Some("original"));
    }

    #[test]
    fn latest_edit_wins_for_player_and_narration() {
        let rows = vec![
            entry("p", kind::PLAYER_MESSAGE, Some("original"), None, json!({})),
            entry("n", kind::NARRATION, Some("first"), None, json!({})),
            entry(
                "e1",
                kind::CONTENT_EDITED,
                Some("second"),
                Some("n"),
                json!({}),
            ),
            entry(
                "e2",
                kind::CONTENT_EDITED,
                Some("third"),
                Some("n"),
                json!({}),
            ),
            entry(
                "ep",
                kind::CONTENT_EDITED,
                Some("player edit"),
                Some("p"),
                json!({}),
            ),
        ];
        let visible = active_visible_entries(&rows);
        assert_eq!(visible[0].content.as_deref(), Some("player edit"));
        assert_eq!(visible[1].content.as_deref(), Some("third"));
    }
}
