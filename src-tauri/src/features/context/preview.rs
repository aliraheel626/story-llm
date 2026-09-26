//! Read-only semantic preview of the next continue narration request.

use serde::Serialize;
use tauri::AppHandle;

use crate::ai::{HistoryRole, HistoryTurn, HistoryTurnMarker, TextModelConfig};
use crate::prompts;
use crate::shared::{db::Pool, error::AppResult};

use super::{
    build_message_context, combine_context_blocks,
    injection::{EntityDisplay, Inputs, ToolDescription},
    load_transcript,
    settings::{self, EntityInjection},
    ImagePolicy,
};

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct PreviewMessage {
    pub role: &'static str,
    pub text: String,
    pub image_count: usize,
    pub has_reasoning: bool,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct ContextPreview {
    pub system: String,
    pub messages: Vec<PreviewMessage>,
    pub injected: String,
    pub images_unsupported: bool,
}

pub struct PreviewConfig {
    pub model: TextModelConfig,
    pub image_enabled: bool,
}

pub struct PreviewTool {
    pub name: &'static str,
    pub instruction: Option<&'static str>,
}

pub struct PreviewMetadata {
    pub entities: Vec<EntityDisplay>,
    pub tools: Vec<PreviewTool>,
}

#[derive(Clone, Copy)]
pub struct PreviewCallbacks {
    pub config: fn(&AppHandle, &Pool) -> AppResult<PreviewConfig>,
    pub metadata: fn(&rusqlite::Connection, &str, bool) -> AppResult<PreviewMetadata>,
}

pub fn build_preview(
    conn: &rusqlite::Connection,
    story_id: &str,
    config: &PreviewConfig,
    metadata: impl FnOnce(&rusqlite::Connection, &str) -> AppResult<PreviewMetadata>,
) -> AppResult<ContextPreview> {
    let settings = settings::read_context_settings(conn, story_id)?;
    let injection = settings::read_injection_settings(conn, story_id)?;
    let images_unsupported = settings.includes("images") && !config.model.supports_images;
    let policy = if config.model.supports_images {
        ImagePolicy::Allowed
    } else {
        ImagePolicy::Unsupported
    };
    let mut history = load_transcript(conn, story_id, &settings, policy)?;
    history.push(HistoryTurn {
        entry_id: None,
        role: HistoryRole::Player,
        content: prompts::render_turn("continue", "").expect("continue is a valid turn mode"),
        marker: HistoryTurnMarker::Ledger,
        images: Vec::new(),
        reasoning: None,
    });
    let mut data = metadata(conn, story_id)?;
    if injection.entities == EntityInjection::None {
        data.entities.clear();
    }
    let descriptions = data
        .tools
        .iter()
        .map(|tool| ToolDescription {
            name: tool.name,
            instruction: tool.instruction,
        })
        .collect::<Vec<_>>();
    let plan = build_message_context(&Inputs {
        conn,
        history: &history,
        config: &config.model,
        injection: &injection,
        entities: &data.entities,
        tools: &descriptions,
    })?;
    let last = history.last_mut().expect("synthetic continue turn exists");
    last.content = combine_context_blocks(&[plan.live.clone(), last.content.clone()]);
    Ok(ContextPreview {
        system: prompts::narrator_system_prompt(),
        messages: history
            .into_iter()
            .map(|turn| PreviewMessage {
                role: match turn.role {
                    HistoryRole::Player => "player",
                    HistoryRole::Narrator => "narrator",
                    HistoryRole::Record => "record",
                },
                text: turn.content,
                image_count: turn.images.len(),
                has_reasoning: turn.reasoning.is_some(),
            })
            .collect(),
        injected: plan.live,
        images_unsupported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::{attachments, model::kind, repository};
    use serde_json::json;

    fn config(supports_images: bool) -> PreviewConfig {
        PreviewConfig {
            model: TextModelConfig {
                provider: "test".into(),
                model: "test".into(),
                api_key: "secret-api-key".into(),
                context_window: 32_768,
                supports_images,
            },
            image_enabled: false,
        }
    }

    fn metadata(_: &rusqlite::Connection, _: &str) -> AppResult<PreviewMetadata> {
        Ok(PreviewMetadata {
            entities: vec![EntityDisplay {
                id: "bob".into(),
                name: "Bob".into(),
                kind: "character".into(),
                appearance_anchor: Some("red cloak".into()),
                attributes: Vec::new(),
            }],
            tools: vec![PreviewTool {
                name: "get_entities",
                instruction: Some("Look up characters."),
            }],
        })
    }

    #[test]
    fn preview_matches_transcript_and_live_context_without_writes() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let story_settings = json!({
            "context": {"include": {"images": true, "narration.thoughts": true}},
            "injection": {"entities": "scoped", "author_note": "Keep it tense.",
                          "author_note_enabled": true, "tool_instructions": true}
        });
        conn.execute(
            "INSERT INTO stories (id,title,created_at,updated_at,settings_json) VALUES ('s','Story','now','now',?1)",
            [story_settings.to_string()],
        )
        .unwrap();
        let append = |kind, visibility, content, payload, target: Option<&str>| {
            repository::append_entry(
                &conn, "s", kind, visibility, content, &payload, target, None,
            )
            .unwrap()
        };
        let old = append(
            kind::PLAYER_MESSAGE,
            "visible",
            Some("old"),
            json!({"input_mode":"do"}),
            None,
        );
        append(
            kind::CONTEXT_SUMMARY,
            "hidden",
            Some("Past events condensed."),
            json!({"through_entry_id":old.id,"through_seq":old.seq}),
            None,
        );
        let player = append(
            kind::PLAYER_MESSAGE,
            "visible",
            Some("original"),
            json!({"input_mode":"say"}),
            None,
        );
        append(
            kind::CONTENT_EDITED,
            "hidden",
            Some("revised"),
            json!({}),
            Some(&player.id),
        );
        let narration = append(
            kind::NARRATION,
            "visible",
            Some("The room waits."),
            json!({"thoughts":"private reasoning"}),
            None,
        );
        attachments::insert_image(
            &conn,
            &crate::features::ledger::model::StoryImage {
                id: "asset".into(),
                entry_id: narration.id,
                prompt: "picture".into(),
                created_at: "now".into(),
            },
            "image/png",
            b"secret-image-bytes",
        )
        .unwrap();
        append(
            kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up: Bob"),
            json!({"entity_ids":["bob"]}),
            None,
        );
        let before: (String, String, String) = conn
            .query_row(
                "SELECT title, settings_json,
                    (SELECT json_group_array(json_object(
                        'id',id,'seq',seq,'kind',kind,'visibility',visibility,
                        'content',content,'payload',payload_json,'target',target_entry_id,
                        'turn',turn_id,'created',created_at))
                     FROM ledger_entries WHERE story_id='s')
             FROM stories WHERE id='s'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        for supports_images in [true, false] {
            let config = config(supports_images);
            let preview = build_preview(&conn, "s", &config, metadata).unwrap();
            let settings = settings::read_context_settings(&conn, "s").unwrap();
            let injection = settings::read_injection_settings(&conn, "s").unwrap();
            let mut history = load_transcript(
                &conn,
                "s",
                &settings,
                if supports_images {
                    ImagePolicy::Allowed
                } else {
                    ImagePolicy::Unsupported
                },
            )
            .unwrap();
            assert_eq!(history[0].marker, HistoryTurnMarker::Summary);
            assert!(!history
                .iter()
                .any(|turn| turn.content.contains("<do>old</do>")));
            assert!(history
                .iter()
                .any(|turn| turn.content == "<say>revised</say>"));
            assert!(!history.iter().any(|turn| turn.content.contains("original")));
            history.push(HistoryTurn {
                entry_id: None,
                role: HistoryRole::Player,
                content: "<continue/>".into(),
                marker: HistoryTurnMarker::Ledger,
                images: Vec::new(),
                reasoning: None,
            });
            let data = metadata(&conn, "s").unwrap();
            let descriptions = data
                .tools
                .iter()
                .map(|tool| ToolDescription {
                    name: tool.name,
                    instruction: tool.instruction,
                })
                .collect::<Vec<_>>();
            let plan = build_message_context(&Inputs {
                conn: &conn,
                history: &history,
                config: &config.model,
                injection: &injection,
                entities: &data.entities,
                tools: &descriptions,
            })
            .unwrap();
            history.last_mut().unwrap().content =
                combine_context_blocks(&[plan.live.clone(), "<continue/>".into()]);
            let expected = history
                .into_iter()
                .map(|turn| PreviewMessage {
                    role: match turn.role {
                        HistoryRole::Player => "player",
                        HistoryRole::Narrator => "narrator",
                        HistoryRole::Record => "record",
                    },
                    text: turn.content,
                    image_count: turn.images.len(),
                    has_reasoning: turn.reasoning.is_some(),
                })
                .collect::<Vec<_>>();
            assert_eq!(preview.messages, expected);
            assert_eq!(preview.system, prompts::narrator_system_prompt());
            assert_eq!(preview.injected, plan.live);
            assert_eq!(preview.images_unsupported, !supports_images);
            assert!(preview.messages.iter().any(|msg| msg.role == "record"));
            assert!(preview.messages.iter().any(|msg| msg.has_reasoning));
            assert_eq!(
                preview
                    .messages
                    .iter()
                    .map(|msg| msg.image_count)
                    .sum::<usize>(),
                usize::from(supports_images)
            );
            assert!(preview
                .messages
                .last()
                .unwrap()
                .text
                .starts_with(&preview.injected));
            assert!(preview
                .messages
                .last()
                .unwrap()
                .text
                .ends_with("<continue/>"));
            let wire = serde_json::to_string(&preview).unwrap();
            assert!(!wire.contains("secret-image-bytes"));
            assert!(!wire.contains("private reasoning"));
            assert!(!wire.contains("secret-api-key"));
        }

        let after: (String, String, String) = conn
            .query_row(
                "SELECT title, settings_json,
                    (SELECT json_group_array(json_object(
                        'id',id,'seq',seq,'kind',kind,'visibility',visibility,
                        'content',content,'payload',payload_json,'target',target_entry_id,
                        'turn',turn_id,'created',created_at))
                     FROM ledger_entries WHERE story_id='s')
             FROM stories WHERE id='s'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(before, after);

        conn.execute(
            "UPDATE stories SET settings_json = '{}' WHERE id = 's'",
            [],
        )
        .unwrap();
        let disabled = build_preview(&conn, "s", &config(false), metadata).unwrap();
        assert!(!disabled.images_unsupported);
        assert!(disabled.messages.iter().all(|message| message.image_count == 0));
    }
}
