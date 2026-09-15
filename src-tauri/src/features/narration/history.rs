use std::collections::HashMap;

use crate::ai::HistoryTurn;
use crate::features::timeline::{model::kind, reducer, repository};
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

/// Reconstructs model history from the logical branch timeline. The latest
/// durable summary replaces its covered prefix; later revisions and hidden
/// authoritative events are replayed in chronological order.
pub(super) fn load_history(
    pool: &Pool,
    branch_id: &str,
    before_seq: Option<i64>,
) -> AppResult<Vec<HistoryTurn>> {
    let conn = pool.get()?;
    let mut raw = repository::list_logical_entries(&conn, branch_id)?;
    if let Some(seq) = before_seq {
        raw.retain(|entry| entry.branch_id != branch_id || entry.seq < seq);
    }
    Ok(history_from_entries(&raw))
}

fn history_from_entries(
    raw: &[crate::features::timeline::model::TimelineEntry],
) -> Vec<HistoryTurn> {
    let active: HashMap<String, _> = reducer::active_visible_entries(raw)
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    let summary_boundary = raw
        .iter()
        .enumerate()
        .rev()
        .find_map(|(summary_index, entry)| {
            if entry.kind != kind::CONTEXT_SUMMARY {
                return None;
            }
            let through_id = entry.payload.get("through_entry_id")?.as_str()?;
            let covered_index = raw
                .iter()
                .position(|candidate| candidate.id == through_id)?;
            (covered_index < summary_index).then_some((summary_index, covered_index))
        });
    let summary_index = summary_boundary.map(|(summary, _)| summary);
    let covered_index = summary_boundary.map(|(_, covered)| covered);
    let mut history = Vec::new();
    if let Some(index) = summary_index {
        history.push(HistoryTurn {
            entry_id: None,
            is_player: false,
            content: format!(
                "[Authoritative context summary]\n{}",
                raw[index].content.as_deref().unwrap_or_default()
            ),
        });
    }

    for (index, entry) in raw.iter().enumerate() {
        if Some(index) <= covered_index || entry.kind == kind::CONTEXT_SUMMARY {
            continue;
        }
        if let Some(visible) = active.get(&entry.id) {
            let mode = visible
                .payload
                .get("input_mode")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if mode == "story"
                && raw.iter().skip(index + 1).any(|later| {
                    later.payload.get("input_mode").and_then(|v| v.as_str())
                        == Some("generated_story")
                })
            {
                continue;
            }
            history.push(HistoryTurn {
                entry_id: Some(entry.id.clone()),
                is_player: visible.kind == kind::PLAYER_MESSAGE,
                content: visible.content.clone().unwrap_or_default(),
            });
            continue;
        }
        let contextual = matches!(
            entry.kind.as_str(),
            kind::CONTENT_EDITED
                | kind::NARRATION_SELECTED
                | kind::MECHANICAL_RESULT
                | kind::ENTITY_CREATED
                | kind::ENTITY_UPDATED
                | kind::ENTITY_DELETED
                | kind::ENTITY_ATTRIBUTE_CHANGED
                | kind::ENTITY_ATTRIBUTE_REMOVED
                | kind::IMAGE_GENERATED
                | kind::CONTEXT_NOTE_UPDATED
                | kind::MECHANICS_SETTINGS_CHANGED
                | kind::WORLD_EVENT
        );
        if !contextual {
            continue;
        }
        let content = if entry.kind == kind::NARRATION_SELECTED {
            entry
                .payload
                .get("selected_entry_id")
                .and_then(|v| v.as_str())
                .and_then(|id| raw.iter().find(|candidate| candidate.id == id))
                .and_then(|candidate| candidate.content.clone())
                .unwrap_or_default()
        } else {
            entry
                .content
                .clone()
                .unwrap_or_else(|| entry.payload.to_string())
        };
        history.push(HistoryTurn {
            entry_id: Some(entry.id.clone()),
            is_player: false,
            content: format!("[Authoritative story event: {}]\n{content}", entry.kind),
        });
    }
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::timeline::model::TimelineEntry;
    use serde_json::json;

    fn entry(
        id: &str,
        seq: i64,
        event_kind: &str,
        content: Option<&str>,
        payload: serde_json::Value,
    ) -> TimelineEntry {
        TimelineEntry {
            id: id.into(),
            branch_id: "b".into(),
            seq,
            kind: event_kind.into(),
            visibility: if matches!(event_kind, kind::PLAYER_MESSAGE | kind::NARRATION) {
                "visible"
            } else {
                "hidden"
            }
            .into(),
            content: content.map(str::to_string),
            payload,
            target_entry_id: None,
            created_at: "now".into(),
        }
    }

    #[test]
    fn newest_summary_replaces_covered_prefix_and_keeps_later_events() {
        let rows = vec![
            entry(
                "p1",
                0,
                kind::PLAYER_MESSAGE,
                Some("old action"),
                json!({"input_mode":"do"}),
            ),
            entry(
                "n1",
                1,
                kind::NARRATION,
                Some("old narration"),
                json!({"input_mode":"generated"}),
            ),
            entry(
                "sum",
                2,
                kind::CONTEXT_SUMMARY,
                Some("The archive was entered."),
                json!({"through_entry_id":"n1"}),
            ),
            entry(
                "mechanic",
                3,
                kind::MECHANICAL_RESULT,
                Some("Stealth succeeded."),
                json!({}),
            ),
            entry(
                "p2",
                4,
                kind::PLAYER_MESSAGE,
                Some("I take the key."),
                json!({"input_mode":"do"}),
            ),
        ];

        let history = history_from_entries(&rows);
        assert_eq!(history.len(), 3);
        assert!(history[0].content.contains("The archive was entered."));
        assert!(history[1].content.contains("Stealth succeeded."));
        assert_eq!(history[2].content, "I take the key.");
        assert!(!history
            .iter()
            .any(|turn| turn.content.contains("old narration")));
    }

    #[test]
    fn invalid_summary_boundary_is_ignored() {
        let rows = vec![
            entry(
                "p1",
                0,
                kind::PLAYER_MESSAGE,
                Some("keep me"),
                json!({"input_mode":"do"}),
            ),
            entry(
                "sum",
                1,
                kind::CONTEXT_SUMMARY,
                Some("bad summary"),
                json!({"through_entry_id":"missing"}),
            ),
        ];
        let history = history_from_entries(&rows);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "keep me");
    }
}
