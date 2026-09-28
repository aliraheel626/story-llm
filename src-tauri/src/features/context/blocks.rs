//! Ordered per-message context assembly for narrator requests.

use std::collections::{HashMap, HashSet};

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::entities::{
    self,
    model::{Entity, EntityAttributeValue},
};
use crate::features::transcript::{query, repository};
use crate::features::stories::settings::{EntityContext, ContextSettings};
use crate::prompts;
use crate::shared::error::AppResult;

use super::raw_tail_boundary;

pub(crate) struct ToolDescription<'a> {
    pub name: &'a str,
    pub instruction: Option<&'a str>,
}

pub(crate) struct Inputs<'a> {
    pub conn: &'a rusqlite::Connection,
    pub story_id: &'a str,
    pub history: &'a [HistoryTurn],
    pub config: &'a TextModelConfig,
    pub context: &'a ContextSettings,
    pub tools: &'a [ToolDescription<'a>],
    pub rejected_reply: Option<&'a str>,
}

pub(crate) struct ContextPlan {
    pub live: String,
    pub full: String,
}

fn format_entity_context(
    data: &[Entity],
    attributes: &HashMap<String, Vec<EntityAttributeValue>>,
    detailed_entity_ids: Option<&HashSet<String>>,
) -> String {
    let mut lines = vec![prompts::ENTITY_CONTEXT_HEADER.to_string()];
    for entity in data {
        if detailed_entity_ids.is_some_and(|entity_ids| !entity_ids.contains(&entity.id)) {
            lines.push(format!("- {} ({})", entity.name, entity.kind));
            continue;
        }
        let appearance = entity
            .appearance_anchor
            .as_deref()
            .map(|a| format!("; appearance: {a}"))
            .unwrap_or_default();
        let attributes = match attributes.get(&entity.id) {
            Some(attrs) if !attrs.is_empty() => format!(
                "; attributes: {}",
                attrs
                    .iter()
                    .map(|attribute| format!("{}={}", attribute.canonical_name, attribute.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        };
        lines.push(format!(
            "- {} ({}){appearance}{attributes}",
            entity.name, entity.kind
        ));
    }
    format!("<entities>\n{}\n</entities>", lines.join("\n"))
}

fn touched_entity_ids(
    conn: &rusqlite::Connection,
    raw_tail: &[HistoryTurn],
) -> AppResult<HashSet<String>> {
    let Some(first_id) = raw_tail
        .iter()
        .filter_map(|turn| turn.entry_id.as_deref())
        .next()
    else {
        return Ok(HashSet::new());
    };
    let first = repository::get_entry(conn, first_id)?;
    query::entities_touched_since(conn, &first.story_id, first.seq)
}

fn tool_context(specs: &[ToolDescription<'_>]) -> String {
    if specs.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!(
        "Available narrator tools for this turn: {}.",
        specs
            .iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>()
            .join(", ")
    )];
    lines.extend(
        specs
            .iter()
            .filter_map(|spec| spec.instruction)
            .map(str::to_string),
    );
    format!(
        "<additional_instructions>\n{}\n</additional_instructions>",
        lines.join("\n")
    )
}

pub(crate) fn combine_context_blocks(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn build_message_context(inputs: &Inputs<'_>) -> AppResult<ContextPlan> {
    let (entities, attributes) = if inputs.context.entities == EntityContext::None {
        (Vec::new(), HashMap::new())
    } else {
        let entities = entities::list_entities_sync(inputs.conn, inputs.story_id, None)?;
        let ids = entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect::<Vec<_>>();
        let attributes = entities::attributes::list_entity_attributes_for_entities_sync(
            inputs.conn,
            inputs.story_id,
            &ids,
        )?;
        (entities, attributes)
    };
    let entities_full = if inputs.context.entities == EntityContext::None {
        String::new()
    } else {
        format_entity_context(&entities, &attributes, None)
    };
    let note = inputs.context.author_note.trim();
    let author_note = if inputs.context.author_note_enabled && !note.is_empty() {
        format!("<author_note>{note}</author_note>")
    } else {
        String::new()
    };
    let tools = if inputs.context.tool_instructions {
        tool_context(inputs.tools)
    } else {
        String::new()
    };

    let full = combine_context_blocks(&[entities_full.clone(), author_note.clone(), tools.clone()]);
    let entities_live = if inputs.context.entities != EntityContext::Scoped {
        entities_full
    } else {
        let split = raw_tail_boundary(
            inputs.history,
            inputs.config,
            &prompts::narrator_system_prompt(),
            &full,
        );
        let touched = touched_entity_ids(inputs.conn, &inputs.history[split..])?;
        format_entity_context(&entities, &attributes, Some(&touched))
    };
    let retry = inputs
        .rejected_reply
        .map(prompts::retry_instruction)
        .unwrap_or_default();
    let live = combine_context_blocks(&[entities_live, author_note, tools, retry]);
    Ok(ContextPlan { live, full })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{HistoryRole, HistoryTurnMarker};
    use crate::features::transcript::model::kind as transcript_kind;
    use crate::features::{
        entities,
        images::model::ImageRequest,
        narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec},
        stories::settings::NarratorToolSettings,
        turn::TurnTx,
    };
    use crate::shared::db::Pool;
    use rig_agent::tool::PortableDynamicTool;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn config() -> TextModelConfig {
        TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 32_768,
            supports_images: false,
        }
    }

    fn descriptions<'a>(specs: &'a [&'a ToolSpec]) -> Vec<ToolDescription<'a>> {
        specs
            .iter()
            .map(|spec| ToolDescription {
                name: spec.name,
                instruction: spec.instruction,
            })
            .collect()
    }

    fn story_with_entity_query() -> (Pool, HistoryTurn) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story_with_settings(
            &conn, "s", json!({"author_note":"Keep it terse."}),
        );
        entities::create_entity_with_id_sync(
            &conn,
            "bob",
            "s",
            "character",
            "Bob",
            Some("a red cloak"),
            "test",
            None,
            None,
        )
        .unwrap();
        let query_id = crate::shared::test_support::record(
            &conn, "s", transcript_kind::ENTITY_QUERIED, Some("Looked up: Bob"),
            json!({"entity_ids":["bob"]}), None, None,
        );
        conn.execute(
            "UPDATE stories SET settings_json = ?1 WHERE id = 's'",
            [json!({"injection": {"entities":"scoped", "author_note":"Keep it terse.", "author_note_enabled":true, "tool_instructions":true}}).to_string()],
        )
        .unwrap();
        drop(conn);

        (
            pool,
            HistoryTurn {
                entry_id: Some(query_id),
                role: HistoryRole::Narrator,
                content: "[Authoritative story event: entity_queried]\nLooked up: Bob".into(),
                marker: HistoryTurnMarker::Transcript,
                images: Vec::new(),
                reasoning: None,
            },
        )
    }

    #[tokio::test]
    async fn pipeline_builds_all_blocks_in_fixed_order() {
        let (pool, history_turn) = story_with_entity_query();
        let history = vec![history_turn];
        let config = config();
        let settings = NarratorToolSettings::default();
        let tools = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: true,
            illustrate: false,
        });
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        let plan = turn
            .with(|conn| {
                let context = crate::features::stories::settings::read_context_settings(conn, "s")?;
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    history: &history,
                    config: &config,
                    context: &context,
                    tools: &descriptions(&tools),
                    rejected_reply: None,
                })
            })
            .await
            .unwrap();

        assert!(plan
            .live
            .contains("- Bob (character); appearance: a red cloak"));
        let entities = plan.live.find("<entities>").unwrap();
        let note = plan.live.find("<author_note>").unwrap();
        let tools_position = plan.live.find("<additional_instructions>").unwrap();
        assert!(entities < note && note < tools_position);
        assert!(plan.live.contains(&tool_context(&descriptions(&tools))));
        assert!(plan.full.contains(&tool_context(&descriptions(&tools))));
        assert!(plan.full.contains("<entities>"));
    }

    #[tokio::test]
    async fn context_sees_uncommitted_story_and_entity_changes() {
        let (pool, history_turn) = story_with_entity_query();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            conn.execute(
                "UPDATE stories SET settings_json = ?1 WHERE id = 's'",
                [json!({"injection": {"entities":"scoped", "author_note":"A new direction.", "author_note_enabled":true, "tool_instructions":true}}).to_string()],
            )?;
            entities::create_entity_with_id_sync(
                conn, "alice", "s", "character", "Alice", Some("a blue coat"),
                "test", None, None,
            )?;
            Ok(())
        }).await.unwrap();
        let history = vec![history_turn];
        let plan = turn
            .with(|conn| {
                let context = crate::features::stories::settings::read_context_settings(conn, "s")?;
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    history: &history,
                    config: &config(),
                    context: &context,
                    tools: &[],
                    rejected_reply: None,
                })
            })
            .await
            .unwrap();

        assert!(plan
            .full
            .contains("Alice (character); appearance: a blue coat"));
        assert!(plan
            .live
            .contains("<author_note>A new direction.</author_note>"));
        assert!(plan
            .live
            .contains("Bob (character); appearance: a red cloak"));
        assert!(plan
            .live
            .contains("Alice (character); appearance: a blue coat"));
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn entity_snapshot_reads_uncommitted_attributes_on_turn_connection() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        drop(conn);
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            entities::create_entity_with_id_sync(
                conn,
                "alice",
                "s",
                "character",
                "Alice",
                Some("blue coat"),
                "test",
                None,
                None,
            )?;
            let attribute_id: String = conn.query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Accuracy'",
                [],
                |row| row.get(0),
            )?;
            entities::attributes::set_entity_attribute_sync(
                conn,
                "s",
                "alice",
                &attribute_id,
                7.0,
            )?;
            let snapshot = entities::list_entities_sync(conn, "s", None)?;
            let alice = snapshot.iter().find(|entity| entity.id == "alice").unwrap();
            let attributes = entities::attributes::list_entity_attributes_for_entities_sync(
                conn,
                "s",
                &["alice"],
            )?;
            let alice_attributes = &attributes["alice"];
            assert_eq!(snapshot.len(), 1);
            assert_eq!(alice.id, "alice");
            assert_eq!(alice.name, "Alice");
            assert_eq!(alice.appearance_anchor.as_deref(), Some("blue coat"));
            assert_eq!(alice_attributes.len(), 1);
            assert_eq!(alice_attributes[0].canonical_name, "Accuracy");
            assert_eq!(alice_attributes[0].value, 7.0);
            let context = ContextSettings {
                entities: EntityContext::All,
                author_note_enabled: false,
                author_note: String::new(),
                tool_instructions: false,
            };
            let plan = build_message_context(&Inputs {
                conn,
                story_id: "s",
                history: &[],
                config: &config(),
                context: &context,
                tools: &[],
                rejected_reply: None,
            })?;
            assert!(plan
                .full
                .contains("Alice (character); appearance: blue coat; attributes: Accuracy=7"));
            assert_eq!(plan.full, plan.live);
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn muted_note_and_tool_instructions_leave_offered_tools_unchanged() {
        let (pool, history_turn) = story_with_entity_query();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        let specs = catalog::enabled(&ToolAvailability {
            settings: &NarratorToolSettings::default(),
            image_enabled: false,
            illustrate: false,
        });
        let plan = turn
            .with(|conn| {
                let context = ContextSettings {
                    entities: EntityContext::None,
                    author_note_enabled: false,
                    author_note: "Keep it terse.".into(),
                    tool_instructions: false,
                };
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    history: &[history_turn],
                    config: &config(),
                    context: &context,
                    tools: &descriptions(&specs),
                    rejected_reply: None,
                })
            })
            .await
            .unwrap();
        assert!(!specs.is_empty());
        assert!(plan.live.is_empty());
        assert!(plan.full.is_empty());
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn retry_adds_rejected_text_only_to_live_context_after_tools() {
        let (pool, history_turn) = story_with_entity_query();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            let context = crate::features::stories::settings::read_context_settings(conn, "s")?;
            let tools = [ToolDescription {
                name: "get_entities",
                instruction: Some("Look up characters."),
            }];
            let history = [history_turn];
            let base = Inputs {
                conn,
                story_id: "s",
                history: &history,
                config: &config(),
                context: &context,
                tools: &tools,
                rejected_reply: Some("Rejected narration."),
            };
            let retry = build_message_context(&base)?;
            assert!(retry
                .live
                .contains("<rejected_reply>Rejected narration.</rejected_reply>"));
            assert!(
                retry.live.find("</additional_instructions>").unwrap()
                    < retry.live.find("<retry>").unwrap()
            );
            assert!(!retry.full.contains("<retry>"));
            let normal = build_message_context(&Inputs {
                rejected_reply: None,
                ..base
            })?;
            assert!(!normal.live.contains("<retry>"));
            assert_eq!(normal.full, retry.full);
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
    }

    #[test]
    fn tool_instructions_name_only_available_tools() {
        let mut settings = NarratorToolSettings {
            get_entities: false,
            create_entity: false,
            update_entity: false,
            adjust_entity_attribute: false,
            roll_check: false,
            ..NarratorToolSettings::default()
        };
        let none = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: false,
            illustrate: false,
        });
        assert_eq!(tool_context(&descriptions(&none)), "");

        let image = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: true,
            illustrate: true,
        });
        assert_eq!(
            tool_context(&descriptions(&image)),
            format!(
                "<additional_instructions>\nAvailable narrator tools for this turn: {}.\n{}\n</additional_instructions>",
                prompts::ILLUSTRATE_SCENE_TOOL_NAME,
                prompts::IMAGE_TOOL_AVAILABLE_INSTRUCTION
            )
        );

        settings.roll_check = true;
        let roll = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: false,
            illustrate: false,
        });
        assert_eq!(
            tool_context(&descriptions(&roll)),
            format!(
                "<additional_instructions>\nAvailable narrator tools for this turn: {}.\n{}\n</additional_instructions>",
                prompts::ROLL_CHECK_TOOL_NAME,
                prompts::ROLL_CHECK_AVAILABLE_INSTRUCTION
            )
        );
    }

    fn names_from_tool_context(context: &str) -> Vec<String> {
        if context.is_empty() {
            return Vec::new();
        }
        context
            .lines()
            .nth(1)
            .unwrap()
            .strip_prefix("Available narrator tools for this turn: ")
            .unwrap()
            .strip_suffix('.')
            .unwrap()
            .split(", ")
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn built_tool_names_match_injected_names_across_availability_combinations() {
        let disabled = NarratorToolSettings {
            get_entities: false,
            create_entity: false,
            update_entity: false,
            adjust_entity_attribute: false,
            roll_check: false,
            illustrate_scene: false,
        };
        let cases = [
            (disabled, false, false),
            (
                NarratorToolSettings {
                    roll_check: true,
                    update_entity: true,
                    ..disabled
                },
                false,
                false,
            ),
            (NarratorToolSettings::default(), true, false),
            (NarratorToolSettings::default(), true, true),
            (NarratorToolSettings::default(), false, true),
        ];

        for (settings, image_enabled, illustrate) in cases {
            let specs = catalog::enabled(&ToolAvailability {
                settings: &settings,
                image_enabled,
                illustrate,
            });
            let pool = crate::shared::db::test_pool();
            let deps = ToolDeps {
                turn: Some(TurnTx::begin(&pool, &Default::default(), "s").unwrap()),
                target_entry_id: Some("narration".into()),
                turn_id: Some("turn".into()),
                embedding_api_key: String::new(),
                image_requests: Arc::new(Mutex::new(Vec::<ImageRequest>::new())),
            };
            let built = specs
                .iter()
                .map(|spec| (spec.build)(&deps))
                .collect::<Vec<PortableDynamicTool>>();
            let built_names = built
                .iter()
                .map(|tool| tool.name().to_string())
                .collect::<Vec<_>>();

            assert_eq!(
                built_names,
                names_from_tool_context(&tool_context(&descriptions(&specs)))
            );
        }
    }
}
