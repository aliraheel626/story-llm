use std::collections::HashMap;

use crate::ai::{HistoryTurn, HistoryTurnMarker};
use crate::features::compaction;
use crate::features::ledger::{model::kind, reducer, repository};
use crate::shared::error::AppResult;

/// Reconstructs model history from the story ledger. The latest
/// durable summary replaces its covered prefix; later revisions and hidden
/// authoritative events are replayed in chronological order. When a summary
/// already exists, only entries from its boundary onward are even fetched —
/// a long, already-compacted story doesn't reload and re-decode everything
/// before it on every turn.
pub fn load_transcript(conn: &rusqlite::Connection, story_id: &str) -> AppResult<Vec<HistoryTurn>> {
    let since_seq = compaction::boundary_for(conn, story_id)?.map(|boundary| boundary.through_seq);
    let raw = match since_seq {
        Some(seq) => repository::list_logical_entries_since(conn, story_id, seq)?,
        None => repository::list_logical_entries(conn, story_id)?,
    };
    Ok(history_from_entries(&raw))
}

fn history_from_entries(raw: &[crate::features::ledger::model::LedgerEntry]) -> Vec<HistoryTurn> {
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
            marker: HistoryTurnMarker::Summary,
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
            let is_player = visible.kind == kind::PLAYER_MESSAGE;
            let content = if is_player {
                crate::prompts::render_turn(mode, visible.content.as_deref().unwrap_or_default())
                    .unwrap_or_else(|| visible.content.clone().unwrap_or_default())
            } else {
                visible.content.clone().unwrap_or_default()
            };
            history.push(HistoryTurn {
                entry_id: Some(entry.id.clone()),
                is_player,
                content,
                marker: HistoryTurnMarker::Ledger,
            });
            continue;
        }
        // CONTENT_EDITED is deliberately excluded here:
        // their effect is already folded into the primary turn above via
        // `active`, so surfacing them again would duplicate that same text
        // as a second, decontextualized "authoritative event" line.
        if entry.kind == kind::ENTITY_CREATED
            && entry.payload.get("source").and_then(|value| value.as_str())
                == Some("story_bootstrap")
        {
            continue;
        }
        let contextual = matches!(
            entry.kind.as_str(),
            kind::ENTITY_CREATED
                | kind::ENTITY_QUERIED
                | kind::ENTITY_UPDATED
                | kind::ENTITY_DELETED
                | kind::ENTITY_ATTRIBUTE_CHANGED
                | kind::ENTITY_ATTRIBUTE_REMOVED
                | kind::IMAGE_GENERATED
                | kind::DICEROLL
        );
        if !contextual {
            continue;
        }
        let content = entry
            .content
            .clone()
            .unwrap_or_else(|| entry.payload.to_string());
        let content = format!("[Authoritative story event: {}]\n{content}", entry.kind);
        history.push(HistoryTurn {
            entry_id: Some(entry.id.clone()),
            is_player: false,
            content,
            marker: HistoryTurnMarker::Ledger,
        });
    }
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::model::LedgerEntry;
    use crate::prompts;
    use serde_json::json;

    fn entry(
        id: &str,
        seq: i64,
        event_kind: &str,
        content: Option<&str>,
        payload: serde_json::Value,
    ) -> LedgerEntry {
        LedgerEntry {
            id: id.into(),
            story_id: "s".into(),
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
            turn_id: None,
            created_at: "now".into(),
        }
    }

    #[test]
    fn legacy_author_note_events_are_inert() {
        let note = entry(
            "note",
            0,
            "context_note_updated",
            Some("Author's note was updated"),
            json!({"author_note":"old direction"}),
        );

        assert!(history_from_entries(&[note]).is_empty());
    }

    #[test]
    fn migrated_player_bootstrap_does_not_trail_narration_history() {
        let rows = vec![
            entry(
                "p1",
                0,
                kind::PLAYER_MESSAGE,
                Some("Open the door"),
                json!({"input_mode":"do"}),
            ),
            entry("n1", 1, kind::NARRATION, Some("It opens."), json!({})),
            entry(
                "bootstrap",
                2,
                kind::ENTITY_CREATED,
                Some("You was added as a character."),
                json!({"name":"You","source":"story_bootstrap"}),
            ),
        ];
        let history = history_from_entries(&rows);
        assert_eq!(history.len(), 2);
        assert_eq!(history.last().unwrap().entry_id.as_deref(), Some("n1"));
    }

    #[test]
    fn edited_narration_appears_only_once() {
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
            json!({"reason":"user_edit"}),
        );
        edited.target_entry_id = Some("n1".into());
        narration.target_entry_id = None;

        let history = history_from_entries(&[narration, edited]);
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
        assert_eq!(history[2].content, "<do>I take the key.</do>");
        assert!(!history
            .iter()
            .any(|turn| turn.content.contains("old narration")));
    }

    #[test]
    fn dice_rolls_are_always_contextual() {
        let rows = vec![
            entry("n1", 0, kind::NARRATION, Some("The door opens."), json!({})),
            entry(
                "diceroll",
                1,
                kind::DICEROLL,
                Some("Stealth succeeded."),
                json!({}),
            ),
            entry(
                "query",
                2,
                kind::ENTITY_QUERIED,
                Some("Looked up: Bob"),
                json!({}),
            ),
            entry(
                "p2",
                3,
                kind::PLAYER_MESSAGE,
                Some("I take the key."),
                json!({"input_mode":"do"}),
            ),
        ];

        let history = history_from_entries(&rows);
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].content, "The door opens.");
        assert_eq!(history[1].entry_id.as_deref(), Some("diceroll"));
        assert!(history[1].content.contains("Stealth succeeded."));
        assert!(history[2].content.contains("Looked up: Bob"));
        assert_eq!(history[3].content, "<do>I take the key.</do>");
    }

    #[test]
    fn load_transcript_ignores_legacy_dice_roll_preference() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let roll = repository::append_entry(
            &conn,
            "s",
            kind::DICEROLL,
            "hidden",
            Some("Stealth succeeded."),
            &json!({}),
            None,
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('context_injection', ?1)",
            [r#"{"entity_context_mode":"scoped","dice_rolls_in_context":false}"#],
        )
        .unwrap();
        drop(conn);
        let history = load_transcript(&pool.get().unwrap(), "s").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].entry_id.as_deref(), Some(roll.id.as_str()));
    }

    #[tokio::test]
    async fn load_transcript_reads_uncommitted_player_entry() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        ).unwrap();
        drop(conn);
        let turn = crate::features::ledger::turn_tx::TurnTx::begin(&pool, &Default::default(), "s")
            .unwrap();
        turn.with(|conn| {
            repository::append_entry(
                conn,
                "s",
                kind::PLAYER_MESSAGE,
                "visible",
                Some("I enter"),
                &json!({"input_mode":"do"}),
                None,
                None,
            )?;
            let history = load_transcript(conn, "s")?;
            assert_eq!(history.last().unwrap().content, "<do>I enter</do>");
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
        assert!(load_transcript(&pool.get().unwrap(), "s")
            .unwrap()
            .is_empty());
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
        assert_eq!(history[0].content, "<do>keep me</do>");
    }

    #[test]
    fn legacy_entity_queries_remain_contextual_but_tool_calls_do_not() {
        let rows = vec![
            entry(
                "query",
                0,
                kind::ENTITY_QUERIED,
                Some("Looked up: Bob"),
                json!({"entity_ids":["bob"]}),
            ),
            entry(
                "tool",
                1,
                kind::TOOL_CALL,
                Some("Checking who's here"),
                json!({"tool":"get_entities","args":{},"result":{"entities":[]},"ok":true}),
            ),
        ];

        let history = history_from_entries(&rows);
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].content,
            "[Authoritative story event: entity_queried]\nLooked up: Bob"
        );
    }

    #[test]
    fn history_uses_the_canonical_renderer_for_every_action_mode() {
        for mode in prompts::TURN_MODES {
            let content = if matches!(*mode, "see" | "continue") {
                ""
            } else {
                "content"
            };
            let row = entry(
                "action",
                0,
                kind::PLAYER_MESSAGE,
                Some(content),
                json!({"input_mode": mode}),
            );
            let history = history_from_entries(&[row]);
            assert_eq!(
                history[0].content,
                prompts::render_turn(mode, content).unwrap()
            );
        }
    }

    #[test]
    fn summary_with_a_cascade_deleted_boundary_does_not_hide_older_history() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let old = repository::append_entry(
            &conn,
            "s",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("Keep this older turn."),
            &json!({"input_mode":"do"}),
            None,
            None,
        )
        .unwrap();
        let narration = repository::append_entry(
            &conn,
            "s",
            kind::NARRATION,
            "visible",
            Some("Temporary narration."),
            &json!({"input_mode":"generated"}),
            None,
            None,
        )
        .unwrap();
        let query = repository::append_entry(
            &conn,
            "s",
            kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up: Bob"),
            &json!({"entity_ids":["bob"]}),
            Some(&narration.id),
            None,
        )
        .unwrap();
        repository::append_entry(
            &conn,
            "s",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("Invalid after the query is deleted."),
            &json!({"through_entry_id":query.id,"through_seq":query.seq}),
            None,
            None,
        )
        .unwrap();

        conn.execute("DELETE FROM ledger_entries WHERE id = ?1", [&narration.id])
            .unwrap();
        let query_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&query.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(query_count, 0);
        drop(conn);

        let history = load_transcript(&pool.get().unwrap(), "s").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].entry_id.as_deref(), Some(old.id.as_str()));
        assert_eq!(history[0].content, "<do>Keep this older turn.</do>");
    }
}
