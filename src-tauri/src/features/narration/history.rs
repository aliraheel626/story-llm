use std::collections::HashMap;

use crate::ai::HistoryTurn;
use crate::features::timeline::{model::kind, reducer, repository};
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

/// The most recent durable summary's `through_seq` for a branch, if one
/// exists — everything at or before it is superseded and doesn't need to be
/// refetched/redecoded on every turn.
fn latest_summary_through_seq(
    conn: &rusqlite::Connection,
    branch_id: &str,
    before_seq: Option<i64>,
) -> AppResult<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT seq, payload_json FROM timeline_entries WHERE branch_id = ?1 AND kind = ?2 ORDER BY seq DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![branch_id, kind::CONTEXT_SUMMARY], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (summary_seq, payload_json) = row?;
        let through_seq = serde_json::from_str::<serde_json::Value>(&payload_json)
            .ok()
            .and_then(|value| value.get("through_seq").and_then(|seq| seq.as_i64()));
        if let Some(through_seq) = through_seq {
            if before_seq
                .is_none_or(|before_seq| summary_seq < before_seq && through_seq < before_seq)
            {
                return Ok(Some(through_seq));
            }
        }
    }
    Ok(None)
}

/// Reconstructs model history from the logical branch timeline. The latest
/// durable summary replaces its covered prefix; later revisions and hidden
/// authoritative events are replayed in chronological order. When a summary
/// already exists, only entries from its boundary onward are even fetched —
/// a long, already-compacted branch doesn't reload and re-decode everything
/// before it on every turn.
pub(super) fn load_history(
    pool: &Pool,
    branch_id: &str,
    before_seq: Option<i64>,
) -> AppResult<Vec<HistoryTurn>> {
    let conn = pool.get()?;
    let since_seq = latest_summary_through_seq(&conn, branch_id, before_seq)?;
    let mut raw = match since_seq {
        Some(seq) => repository::list_logical_entries_since(&conn, branch_id, seq)?,
        None => repository::list_logical_entries(&conn, branch_id)?,
    };
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
        // CONTENT_EDITED/NARRATION_SELECTED are deliberately excluded here:
        // their effect is already folded into the primary turn above via
        // `active`, so surfacing them again would duplicate that same text
        // as a second, decontextualized "authoritative event" line.
        let contextual = matches!(
            entry.kind.as_str(),
            kind::DICEROLL
                | kind::ENTITY_CREATED
                | kind::ENTITY_UPDATED
                | kind::ENTITY_DELETED
                | kind::ENTITY_ATTRIBUTE_CHANGED
                | kind::ENTITY_ATTRIBUTE_REMOVED
                | kind::IMAGE_GENERATED
                | kind::CONTEXT_NOTE_UPDATED
                | kind::DICEROLL_SETTINGS_CHANGED
                | kind::WORLD_EVENT
        );
        if !contextual {
            continue;
        }
        let content = entry
            .content
            .clone()
            .unwrap_or_else(|| entry.payload.to_string());
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
    fn edited_and_reselected_narration_appears_only_once() {
        let mut narration = entry(
            "n1",
            0,
            kind::NARRATION,
            Some("original"),
            json!({"input_mode":"generated"}),
        );
        let mut edited = entry(
            "e1",
            1,
            kind::CONTENT_EDITED,
            Some("edited text"),
            json!({"reason":"user_edit","applies_to":"n1"}),
        );
        edited.target_entry_id = Some("n1".into());
        let mut selected = entry(
            "s1",
            2,
            kind::NARRATION_SELECTED,
            None,
            json!({"selected_entry_id":"n1"}),
        );
        selected.target_entry_id = Some("n1".into());
        narration.target_entry_id = None;

        let history = history_from_entries(&[narration, edited, selected]);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "edited text");
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
                "diceroll",
                3,
                kind::DICEROLL,
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

    #[test]
    fn retry_between_summaries_uses_the_older_summary_boundary() {
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

        let old_narration = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("The archive was entered."),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        repository::append_entry(
            &conn,
            "b",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("The party entered the archive."),
            &json!({"through_entry_id":old_narration.id,"through_seq":old_narration.seq}),
            None,
        )
        .unwrap();
        let intervening_player = repository::append_entry(
            &conn,
            "b",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("I inspect the sealed door."),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        let retry_target = repository::append_entry(
            &conn,
            "b",
            kind::NARRATION,
            "visible",
            Some("The seal begins to glow."),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        repository::append_entry(
            &conn,
            "b",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("The party reached the sealed door."),
            &json!({"through_entry_id":retry_target.id,"through_seq":retry_target.seq}),
            None,
        )
        .unwrap();
        // A retry can append a new compaction event after the entry being
        // retried even when that summary covers an earlier prefix. It must not
        // be selected because the before-seq cutoff would then remove the
        // summary event itself along with the target.
        repository::append_entry(
            &conn,
            "b",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("A later retry summarized through the player's action."),
            &json!({
                "through_entry_id": intervening_player.id,
                "through_seq": intervening_player.seq,
            }),
            None,
        )
        .unwrap();
        drop(conn);

        let history = load_history(&pool, "b", Some(retry_target.seq)).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0]
            .content
            .contains("The party entered the archive."));
        assert_eq!(history[1].content, "I inspect the sealed door.");
    }
}
