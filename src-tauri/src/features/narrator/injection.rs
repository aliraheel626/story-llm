//! Ordered per-message context assembly for narrator requests.

use std::collections::{HashMap, HashSet};

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{
    compaction, entities,
    ledger::{model::kind as ledger_kind, turn_tx::TurnTx},
    settings,
};
use crate::prompts;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::catalog::ToolSpec;

struct EntityContextData {
    entities: Vec<entities::model::Entity>,
    attrs_by_entity: HashMap<String, Vec<entities::model::EntityAttributeValue>>,
}

pub(super) struct Inputs<'a> {
    pub turn: &'a TurnTx,
    pub settings_pool: &'a Pool,
    pub story_id: &'a str,
    pub history: &'a [HistoryTurn],
    pub config: &'a TextModelConfig,
    pub tool_specs: &'a [&'static ToolSpec],
}

pub(super) struct ContextPlan {
    pub live: String,
    pub full: String,
}

fn load_entity_context_data(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<EntityContextData> {
    let entities = entities::list_entities_sync(conn, story_id, None)?;
    let entity_ids = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>();
    let attrs_by_entity = entities::attributes::list_entity_attributes_for_entities_sync(
        conn,
        story_id,
        &entity_ids,
    )?;
    Ok(EntityContextData {
        entities,
        attrs_by_entity,
    })
}

fn format_entity_context(
    data: &EntityContextData,
    detailed_entity_ids: Option<&HashSet<String>>,
) -> String {
    let mut lines = vec![prompts::ENTITY_CONTEXT_HEADER.to_string()];
    for entity in &data.entities {
        if detailed_entity_ids.is_some_and(|entity_ids| !entity_ids.contains(&entity.id)) {
            lines.push(format!("- {} ({})", entity.name, entity.kind));
            continue;
        }
        let appearance = entity
            .appearance_anchor
            .as_deref()
            .map(|a| format!("; appearance: {a}"))
            .unwrap_or_default();
        let attributes = match data.attrs_by_entity.get(&entity.id) {
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
    let mut touched = HashSet::new();
    let entry_ids = raw_tail
        .iter()
        .filter_map(|turn| turn.entry_id.as_deref())
        .collect::<Vec<_>>();
    if entry_ids.is_empty() {
        return Ok(touched);
    }
    let entry_ids_json = serde_json::to_string(&entry_ids).map_err(|error| {
        AppError::Other(format!("failed to serialize ledger entry ids: {error}"))
    })?;
    let mut stmt = conn.prepare(
        "SELECT kind, payload_json FROM ledger_entries
         WHERE id IN (SELECT value FROM json_each(?1))
           AND kind IN (?2, ?3, ?4, ?5)",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            entry_ids_json,
            ledger_kind::ENTITY_CREATED,
            ledger_kind::ENTITY_UPDATED,
            ledger_kind::ENTITY_ATTRIBUTE_CHANGED,
            ledger_kind::ENTITY_QUERIED,
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    for row in rows {
        let (entry_kind, payload_json) = row?;
        let payload = serde_json::from_str::<serde_json::Value>(&payload_json)
            .map_err(|error| AppError::Other(format!("invalid ledger payload JSON: {error}")))?;
        match entry_kind.as_str() {
            ledger_kind::ENTITY_CREATED
            | ledger_kind::ENTITY_UPDATED
            | ledger_kind::ENTITY_ATTRIBUTE_CHANGED => {
                if let Some(entity_id) = payload.get("entity_id").and_then(|id| id.as_str()) {
                    touched.insert(entity_id.to_string());
                }
            }
            ledger_kind::ENTITY_QUERIED => {
                if let Some(entity_ids) = payload.get("entity_ids").and_then(|ids| ids.as_array()) {
                    touched.extend(
                        entity_ids
                            .iter()
                            .filter_map(|id| id.as_str().map(str::to_string)),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(touched)
}

enum EntitiesFull {
    None,
    All(String),
    Scoped {
        data: EntityContextData,
        full: String,
    },
}

impl EntitiesFull {
    fn full(&self) -> &str {
        match self {
            Self::None => "",
            Self::All(full) | Self::Scoped { full, .. } => full,
        }
    }

    async fn live(
        self,
        turn: &TurnTx,
        history: &[HistoryTurn],
        config: &TextModelConfig,
        full: &str,
    ) -> AppResult<String> {
        match self {
            Self::None => Ok(String::new()),
            Self::All(dump) => Ok(dump),
            Self::Scoped { data, .. } => {
                let split = compaction::raw_tail_boundary(
                    history,
                    config,
                    &prompts::narrator_system_prompt(),
                    full,
                );
                let touched = turn
                    .with(|conn| touched_entity_ids(conn, &history[split..]))
                    .await?;
                Ok(format_entity_context(&data, Some(&touched)))
            }
        }
    }
}

async fn entities_full(
    turn: &TurnTx,
    settings_pool: &Pool,
    story_id: &str,
) -> AppResult<EntitiesFull> {
    let context = settings::read_context_injection_settings(settings_pool)?;
    if context.entity_context_mode == "none" {
        return Ok(EntitiesFull::None);
    }

    let data = turn
        .with(|conn| load_entity_context_data(conn, story_id))
        .await?;
    let full = format_entity_context(&data, None);
    Ok(if context.entity_context_mode == "all" {
        EntitiesFull::All(full)
    } else {
        EntitiesFull::Scoped { data, full }
    })
}

fn tool_context(specs: &[&ToolSpec]) -> String {
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

pub(super) fn combine_context_blocks(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) async fn build_message_context(inputs: &Inputs<'_>) -> AppResult<ContextPlan> {
    let entities = entities_full(inputs.turn, inputs.settings_pool, inputs.story_id).await?;
    let author_note = inputs
        .turn
        .with(|conn| {
            let raw: String = conn
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
            Ok(if enabled && !note.is_empty() {
                format!("<author_note>{note}</author_note>")
            } else {
                String::new()
            })
        })
        .await?;
    let tools = tool_context(inputs.tool_specs);

    let full = combine_context_blocks(&[
        entities.full().to_string(),
        author_note.clone(),
        tools.clone(),
    ]);
    let entities_live = entities
        .live(inputs.turn, inputs.history, inputs.config, &full)
        .await?;
    let live = combine_context_blocks(&[entities_live, author_note, tools]);
    Ok(ContextPlan { live, full })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::HistoryTurnMarker;
    use crate::features::{
        images::model::ImageRequest,
        ledger::repository::append_entry,
        narrator::catalog::{self, ToolAvailability, ToolDeps},
        stories::settings::NarratorToolSettings,
    };
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
        let plan = build_message_context(&Inputs {
            turn: &turn,
            settings_pool: &pool,
            story_id: "s",
            history: &history,
            config: &config,
            tool_specs: &tools,
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
        assert!(plan.live.contains(&tool_context(&tools)));
        assert!(plan.full.contains(&tool_context(&tools)));
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
        let plan = build_message_context(&Inputs {
            turn: &turn,
            settings_pool: &pool,
            story_id: "s",
            history: &history,
            config: &config(),
            tool_specs: &[],
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
        assert_eq!(tool_context(&none), "");

        let image = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: true,
            illustrate: true,
        });
        assert_eq!(
            tool_context(&image),
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
            tool_context(&roll),
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

            assert_eq!(built_names, names_from_tool_context(&tool_context(&specs)));
        }
    }
}
