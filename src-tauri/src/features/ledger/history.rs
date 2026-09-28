use std::collections::HashMap;

use crate::ai::{HistoryImage, HistoryRole, HistoryTurn, HistoryTurnMarker};
use crate::features::ledger::{attachments, model::kind, query, reducer, summaries};
use crate::shared::error::AppResult;

use super::filter::TranscriptSettings;

pub const MAX_CONTEXT_IMAGES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePolicy {
    Allowed,
    Unsupported,
}

/// Reconstructs model history from the story's transcript. The latest
/// durable summary replaces its covered prefix; later revisions and hidden
/// authoritative events are replayed in chronological order. When a summary
/// already exists, only entries from its boundary onward are even fetched —
/// a long, already-compacted story doesn't reload and re-decode everything
/// before it on every turn.
pub fn for_model(
    conn: &rusqlite::Connection,
    story_id: &str,
    settings: &TranscriptSettings,
    images: ImagePolicy,
) -> AppResult<Vec<HistoryTurn>> {
    let boundary = summaries::latest_boundary(conn, story_id)?;
    let since_seq = boundary.as_ref().map(|boundary| boundary.through_seq);
    let mut kinds = vec![kind::PLAYER_MESSAGE, kind::NARRATION, kind::CONTEXT_SUMMARY];
    kinds.extend(
        kind::RECORD_KINDS
            .iter()
            .copied()
            .filter(|kind| settings.includes(&format!("record.{kind}"))),
    );
    let modes = crate::prompts::TURN_MODES
        .iter()
        .copied()
        .filter(|mode| settings.includes(&format!("action.{mode}")))
        .collect::<Vec<_>>();
    let raw = query::select(
        conn,
        story_id,
        &query::LedgerQuery {
            since_seq,
            kinds: Some(&kinds),
            input_modes: Some(&modes),
            ..Default::default()
        },
    )?;
    let image_rows = if settings.includes("images") && images == ImagePolicy::Allowed {
        attachments::images_for_entries(conn, story_id, since_seq, MAX_CONTEXT_IMAGES)?
    } else {
        Vec::new()
    };
    let mut image_map: HashMap<String, Vec<HistoryImage>> = HashMap::new();
    for (entry_id, media_type, bytes) in image_rows {
        image_map
            .entry(entry_id)
            .or_default()
            .push(HistoryImage { media_type, bytes });
    }
    Ok(history_from_entries_with_settings(
        &raw,
        settings,
        boundary
            .as_ref()
            .map(|boundary| (boundary.summary_entry_id.as_str(), boundary.through_seq)),
        &mut image_map,
    ))
}

#[cfg(test)]
fn history_from_entries(raw: &[crate::features::ledger::model::LedgerEntry]) -> Vec<HistoryTurn> {
    history_from_entries_with_settings(raw, &TranscriptSettings::default(), None, &mut HashMap::new())
}

fn history_from_entries_with_settings(
    raw: &[crate::features::ledger::model::LedgerEntry],
    settings: &TranscriptSettings,
    boundary: Option<(&str, i64)>,
    images: &mut HashMap<String, Vec<HistoryImage>>,
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
            if boundary.is_some_and(|(id, _)| entry.id != id) {
                return None;
            }
            let through_id = entry.payload.get("through_entry_id")?.as_str()?;
            let covered_seq = boundary.map(|(_, seq)| seq).or_else(|| {
                raw.iter()
                    .find(|candidate| candidate.id == through_id)
                    .map(|candidate| candidate.seq)
            })?;
            (covered_seq < entry.seq).then_some((summary_index, covered_seq))
        });
    let summary_index = summary_boundary.map(|(summary, _)| summary);
    let covered_seq = summary_boundary.map(|(_, covered)| covered);
    let mut history = Vec::new();
    if let Some(index) = summary_index {
        history.push(HistoryTurn {
            entry_id: None,
            role: HistoryRole::Narrator,
            content: format!(
                "[Authoritative context summary]\n{}",
                raw[index].content.as_deref().unwrap_or_default()
            ),
            marker: HistoryTurnMarker::Summary,
            images: Vec::new(),
            reasoning: None,
        });
    }

    for entry in raw {
        if covered_seq.is_some_and(|seq| entry.seq <= seq) || entry.kind == kind::CONTEXT_SUMMARY {
            continue;
        }
        if let Some(visible) = active.get(&entry.id) {
            if visible.kind != kind::NARRATION || settings.includes("narration") {
                let mode = visible
                    .payload
                    .get("input_mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let is_player = visible.kind == kind::PLAYER_MESSAGE;
                let content = if is_player {
                    crate::prompts::render_turn(
                        mode,
                        visible.content.as_deref().unwrap_or_default(),
                    )
                    .unwrap_or_else(|| visible.content.clone().unwrap_or_default())
                } else {
                    visible.content.clone().unwrap_or_default()
                };
                history.push(HistoryTurn {
                    entry_id: Some(entry.id.clone()),
                    role: if is_player {
                        HistoryRole::Player
                    } else {
                        HistoryRole::Narrator
                    },
                    content,
                    marker: HistoryTurnMarker::Ledger,
                    images: Vec::new(),
                    reasoning: (visible.kind == kind::NARRATION
                        && settings.includes("narration.thoughts"))
                    .then(|| {
                        visible
                            .payload
                            .get("thoughts")
                            .and_then(|value| value.as_str())
                            .map(str::to_string)
                    })
                    .flatten()
                    .filter(|thoughts| !thoughts.is_empty()),
                });
            }
            if visible.kind == kind::NARRATION {
                for image in images.remove(&entry.id).unwrap_or_default() {
                    history.push(HistoryTurn {
                        entry_id: Some(entry.id.clone()),
                        role: HistoryRole::Record,
                        content: "[Authoritative story event: image]".into(),
                        marker: HistoryTurnMarker::Ledger,
                        images: vec![image],
                        reasoning: None,
                    });
                }
            }
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
        if !kind::RECORD_KINDS.contains(&entry.kind.as_str())
            || !settings.includes(&format!("record.{}", entry.kind))
        {
            continue;
        }
        let content = if entry.kind == kind::TOOL_CALL {
            let tool = entry
                .payload
                .get("tool")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let compact = |key| {
                let value = entry
                    .payload
                    .get(key)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
                    .to_string();
                value.chars().take(500).collect::<String>()
            };
            format!("{}({}) → {}", tool, compact("args"), compact("result"))
        } else {
            entry
                .content
                .clone()
                .unwrap_or_else(|| entry.payload.to_string())
        };
        let content = format!("[Authoritative story event: {}]\n{content}", entry.kind);
        history.push(HistoryTurn {
            entry_id: Some(entry.id.clone()),
            role: HistoryRole::Record,
            content,
            marker: HistoryTurnMarker::Ledger,
            images: Vec::new(),
            reasoning: None,
        });
    }
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::model::LedgerEntry;
    use crate::features::ledger::repository;
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
        assert_eq!(history[0].role, HistoryRole::Narrator);
        assert!(history[0].content.contains("The archive was entered."));
        assert_eq!(history[1].role, HistoryRole::Record);
        assert!(history[1].content.contains("Stealth succeeded."));
        assert_eq!(history[2].role, HistoryRole::Player);
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
    fn player_narration_and_dice_roll_have_distinct_history_roles() {
        let rows = vec![
            entry(
                "player",
                0,
                kind::PLAYER_MESSAGE,
                Some("Open the door"),
                json!({"input_mode":"do"}),
            ),
            entry(
                "narration",
                1,
                kind::NARRATION,
                Some("It opens."),
                json!({}),
            ),
            entry(
                "roll",
                2,
                kind::DICEROLL,
                Some("Stealth succeeded."),
                json!({}),
            ),
        ];
        let history = history_from_entries(&rows);
        assert_eq!(
            history.iter().map(|turn| turn.role).collect::<Vec<_>>(),
            [
                HistoryRole::Player,
                HistoryRole::Narrator,
                HistoryRole::Record,
            ]
        );
        assert_eq!(
            history[2].content,
            "[Authoritative story event: diceroll]\nStealth succeeded."
        );
    }

    #[test]
    fn for_model_ignores_legacy_dice_roll_preference() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        let roll_id = crate::shared::test_support::record(
            &conn, "s", kind::DICEROLL, Some("Stealth succeeded."), json!({}), None, None,
        );
        let roll = repository::get_entry(&conn, &roll_id).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('context_injection', ?1)",
            [r#"{"entity_context_mode":"scoped","dice_rolls_in_context":false}"#],
        )
        .unwrap();
        drop(conn);
        let history = for_model(
            &pool.get().unwrap(),
            "s",
            &TranscriptSettings::default(),
            ImagePolicy::Unsupported,
        )
        .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].entry_id.as_deref(), Some(roll.id.as_str()));
    }

    #[tokio::test]
    async fn for_model_reads_uncommitted_player_entry() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        drop(conn);
        let turn = crate::features::turn::TurnTx::begin(&pool, &Default::default(), "s").unwrap();
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
            let history = for_model(
                conn,
                "s",
                &TranscriptSettings::default(),
                ImagePolicy::Unsupported,
            )?;
            assert_eq!(history.last().unwrap().content, "<do>I enter</do>");
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
        assert!(for_model(
            &pool.get().unwrap(),
            "s",
            &TranscriptSettings::default(),
            ImagePolicy::Unsupported
        )
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
        crate::shared::test_support::story(&conn, "s");
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

        let history = for_model(
            &pool.get().unwrap(),
            "s",
            &TranscriptSettings::default(),
            ImagePolicy::Unsupported,
        )
        .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].entry_id.as_deref(), Some(old.id.as_str()));
        assert_eq!(history[0].content, "<do>Keep this older turn.</do>");
    }

    #[test]
    fn settings_select_actions_narration_thoughts_and_records_independently() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        let append = |kind, content, payload| {
            crate::shared::test_support::record(&conn, "s", kind, content, payload, None, None)
        };
        append(
            kind::PLAYER_MESSAGE,
            Some("first"),
            json!({"input_mode":"do"}),
        );
        append(kind::PLAYER_MESSAGE, Some(""), json!({"input_mode":"see"}));
        append(
            kind::NARRATION,
            Some("scene"),
            json!({"thoughts":"hidden thought"}),
        );
        append(kind::DICEROLL, Some("roll"), json!({}));
        append(kind::IMAGE_GENERATED, Some("prompt"), json!({}));
        append(
            kind::ENTITY_CREATED,
            Some("bootstrap"),
            json!({"source":"story_bootstrap"}),
        );
        append(
            kind::TOOL_CALL,
            Some("display label"),
            json!({"tool":"find","args":{"name":"é"},"result":[1]}),
        );
        let defaults = TranscriptSettings::default();
        let load = |settings: &TranscriptSettings| {
            for_model(&conn, "s", settings, ImagePolicy::Unsupported).unwrap()
        };
        let history = load(&defaults);
        assert_eq!(
            history
                .iter()
                .map(|turn| turn.content.as_str())
                .collect::<Vec<_>>(),
            [
                "<do>first</do>",
                "scene",
                "[Authoritative story event: diceroll]\nroll"
            ]
        );
        assert!(history[1].reasoning.is_none());

        let mut settings = defaults.clone();
        settings.include.insert("action.do".into(), false);
        settings.include.insert("action.see".into(), true);
        settings.include.insert("narration".into(), false);
        settings.include.insert("narration.thoughts".into(), true);
        settings.include.insert("record.diceroll".into(), false);
        settings
            .include
            .insert("record.image_generated".into(), true);
        settings.include.insert("record.tool_call".into(), true);
        let history = load(&settings);
        assert_eq!(history[0].content, "<see/>");
        assert!(!history.iter().any(|turn| turn.role == HistoryRole::Narrator));
        assert!(history[1].content.contains("image_generated]\nprompt"));
        assert_eq!(
            history[2].content,
            "[Authoritative story event: tool_call]\nfind({\"name\":\"é\"}) → [1]"
        );
        assert!(!history
            .iter()
            .any(|turn| turn.content.contains("bootstrap")));
        settings.include.insert("narration".into(), true);
        let history = load(&settings);
        assert_eq!(history[1].content, "scene");
        assert_eq!(history[1].reasoning.as_deref(), Some("hidden thought"));
        settings.include.insert("narration.thoughts".into(), false);
        assert!(load(&settings)[1].reasoning.is_none());
    }

    #[test]
    fn tool_arguments_and_results_are_capped_by_unicode_characters() {
        let mut settings = TranscriptSettings::default();
        settings.include.insert("record.tool_call".into(), true);
        let row = entry(
            "tool",
            0,
            kind::TOOL_CALL,
            None,
            json!({"tool":"sample","args":"é".repeat(600),"result":"😀".repeat(600)}),
        );
        let history =
            history_from_entries_with_settings(&[row], &settings, None, &mut HashMap::new());
        let content = &history[0].content;
        assert!(content.contains(&"é".repeat(499)));
        assert!(content.contains(&"😀".repeat(499)));
        assert!(!content.contains(&"é".repeat(501)));
        assert!(!content.contains(&"😀".repeat(501)));
    }

    #[test]
    fn images_are_latest_four_after_the_summary_and_ignore_unsupported_mime() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        let old = repository::append_entry(
            &conn,
            "s",
            kind::NARRATION,
            "visible",
            Some("old"),
            &json!({}),
            None,
            None,
        )
        .unwrap();
        let attach = |entry_id: &str, id: &str, mime: &str| {
            attachments::insert_image(
                &conn,
                &crate::features::ledger::model::StoryImage {
                    id: id.into(),
                    entry_id: entry_id.into(),
                    prompt: "prompt".into(),
                    created_at: id.into(),
                },
                mime,
                id.as_bytes(),
            )
            .unwrap();
        };
        attach(&old.id, "old", "image/png");
        repository::append_entry(
            &conn,
            "s",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("prior"),
            &json!({"through_entry_id":old.id,"through_seq":old.seq}),
            None,
            None,
        )
        .unwrap();
        for index in 0..6 {
            let narration = repository::append_entry(
                &conn,
                "s",
                kind::NARRATION,
                "visible",
                Some(&format!("scene {index}")),
                &json!({}),
                None,
                None,
            )
            .unwrap();
            attach(&narration.id, &format!("image-{index}"), "image/png");
        }
        let last = repository::append_entry(
            &conn,
            "s",
            kind::NARRATION,
            "visible",
            Some("bad"),
            &json!({}),
            None,
            None,
        )
        .unwrap();
        attach(&last.id, "bad", "application/octet-stream");
        let mut settings = TranscriptSettings::default();
        settings.include.insert("images".into(), true);
        let allowed = for_model(&conn, "s", &settings, ImagePolicy::Allowed).unwrap();
        let images = allowed
            .iter()
            .filter(|turn| !turn.images.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(images.len(), MAX_CONTEXT_IMAGES);
        assert_eq!(
            images
                .iter()
                .map(|turn| turn.images[0].bytes.as_slice())
                .collect::<Vec<_>>(),
            [b"image-2".as_slice(), b"image-3", b"image-4", b"image-5"]
        );
        for image in images {
            let at = allowed
                .iter()
                .position(|turn| std::ptr::eq(turn, image))
                .unwrap();
            assert_eq!(allowed[at - 1].entry_id, image.entry_id);
            assert_eq!(image.content, "[Authoritative story event: image]");
        }
        assert!(allowed[0].content.contains("prior"));
        assert!(
            for_model(&conn, "s", &settings, ImagePolicy::Unsupported)
                .unwrap()
                .iter()
                .all(|turn| turn.images.is_empty())
        );
        settings.include.insert("narration".into(), false);
        assert_eq!(
            for_model(&conn, "s", &settings, ImagePolicy::Allowed)
                .unwrap()
                .iter()
                .filter(|turn| !turn.images.is_empty())
                .count(),
            4
        );
        settings.include.insert("images".into(), false);
        assert!(for_model(&conn, "s", &settings, ImagePolicy::Allowed)
            .unwrap()
            .iter()
            .all(|turn| turn.images.is_empty()));
    }

    #[test]
    fn summary_survives_when_its_covered_action_is_filtered_out() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        let action = repository::append_entry(
            &conn,
            "s",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("old"),
            &json!({"input_mode":"do"}),
            None,
            None,
        )
        .unwrap();
        repository::append_entry(
            &conn,
            "s",
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("summary"),
            &json!({"through_entry_id":action.id,"through_seq":action.seq}),
            None,
            None,
        )
        .unwrap();
        repository::append_entry(
            &conn,
            "s",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("new"),
            &json!({"input_mode":"say"}),
            None,
            None,
        )
        .unwrap();
        let mut settings = TranscriptSettings::default();
        settings.include.insert("action.do".into(), false);
        let history = for_model(&conn, "s", &settings, ImagePolicy::Unsupported).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].marker, HistoryTurnMarker::Summary);
        assert!(history[0].content.contains("summary"));
        assert_eq!(history[1].content, "<say>new</say>");
    }
}
