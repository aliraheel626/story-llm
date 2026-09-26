//! Ordered per-message context assembly for narrator requests.

use std::collections::HashSet;

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::ledger::{query, repository};
use crate::prompts;
use crate::shared::error::{AppError, AppResult};

use super::raw_tail_boundary;

pub(crate) struct EntityDisplay {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub appearance_anchor: Option<String>,
    pub attributes: Vec<AttributeDisplay>,
}

pub(crate) struct AttributeDisplay {
    pub canonical_name: String,
    pub value: f64,
}

pub(crate) struct ToolDescription<'a> {
    pub name: &'a str,
    pub instruction: Option<&'a str>,
}

pub(crate) struct Inputs<'a> {
    pub conn: &'a rusqlite::Connection,
    pub story_id: &'a str,
    pub history: &'a [HistoryTurn],
    pub config: &'a TextModelConfig,
    pub entity_mode: &'a str,
    pub entities: &'a [EntityDisplay],
    pub tools: &'a [ToolDescription<'a>],
}

pub(crate) struct ContextPlan {
    pub live: String,
    pub full: String,
}

fn format_entity_context(
    data: &[EntityDisplay],
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
        let attributes = match &entity.attributes {
            attrs if !attrs.is_empty() => format!(
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
    let entities_full = if inputs.entity_mode == "none" {
        String::new()
    } else {
        format_entity_context(inputs.entities, None)
    };
    let author_note = {
        let raw: String = inputs
            .conn
            .query_row(
                "SELECT settings_json FROM stories WHERE id = ?1",
                [inputs.story_id],
                |row| row.get(0),
            )
            .map_err(|_| AppError::NotFound(format!("story {} not found", inputs.story_id)))?;
        let settings: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));
        let enabled = settings
            .get("author_note_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let note = settings
            .get("author_note")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        if enabled && !note.is_empty() {
            format!("<author_note>{note}</author_note>")
        } else {
            String::new()
        }
    };
    let tools = tool_context(inputs.tools);

    let full = combine_context_blocks(&[entities_full.clone(), author_note.clone(), tools.clone()]);
    let entities_live = if inputs.entity_mode == "none" || inputs.entity_mode == "all" {
        entities_full
    } else {
        let split = raw_tail_boundary(
            inputs.history,
            inputs.config,
            &prompts::narrator_system_prompt(),
            &full,
        );
        let touched = touched_entity_ids(inputs.conn, &inputs.history[split..])?;
        format_entity_context(inputs.entities, Some(&touched))
    };
    let live = combine_context_blocks(&[entities_live, author_note, tools]);
    Ok(ContextPlan { live, full })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::HistoryTurnMarker;
    use crate::features::ledger::model::kind as ledger_kind;
    use crate::features::{
        entities,
        images::model::ImageRequest,
        ledger::repository::append_entry,
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
        }
    }

    fn snapshot(conn: &rusqlite::Connection) -> Vec<EntityDisplay> {
        let entities = entities::list_entities_sync(conn, "s", None).unwrap();
        let ids = entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect::<Vec<_>>();
        let mut attrs =
            entities::attributes::list_entity_attributes_for_entities_sync(conn, "s", &ids)
                .unwrap();
        entities
            .into_iter()
            .map(|entity| EntityDisplay {
                attributes: attrs
                    .remove(&entity.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|attr| AttributeDisplay {
                        canonical_name: attr.canonical_name,
                        value: attr.value,
                    })
                    .collect(),
                id: entity.id,
                name: entity.name,
                kind: entity.kind,
                appearance_anchor: entity.appearance_anchor,
            })
            .collect()
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
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{\"author_note\":\"Keep it terse.\"}')",
            [],
        )
        .unwrap();
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
        let query = append_entry(
            &conn,
            "s",
            ledger_kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up: Bob"),
            &json!({"entity_ids":["bob"]}),
            None,
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('context_injection', ?1)",
            [json!({"entity_context_mode":"scoped"}).to_string()],
        )
        .unwrap();
        drop(conn);

        (
            pool,
            HistoryTurn {
                entry_id: Some(query.id),
                is_player: false,
                content: "[Authoritative story event: entity_queried]\nLooked up: Bob".into(),
                marker: HistoryTurnMarker::Ledger,
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
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    history: &history,
                    config: &config,
                    entity_mode: "scoped",
                    entities: &snapshot(conn),
                    tools: &descriptions(&tools),
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
                "UPDATE stories SET settings_json = '{\"author_note\":\"A new direction.\"}' WHERE id = 's'",
                [],
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
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    history: &history,
                    config: &config(),
                    entity_mode: "scoped",
                    entities: &snapshot(conn),
                    tools: &[],
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
        assert!(plan.live.contains("Alice (character); appearance: a blue coat"));
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
