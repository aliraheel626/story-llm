use serde_json::{json, Map, Value};

use crate::features::entities::{
    self, attributes,
    model::{Entity, CHARACTER},
    registry::{self, AttributeResolution},
};
use crate::features::turn::TurnTx;
use crate::shared::error::{AppError, AppResult};

pub(super) fn object_args<'a>(
    args: &'a Value,
    allowed: &[&str],
    tool: &str,
) -> AppResult<&'a Map<String, Value>> {
    let fields = args
        .as_object()
        .ok_or_else(|| AppError::Invalid(format!("{tool} expects an object")))?;
    if let Some(key) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(AppError::Invalid(format!("{tool} does not accept '{key}'")));
    }
    Ok(fields)
}

pub(super) fn required_string(fields: &Map<String, Value>, field: &str) -> AppResult<String> {
    let value = fields
        .get(field)
        .ok_or_else(|| AppError::Invalid(format!("{field} is required")))?;
    let value = value
        .as_str()
        .ok_or_else(|| AppError::Invalid(format!("{field} must be a string")))?
        .trim();
    if value.is_empty() {
        return Err(AppError::Invalid(format!("{field} must not be empty")));
    }
    Ok(value.to_string())
}

pub(super) fn optional_string(
    fields: &Map<String, Value>,
    field: &str,
) -> AppResult<Option<String>> {
    fields.get(field).map(|_| required_string(fields, field)).transpose()
}

pub(super) fn nullable_field(
    fields: &Map<String, Value>,
    field: &str,
) -> AppResult<Option<Option<String>>> {
    match fields.get(field) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) => {
            let value = value.trim();
            Ok(Some((!value.is_empty()).then(|| value.to_string())))
        }
        _ => Err(AppError::Invalid(format!("{field} must be a string or null"))),
    }
}

pub(super) fn resolve_entity(
    conn: &rusqlite::Connection,
    story_id: &str,
    reference: &str,
) -> AppResult<Entity> {
    let reference = reference.trim();
    let entities = entities::list_entities_sync(conn, story_id, None)?;
    if let Some(entity) = entities.iter().find(|entity| entity.id == reference) {
        return Ok(entity.clone());
    }
    if let Some((from, to)) = reference
        .split_once('→')
        .or_else(|| reference.split_once("->"))
    {
        let character = |name: &str| {
            entities.iter().find(|entity| {
                entity.kind == CHARACTER && entity.name.eq_ignore_ascii_case(name.trim())
            })
        };
        if let (Some(from), Some(to)) = (character(from), character(to)) {
            if let Some(entity) = entities.iter().find(|entity| {
                entity.link.as_ref().is_some_and(|link| {
                    (link.from_id == from.id && link.to_id == to.id)
                        || (link.direction == "both" && link.from_id == to.id && link.to_id == from.id)
                })
            }) {
                return Ok(entity.clone());
            }
        }
    } else if let Some(entity) = entities.iter().find(|entity| {
        entity.kind == CHARACTER && entity.name.eq_ignore_ascii_case(reference)
    }) {
        return Ok(entity.clone());
    }
    Err(AppError::Invalid(format!("no entity named '{reference}'")))
}

pub(super) fn resolve_character(
    conn: &rusqlite::Connection,
    story_id: &str,
    reference: &str,
) -> AppResult<Entity> {
    let entity = resolve_entity(conn, story_id, reference)?;
    if entity.kind != CHARACTER {
        return Err(AppError::Invalid(format!("'{reference}' must name a character")));
    }
    Ok(entity)
}

#[derive(Clone)]
pub(super) struct StatChange {
    pub attribute: String,
    pub delta: f64,
    pub reason: String,
    pub dramatic: bool,
}

pub(super) fn stats_schema() -> Value {
    json!({
        "type": "array",
        "maxItems": 8,
        "items": {
            "type": "object",
            "properties": {
                "attribute": {"type": "string", "description": "Attribute name, e.g. Affection or Trust."},
                "delta": {"type": "number", "description": "Positive or negative change, on the attribute's own scale."},
                "reason": {"type": "string", "description": "Why this changed, for the audit log."},
                "dramatic": {"type": "boolean", "description": "True only for a major, story-changing swing."}
            },
            "required": ["attribute", "delta", "reason"],
            "additionalProperties": false
        }
    })
}

pub(super) fn parse_stats(value: Option<&Value>) -> AppResult<Option<Vec<StatChange>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| AppError::Invalid("stats must be an array".into()))?;
    if values.len() > 8 {
        return Err(AppError::Invalid("stats must contain at most 8 items".into()));
    }
    values.iter().enumerate().map(|(index, value)| {
        let parse = || {
            let fields = object_args(value, &["attribute", "delta", "reason", "dramatic"], "stat")?;
            let attribute = required_string(fields, "attribute")?;
            let reason = required_string(fields, "reason")?;
            let delta = fields.get("delta").and_then(Value::as_f64).filter(|value| value.is_finite())
                .ok_or_else(|| AppError::Invalid("delta must be a finite number".into()))?;
            let dramatic = match fields.get("dramatic") {
                None => false,
                Some(Value::Bool(value)) => *value,
                _ => return Err(AppError::Invalid("dramatic must be a boolean".into())),
            };
            Ok(StatChange { attribute, delta, reason, dramatic })
        };
        parse().map_err(|error: AppError| AppError::Invalid(format!("stats[{index}]: {error}")))
    }).collect::<AppResult<Vec<_>>>().map(Some)
}

pub(super) struct ResolvedStat {
    change: StatChange,
    resolution: AttributeResolution,
}

pub(super) async fn resolve_stats(
    turn: &TurnTx,
    embedding_api_key: &str,
    kind: &str,
    changes: &[StatChange],
) -> AppResult<Vec<ResolvedStat>> {
    let mut resolved = Vec::with_capacity(changes.len());
    for (index, change) in changes.iter().enumerate() {
        let resolution = registry::resolve_attribute_in_turn(
            turn, embedding_api_key, &change.attribute, kind,
        ).await.map_err(|error| match error {
            AppError::Invalid(message) | AppError::NotFound(message) => {
                AppError::Invalid(format!("stats[{index}]: {message}"))
            }
            error => AppError::Other(format!("stats[{index}]: {error}")),
        })?;
        resolved.push(ResolvedStat { change: change.clone(), resolution });
    }
    Ok(resolved)
}

pub(super) fn apply_resolved_stats(
    conn: &rusqlite::Connection,
    story_id: &str,
    entity_id: &str,
    resolved: Vec<ResolvedStat>,
    target_entry_id: &str,
    turn_id: &str,
) -> AppResult<Vec<Value>> {
    let mut results = Vec::with_capacity(resolved.len());
    for ResolvedStat { change, resolution } in resolved {
        // Another tool may have added this name while the embedding lookup awaited.
        let attribute = if let Some(exact) = registry::find_exact_match(conn, &change.attribute)? {
            exact
        } else {
            match resolution {
                AttributeResolution::Existing(attribute) => attribute,
                AttributeResolution::AddAlias { attribute, alias } => {
                    registry::add_alias(conn, &attribute.id, &alias)?;
                    attribute
                }
                AttributeResolution::Mint(attribute) => {
                    let id = registry::insert_minted_attribute(conn, &attribute)?;
                    registry::find_attribute_by_id(conn, &id)?
                }
            }
        };
        let (before, after) = attributes::apply_attribute_delta(
            conn, story_id, entity_id, &attribute, change.delta, &change.reason,
            target_entry_id, change.dramatic, Some(turn_id),
        )?;
        results.push(json!({"attribute": attribute.canonical_name, "before": before, "after": after}));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::fixture;
    use crate::features::entities::{model::EntityLink, repository};

    #[test]
    fn nullable_fields_trim_clear_and_reject_bad_types() {
        let fields = json!({"set":"  silver hair  ","clear":null,"empty":"  ","bad":false});
        let fields = fields.as_object().unwrap();
        assert_eq!(nullable_field(fields, "missing").unwrap(), None);
        assert_eq!(nullable_field(fields, "set").unwrap(), Some(Some("silver hair".into())));
        assert_eq!(nullable_field(fields, "clear").unwrap(), Some(None));
        assert_eq!(nullable_field(fields, "empty").unwrap(), Some(None));
        assert!(nullable_field(fields, "bad").unwrap_err().to_string().contains("bad must be a string or null"));
    }

    #[test]
    fn stat_parser_checks_limit_unknown_keys_types_and_item_indices() {
        assert!(parse_stats(None).unwrap().is_none());
        assert!(parse_stats(Some(&Value::Null)).is_err());
        let good = json!({"attribute":" Affection ","delta":2,"reason":" reunion "});
        let parsed = parse_stats(Some(&json!([good]))).unwrap().unwrap();
        assert_eq!(parsed[0].attribute, "Affection");
        assert_eq!(parsed[0].reason, "reunion");
        assert!(!parsed[0].dramatic);
        assert!(parse_stats(Some(&json!(vec![good.clone(); 9]))).is_err());
        for bad in [
            json!(false), json!({"attribute":"Trust","delta":"2","reason":"x"}),
            json!({"attribute":"Trust","delta":2,"reason":null}),
            json!({"attribute":"Trust","delta":2,"reason":"x","dramatic":1}),
            json!({"attribute":"Trust","delta":2,"reason":"x","extra":true}),
        ] {
            let error = parse_stats(Some(&json!([good.clone(), bad]))).err().unwrap();
            assert!(error.to_string().contains("stats[1]"));
        }
        assert_eq!(stats_schema()["maxItems"], 8);
        assert_eq!(stats_schema()["items"]["additionalProperties"], false);
    }

    #[tokio::test]
    async fn resolver_reads_present_names_ids_and_relationship_arrows() {
        let (_pool, turn, target, turn_id) = fixture();
        turn.with(|conn| {
            for (id, name) in [("mira", "Mira"), ("varro", "Varro")] {
                repository::create_entity_with_id_sync(conn, id, turn.story_id(), CHARACTER, name, None,
                    "narrator_tool", Some(&target), Some(&turn_id))?;
            }
            let link = repository::create_link_sync(conn, turn.story_id(), EntityLink {
                from_id: "mira".into(), to_id: "varro".into(), label: "siblings".into(),
                direction: "both".into(), description: None,
            }, "narrator_tool", Some(&target), Some(&turn_id))?;
            assert_eq!(resolve_entity(conn, turn.story_id(), " mira ")?.id, "mira");
            assert_eq!(resolve_entity(conn, turn.story_id(), "MIRA")?.id, "mira");
            for reference in ["Mira → Varro", "mira -> varro", "Varro → Mira", "varro -> mira"] {
                assert_eq!(resolve_entity(conn, turn.story_id(), reference)?.id, link.id);
            }
            assert_eq!(resolve_entity(conn, turn.story_id(), &link.id)?.id, link.id);
            assert!(resolve_character(conn, turn.story_id(), &link.id).is_err());
            assert!(resolve_entity(conn, "another-story", "Mira").is_err());
            repository::delete_entity_sync(conn, turn.story_id(), "varro")?;
            assert!(resolve_entity(conn, turn.story_id(), "Varro").is_err());
            assert!(resolve_entity(conn, turn.story_id(), &link.id).is_err());
            assert!(resolve_entity(conn, turn.story_id(), "Mira -> Varro").is_err());
            Ok(())
        }).await.unwrap();
        turn.rollback().await.unwrap();
    }
}
