//! Ordered per-message context assembly for narrator requests.

use std::collections::{HashMap, HashSet};

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{
    compaction, entities, ledger::model::kind as ledger_kind, settings,
    stories::settings::NarratorToolSettings,
};
use crate::prompts;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

use super::author_note;

struct EntityContextData {
    entities: Vec<entities::model::Entity>,
    attrs_by_entity: HashMap<String, Vec<entities::model::EntityAttributeValue>>,
}

pub(super) struct Inputs<'a> {
    pub pool: &'a Pool,
    pub story_id: &'a str,
    pub history: &'a [HistoryTurn],
    pub config: &'a TextModelConfig,
    /// Effective per-turn permissions, with non-applicable tools turned off.
    pub tool_settings: Option<&'a NarratorToolSettings>,
    pub image_enabled: bool,
}

pub(super) struct ContextPlan {
    pub live: String,
    pub full: String,
}

fn load_entity_context_data(pool: &Pool, story_id: &str) -> AppResult<EntityContextData> {
    let conn = pool.get()?;
    let entities = entities::list_entities_sync(&conn, story_id, None)?;
    let entity_ids = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>();
    let attrs_by_entity = entities::attributes::list_entity_attributes_for_entities_sync(
        &conn,
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

fn touched_entity_ids(pool: &Pool, raw_tail: &[HistoryTurn]) -> AppResult<HashSet<String>> {
    let conn = pool.get()?;
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

    fn live(
        self,
        pool: &Pool,
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
                let touched = touched_entity_ids(pool, &history[split..])?;
                Ok(format_entity_context(&data, Some(&touched)))
            }
        }
    }
}

fn entities_full(pool: &Pool, story_id: &str) -> AppResult<EntitiesFull> {
    let context = settings::read_context_injection_settings(pool)?;
    if context.entity_context_mode == "none" {
        return Ok(EntitiesFull::None);
    }

    let data = load_entity_context_data(pool, story_id)?;
    let full = format_entity_context(&data, None);
    Ok(if context.entity_context_mode == "all" {
        EntitiesFull::All(full)
    } else {
        EntitiesFull::Scoped { data, full }
    })
}

fn tool_context(settings: Option<&NarratorToolSettings>, image_enabled: bool) -> String {
    let mut available = Vec::new();
    if settings.is_some_and(|settings| settings.get_entities) {
        available.push(prompts::GET_ENTITIES_TOOL_NAME);
    }
    if settings.is_some_and(|settings| settings.create_entity) {
        available.push(prompts::CREATE_ENTITY_TOOL_NAME);
    }
    if settings.is_some_and(|settings| settings.update_entity) {
        available.push(prompts::UPDATE_ENTITY_TOOL_NAME);
    }
    if settings.is_some_and(|settings| settings.adjust_entity_attribute) {
        available.push(prompts::ADJUST_ENTITY_ATTRIBUTE_TOOL_NAME);
    }
    if settings.is_some_and(|settings| settings.roll_check) {
        available.push(prompts::ROLL_CHECK_TOOL_NAME);
    }
    if image_enabled {
        available.push(prompts::ILLUSTRATE_SCENE_TOOL_NAME);
    }
    if available.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!(
        "Available narrator tools for this turn: {}.",
        available.join(", ")
    )];
    if settings.is_some_and(|settings| settings.roll_check) {
        lines.push(prompts::ROLL_CHECK_AVAILABLE_INSTRUCTION.to_string());
    }
    if image_enabled {
        lines.push(prompts::IMAGE_TOOL_AVAILABLE_INSTRUCTION.to_string());
    }
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

pub(super) fn build_message_context(inputs: &Inputs<'_>) -> AppResult<ContextPlan> {
    let entities = entities_full(inputs.pool, inputs.story_id)?;
    let author_note = author_note::context_block(inputs.pool, inputs.story_id)?;
    let tools = tool_context(inputs.tool_settings, inputs.image_enabled);

    let full = combine_context_blocks(&[
        entities.full().to_string(),
        author_note.clone(),
        tools.clone(),
    ]);
    let entities_live = entities.live(inputs.pool, inputs.history, inputs.config, &full)?;
    let live = combine_context_blocks(&[entities_live, author_note, tools]);
    Ok(ContextPlan { live, full })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::HistoryTurnMarker;
    use crate::features::ledger::repository::append_entry;
    use serde_json::json;

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

    #[test]
    fn pipeline_builds_all_blocks_in_fixed_order() {
        let (pool, history_turn) = story_with_entity_query();
        let history = vec![history_turn];
        let config = config();
        let tools = NarratorToolSettings::default();
        let plan = build_message_context(&Inputs {
            pool: &pool,
            story_id: "s",
            history: &history,
            config: &config,
            tool_settings: Some(&tools),
            image_enabled: true,
        })
        .unwrap();

        assert!(plan
            .live
            .contains("- Bob (character); appearance: a red cloak"));
        let entities = plan.live.find("<entities>").unwrap();
        let note = plan.live.find("<author_note>").unwrap();
        let tools_position = plan.live.find("<additional_instructions>").unwrap();
        assert!(entities < note && note < tools_position);
        assert!(plan.live.contains(&tool_context(Some(&tools), true)));
        assert!(plan.full.contains(&tool_context(Some(&tools), true)));
        assert!(plan.full.contains("<entities>"));
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
        assert_eq!(tool_context(Some(&settings), false), "");
        let image = tool_context(None, true);
        assert!(image.starts_with("<additional_instructions>\n"));
        assert!(image.contains("illustrate_scene"));
        assert!(!image.contains("roll_check"));
        assert!(!image.contains("get_entities"));

        settings.roll_check = true;
        let roll = tool_context(Some(&settings), false);
        assert!(roll.contains("roll_check"));
        assert!(!roll.contains("illustrate_scene"));
    }
}
