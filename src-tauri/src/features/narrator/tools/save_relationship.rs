use std::sync::Arc;

use rig_agent::tool::{PortableDynamicTool, ToolOutput};
use serde_json::{json, Value};

use crate::features::entities::{
    self,
    model::{Entity, EntityLink, RELATIONSHIP},
    repository,
};
use crate::features::narrator::catalog::{self, ToolAvailability, ToolDeps, ToolSpec};
use crate::features::turn::TurnTx;
use crate::shared::error::{AppError, AppResult};

use super::shared::{
    apply_resolved_stats, nullable_field, object_args, optional_string, parse_stats,
    required_string, resolve_character, resolve_stats, stats_schema,
};
use super::to_tool_error;

pub const NAME: &str = "save_relationship";
pub const DESCRIPTION: &str =
    "Record how one character relates to another, or update it, whenever the story establishes \
     or changes how two characters relate. There is one relationship per \
     direction: calling again with the same from and to updates it (a new label replaces the \
     old one, such as \"former friends\" when a friendship ends). `label` is short (\"estranged \
     sister\", \"owes money to\", \"siblings\"). Use direction \"both\" only for truly mutual facts \
     (siblings, married, allies); feelings that differ per side are two one_way relationships. \
     Use stats for how one feels about the other, such as Affection or Trust.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "from": {"type": "string", "description": "A character name (an id is also accepted)."},
            "to": {"type": "string", "description": "A character name (an id is also accepted)."},
            "label": {"type": "string", "description": "Short relationship label, required for a new relationship."},
            "direction": {"type": "string", "enum": ["one_way", "both"], "description": "one_way for directed feelings; both only for mutual facts. Defaults to one_way when creating."},
            "description": {"type": ["string", "null"], "description": "Relationship details; null clears it."},
            "stats": stats_schema()
        },
        "required": ["from", "to"],
        "additionalProperties": false
    })
}

fn label(_args: &Value) -> String {
    "Recording a relationship…".to_string()
}

fn enabled(availability: &ToolAvailability<'_>) -> bool {
    catalog::normal_mode(availability) && availability.settings.save_relationship
}

fn build(deps: &ToolDeps) -> PortableDynamicTool {
    let (turn, target, turn_id) = catalog::turn(deps);
    tool(turn, target, turn_id, deps.embedding_api_key.clone())
}

pub const SPEC: ToolSpec = ToolSpec {
    name: NAME,
    needs_turn: true,
    enabled,
    build,
    label,
};

fn find_relationship(
    conn: &rusqlite::Connection,
    story_id: &str,
    from: &Entity,
    to: &Entity,
    label: Option<&str>,
    direction: Option<&str>,
) -> AppResult<Option<Entity>> {
    if from.id == to.id {
        return Err(AppError::Invalid("relationship endpoints must be different".into()));
    }
    let existing = entities::list_entities_sync(conn, story_id, Some(RELATIONSHIP))?
        .into_iter().find(|entity| entity.link.as_ref().is_some_and(|link| {
            (link.from_id == from.id && link.to_id == to.id)
                || (link.direction == "both" && link.from_id == to.id && link.to_id == from.id)
        }));
    if let Some(entity) = &existing {
        if entity.link.as_ref().is_some_and(|link| {
            link.direction == "both" && link.from_id != from.id && direction == Some("one_way")
        }) {
            return Err(AppError::Invalid(format!(
                "this relationship is stored as {} ↔ {}; make it one-way in that direction, or record the other direction separately",
                to.name, from.name,
            )));
        }
    } else if label.is_none() {
        return Err(AppError::Invalid("label is required for a new relationship".into()));
    }
    Ok(existing)
}

pub(super) fn tool(
    turn: Arc<TurnTx>,
    target_entry_id: String,
    turn_id: String,
    embedding_api_key: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(NAME, DESCRIPTION, schema(), move |args: Value| {
        let turn = turn.clone();
        let target_entry_id = target_entry_id.clone();
        let turn_id = turn_id.clone();
        let embedding_api_key = embedding_api_key.clone();
        Box::pin(async move {
            let fields = object_args(&args, &["from", "to", "label", "direction", "description", "stats"], NAME)
                .map_err(to_tool_error)?;
            let from_reference = required_string(fields, "from").map_err(to_tool_error)?;
            let to_reference = required_string(fields, "to").map_err(to_tool_error)?;
            let label = optional_string(fields, "label").map_err(to_tool_error)?;
            let direction = optional_string(fields, "direction").map_err(to_tool_error)?;
            if direction.as_deref().is_some_and(|value| !matches!(value, "one_way" | "both")) {
                return Err(to_tool_error(AppError::Invalid("direction must be one_way or both".into())));
            }
            let description = nullable_field(fields, "description").map_err(to_tool_error)?;
            let stats = parse_stats(fields.get("stats")).map_err(to_tool_error)?;
            turn.with(|conn| {
                let from = resolve_character(conn, turn.story_id(), &from_reference)?;
                let to = resolve_character(conn, turn.story_id(), &to_reference)?;
                find_relationship(conn, turn.story_id(), &from, &to, label.as_deref(), direction.as_deref())?;
                Ok(())
            }).await.map_err(to_tool_error)?;
            let resolved = resolve_stats(&turn, &embedding_api_key, RELATIONSHIP, stats.as_deref().unwrap_or_default())
                .await.map_err(to_tool_error)?;
            let (entity, created, results) = turn.with_savepoint(|conn| {
                let from = resolve_character(conn, turn.story_id(), &from_reference)?;
                let to = resolve_character(conn, turn.story_id(), &to_reference)?;
                let existing = find_relationship(conn, turn.story_id(), &from, &to, label.as_deref(), direction.as_deref())?;
                let (entity, created) = if let Some(existing) = existing {
                    (repository::update_link_sync(
                        conn, turn.story_id(), &existing.id, label.as_deref(), direction.as_deref(),
                        description.clone(), "narrator_tool", Some(&target_entry_id), Some(&turn_id),
                    )?, false)
                } else {
                    let label = label.clone().ok_or_else(|| AppError::Invalid("label is required for a new relationship".into()))?;
                    (repository::create_link_sync(conn, turn.story_id(), EntityLink {
                        from_id: from.id, to_id: to.id, label,
                        direction: direction.clone().unwrap_or_else(|| "one_way".into()),
                        description: description.clone().flatten(),
                    }, "narrator_tool", Some(&target_entry_id), Some(&turn_id))?, true)
                };
                let results = apply_resolved_stats(conn, turn.story_id(), &entity.id, resolved, &target_entry_id, &turn_id)?;
                Ok((entity, created, results))
            }).await.map_err(to_tool_error)?;
            let mut output = json!({"name": entity.name, "kind": entity.kind, "created": created});
            if stats.is_some() {
                output["stats"] = json!(results);
            }
            Ok(ToolOutput::json(output))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::entities::{attributes, model::CHARACTER, registry};
    use crate::shared::db::Pool;

    async fn fixture() -> (Pool, Arc<TurnTx>, String, String) {
        let (pool, turn, target, turn_id) = super::super::test_support::fixture();
        turn.with(|conn| {
            for (id, name) in [("mira", "Mira"), ("varro", "Varro")] {
                repository::create_entity_with_id_sync(conn, id, turn.story_id(), CHARACTER, name, None,
                    "narrator_tool", Some(&target), Some(&turn_id))?;
            }
            Ok(())
        }).await.unwrap();
        (pool, turn, target, turn_id)
    }

    #[tokio::test]
    async fn names_repeats_reversed_both_and_relabel_keep_one_relationship() {
        let (_pool, turn, target, turn_id) = fixture().await;
        let save = tool(turn.clone(), target, turn_id, String::new());
        let first = save.execute(json!({"from":" mira ","to":"VARRO","label":" siblings ","direction":"both"}))
            .await.unwrap();
        assert_eq!(first.as_json().unwrap(), &json!({"name":"Mira ↔ Varro: siblings","kind":"relationship","created":true}));
        let second = save.execute(json!({"from":"Varro","to":"Mira"})).await.unwrap();
        assert_eq!(second.as_json().unwrap(), &json!({"name":"Mira ↔ Varro: siblings","kind":"relationship","created":false}));
        let same = save.execute(json!({"from":"Mira","to":"Varro","direction":"both","label":"siblings"})).await.unwrap();
        assert_eq!(same.as_json().unwrap()["created"], false);
        let changed = save.execute(json!({"from":"Varro","to":"Mira","label":"estranged siblings"})).await.unwrap();
        assert_eq!(changed.as_json().unwrap()["name"], "Mira ↔ Varro: estranged siblings");
        turn.with(|conn| {
            let links = entities::list_entities_sync(conn, turn.story_id(), Some(RELATIONSHIP))?;
            assert_eq!(links.len(), 1);
            let link = links[0].link.as_ref().unwrap();
            assert_eq!((link.from_id.as_str(), link.to_id.as_str(), link.direction.as_str()), ("mira", "varro", "both"));
            let updates: i64 = conn.query_row("SELECT COUNT(*) FROM transcript_entries WHERE kind='entity_updated'", [], |row| row.get(0))?;
            assert_eq!(updates, 1);
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn missing_label_unknown_endpoints_and_invalid_arguments_write_nothing() {
        let (_pool, turn, target, turn_id) = fixture().await;
        let save = tool(turn.clone(), target, turn_id, String::new());
        let missing = save.execute(json!({"from":"Mira","to":"Varro"})).await.unwrap_err();
        assert!(missing.to_string().contains("label is required for a new relationship"));
        let unknown = save.execute(json!({"from":"Nobody","to":"Varro","label":"rivals"})).await.unwrap_err();
        assert!(unknown.to_string().contains("no entity named 'Nobody'"));
        for args in [
            json!(false), json!({"from":null,"to":"Varro","label":"rivals"}),
            json!({"from":"Mira","to":"Mira","label":"self"}),
            json!({"from":"Mira","to":7,"label":"rivals"}),
            json!({"from":"Mira","to":"Varro","label":null}),
            json!({"from":"Mira","to":"Varro","label":"rivals","direction":null}),
            json!({"from":"Mira","to":"Varro","label":"rivals","direction":"sideways"}),
            json!({"from":"Mira","to":"Varro","label":"rivals","description":false}),
            json!({"from":"Mira","to":"Varro","label":"rivals","stats":null}),
            json!({"from":"Mira","to":"Varro","label":"rivals","unknown":true}),
        ] {
            assert!(save.execute(args).await.is_err());
        }
        turn.with(|conn| {
            assert!(entities::list_entities_sync(conn, turn.story_id(), Some(RELATIONSHIP))?.is_empty());
            Ok(())
        }).await.unwrap();
        assert_eq!(schema()["required"], json!(["from", "to"]));
        assert_eq!(schema()["additionalProperties"], false);
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn reversed_both_to_one_way_is_rejected_instead_of_flipping_meaning() {
        let (_pool, turn, target, turn_id) = fixture().await;
        let save = tool(turn.clone(), target, turn_id, String::new());
        save.execute(json!({"from":"Mira","to":"Varro","label":"allies","direction":"both"})).await.unwrap();
        let error = save.execute(json!({"from":"Varro","to":"Mira","direction":"one_way","description":"must not write"}))
            .await.unwrap_err();
        assert!(error.to_string().contains("this relationship is stored as Mira ↔ Varro; make it one-way in that direction, or record the other direction separately"));
        turn.with(|conn| {
            let link = entities::list_entities_sync(conn, turn.story_id(), Some(RELATIONSHIP))?.remove(0).link.unwrap();
            assert_eq!(link.direction, "both");
            assert_eq!(link.description, None);
            Ok(())
        }).await.unwrap();
        let forward = save.execute(json!({"from":"Mira","to":"Varro","direction":"one_way"})).await.unwrap();
        assert_eq!(forward.as_json().unwrap()["name"], "Mira → Varro: allies");
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn stats_and_nullable_description_return_only_short_results() {
        let (_pool, turn, target, turn_id) = fixture().await;
        let save = tool(turn.clone(), target, turn_id, String::new());
        let first = save.execute(json!({"from":"mira","to":"varro","label":"rivals","description":" old rivalry ",
            "stats":[{"attribute":"Affection","delta":2,"reason":"a truce"}]})).await.unwrap();
        assert_eq!(first.as_json().unwrap(), &json!({"name":"Mira → Varro: rivals","kind":"relationship","created":true,
            "stats":[{"attribute":"Affection","before":0.0,"after":2.0}]}));
        let second = save.execute(json!({"from":"Mira","to":"Varro",
            "stats":[{"attribute":"Affection","delta":1,"reason":"cooperation"}]})).await.unwrap();
        assert_eq!(second.as_json().unwrap()["stats"][0], json!({"attribute":"Affection","before":2.0,"after":3.0}));
        turn.with(|conn| {
            let entity = entities::list_entities_sync(conn, turn.story_id(), Some(RELATIONSHIP))?.remove(0);
            assert_eq!(entity.link.unwrap().description, Some("old rivalry".into()));
            assert_eq!(attributes::list_entity_attributes_sync(conn, turn.story_id(), &entity.id)?[0].source, "inferred");
            Ok(())
        }).await.unwrap();
        let cleared = save.execute(json!({"from":"Mira","to":"Varro","description":null})).await.unwrap();
        assert!(cleared.as_json().unwrap().get("stats").is_none());
        turn.with(|conn| {
            assert_eq!(entities::list_entities_sync(conn, turn.story_id(), Some(RELATIONSHIP))?[0].link.as_ref().unwrap().description, None);
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn second_stat_prepare_failure_leaves_fields_stats_registry_and_events_unchanged() {
        let (_pool, turn, target, turn_id) = fixture().await;
        let save = tool(turn.clone(), target, turn_id, String::new());
        save.execute(json!({"from":"Mira","to":"Varro","label":"rivals","description":"original"})).await.unwrap();
        let event_count = turn.with(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM transcript_entries", [], |row| row.get::<_, i64>(0))?)
        }).await.unwrap();
        let error = save.execute(json!({"from":"Mira","to":"Varro","description":"must not remain",
            "stats":[{"attribute":"Affection","delta":2,"reason":"first"},
                {"attribute":"Unknown relationship stat","delta":1,"reason":"second"}]})).await.unwrap_err();
        assert!(error.to_string().contains("stats[1]"));
        assert!(error.to_string().contains("embedding API key"));
        turn.with(|conn| {
            let entity = entities::list_entities_sync(conn, turn.story_id(), Some(RELATIONSHIP))?.remove(0);
            let raw = repository::load_entity_raw(conn, turn.story_id(), &entity.id)?.unwrap();
            assert_eq!(raw.link.unwrap().description, Some("original".into()));
            assert!(attributes::list_entity_attributes_sync(conn, turn.story_id(), &entity.id)?.is_empty());
            assert!(registry::find_exact_match(conn, "Unknown relationship stat")?.is_none());
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM transcript_entries", [], |row| row.get::<_, i64>(0))?, event_count);
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
    }
}
