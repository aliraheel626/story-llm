//! Optional entity-context injection, controlled only by `entity_context_mode`.

use std::collections::{HashMap, HashSet};

use crate::ai::{HistoryTurn, TextModelConfig};
use crate::features::{entities, settings, timeline::model::kind as timeline_kind};
use crate::prompts;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

pub(super) struct EntityContextData {
    entities: Vec<entities::model::Entity>,
    attrs_by_entity: HashMap<String, Vec<entities::model::EntityAttributeValue>>,
}

#[derive(Debug, Clone)]
pub(super) struct EntityContextPlan {
    pub live_context: Option<String>,
    pub full_context: Option<String>,
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

pub(super) fn format_entity_context(
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

pub(super) fn touched_entity_ids(
    pool: &Pool,
    raw_tail: &[HistoryTurn],
) -> AppResult<HashSet<String>> {
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
        AppError::Other(format!("failed to serialize timeline entry ids: {error}"))
    })?;
    let mut stmt = conn.prepare(
        "SELECT kind, payload_json FROM timeline_entries
         WHERE id IN (SELECT value FROM json_each(?1))
           AND kind IN (?2, ?3, ?4, ?5)",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            entry_ids_json,
            timeline_kind::ENTITY_CREATED,
            timeline_kind::ENTITY_UPDATED,
            timeline_kind::ENTITY_ATTRIBUTE_CHANGED,
            timeline_kind::ENTITY_QUERIED,
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    for row in rows {
        let (entry_kind, payload_json) = row?;
        let payload = serde_json::from_str::<serde_json::Value>(&payload_json)
            .map_err(|error| AppError::Other(format!("invalid timeline payload JSON: {error}")))?;
        match entry_kind.as_str() {
            timeline_kind::ENTITY_CREATED
            | timeline_kind::ENTITY_UPDATED
            | timeline_kind::ENTITY_ATTRIBUTE_CHANGED => {
                if let Some(entity_id) = payload.get("entity_id").and_then(|id| id.as_str()) {
                    touched.insert(entity_id.to_string());
                }
            }
            timeline_kind::ENTITY_QUERIED => {
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

pub(super) fn build_entity_context(
    pool: &Pool,
    story_id: &str,
    history: &[HistoryTurn],
    config: &TextModelConfig,
    preamble: &str,
) -> AppResult<EntityContextPlan> {
    let memory = settings::read_narrator_memory_settings(pool)?;
    if memory.entity_context_mode == "none" {
        return Ok(EntityContextPlan {
            live_context: None,
            full_context: None,
        });
    }

    let context_data = load_entity_context_data(pool, story_id)?;
    let full_context = format_entity_context(&context_data, None);
    if memory.entity_context_mode == "all" {
        return Ok(EntityContextPlan {
            live_context: Some(full_context.clone()),
            full_context: Some(full_context),
        });
    }

    let split =
        crate::features::compaction::raw_tail_boundary(history, config, preamble, &full_context);
    let touched = touched_entity_ids(pool, &history[split..])?;
    let scoped_context = format_entity_context(&context_data, Some(&touched));
    Ok(EntityContextPlan {
        live_context: Some(scoped_context),
        full_context: Some(full_context),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::timeline::repository::append_entry;
    use serde_json::json;

    #[test]
    fn scoped_context_details_touched_entities_and_lists_the_rest() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
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
        entities::create_entity_with_id_sync(
            &conn,
            "mill",
            "s",
            "location",
            "Old Mill",
            Some("a mossy waterwheel"),
            "test",
            None,
        )
        .unwrap();
        let trust_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Trust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO entity_attributes
             (story_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
             VALUES ('s', 'bob', ?1, 7, 'user', 'now', NULL),
                    ('s', 'mill', ?1, 3, 'user', 'now', NULL)",
            [&trust_id],
        )
        .unwrap();
        let query = append_entry(
            &conn,
            "s",
            timeline_kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up: Bob"),
            &json!({"entity_ids":["bob"]}),
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('narrator_memory', ?1)",
            [json!({"tool_call_persistence":true,"entity_context_mode":"scoped"}).to_string()],
        )
        .unwrap();
        drop(conn);

        let history = vec![HistoryTurn {
            entry_id: Some(query.id),
            is_player: false,
            content: "[Authoritative story event: entity_queried]\nLooked up: Bob".into(),
            marker: crate::ai::HistoryTurnMarker::Timeline,
        }];
        let config = TextModelConfig {
            provider: "openrouter".into(),
            model: "test".into(),
            api_key: "test".into(),
            context_window: 32_768,
        };
        let preamble = prompts::narrator_system_prompt(None);
        let plan = build_entity_context(&pool, "s", &history, &config, &preamble).unwrap();

        assert!(plan
            .live_context
            .as_deref()
            .unwrap()
            .contains("- Bob (character); appearance: a red cloak; attributes: Trust=7"));
        assert!(plan
            .live_context
            .as_deref()
            .unwrap()
            .contains("- Old Mill (location)"));
        assert!(!plan
            .live_context
            .as_deref()
            .unwrap()
            .contains("Old Mill (location); appearance:"));
        assert!(plan
            .full_context
            .as_deref()
            .unwrap()
            .contains("Old Mill (location); appearance: a mossy waterwheel; attributes: Trust=3"));
    }
}
