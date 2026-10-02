use serde_json::{json, Value};

use crate::features::transcript::model::{kind, TranscriptEntry};

use super::model::EntityLink;

#[derive(Debug, Clone, PartialEq)]
pub struct NameAnchor {
    pub name: String,
    pub appearance_anchor: Option<String>,
    pub link: Option<EntityLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntityEvent {
    Created {
        entity_id: String,
        kind: String,
        name: Option<String>,
        appearance_anchor: Option<String>,
        link: Option<EntityLink>,
        source: String,
        created_at: String,
    },
    Updated {
        entity_id: String,
        before: NameAnchor,
        after: NameAnchor,
        source: String,
    },
    Deleted {
        entity_id: String,
        name: String,
        source: String,
    },
    AttributeChanged {
        entity_id: String,
        attribute_id: String,
        attribute_name: String,
        before: Option<f64>,
        after: f64,
        source: String,
        delta: Option<f64>,
        cause: Option<String>,
    },
    AttributeRemoved {
        entity_id: String,
        attribute_id: String,
        attribute_name: String,
        before: f64,
        source: String,
    },
}

impl EntityEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Created { .. } => kind::ENTITY_CREATED,
            Self::Updated { .. } => kind::ENTITY_UPDATED,
            Self::Deleted { .. } => kind::ENTITY_DELETED,
            Self::AttributeChanged { .. } => kind::ENTITY_ATTRIBUTE_CHANGED,
            Self::AttributeRemoved { .. } => kind::ENTITY_ATTRIBUTE_REMOVED,
        }
    }

    pub fn payload(&self) -> Value {
        match self {
            Self::Created {
                entity_id,
                kind,
                name,
                appearance_anchor,
                link,
                source,
                ..
            } => {
                let mut payload = json!({
                    "entity_id": entity_id,
                    "kind": kind,
                    "name": name,
                    "appearance_anchor": appearance_anchor,
                    "source": source
                });
                if let Some(link) = link {
                    payload["link"] = json!(link);
                }
                payload
            }
            Self::Updated {
                entity_id,
                before,
                after,
                source,
            } => {
                let mut payload = json!({
                    "entity_id": entity_id,
                    "before": {
                        "name": before.name,
                        "appearance_anchor": before.appearance_anchor
                    },
                    "after": {
                        "name": after.name,
                        "appearance_anchor": after.appearance_anchor
                    },
                    "source": source
                });
                if let Some(link) = &before.link {
                    payload["before"]["link"] = json!(link);
                }
                if let Some(link) = &after.link {
                    payload["after"]["link"] = json!(link);
                }
                payload
            }
            Self::Deleted {
                entity_id,
                name,
                source,
            } => json!({"entity_id": entity_id, "name": name, "source": source}),
            Self::AttributeChanged {
                entity_id,
                attribute_id,
                attribute_name,
                before,
                after,
                source,
                delta,
                cause,
            } => {
                let mut payload = json!({
                    "entity_id": entity_id,
                    "attribute_id": attribute_id,
                    "attribute_name": attribute_name,
                    "before": before,
                    "after": after,
                    "source": source
                });
                if let Some(delta) = delta {
                    payload["delta"] = json!(delta);
                }
                if let Some(cause) = cause {
                    payload["cause"] = json!(cause);
                }
                payload
            }
            Self::AttributeRemoved {
                entity_id,
                attribute_id,
                attribute_name,
                before,
                source,
            } => json!({
                "entity_id": entity_id,
                "attribute_id": attribute_id,
                "attribute_name": attribute_name,
                "before": before,
                "source": source
            }),
        }
    }

    pub fn from_entry(entry: &TranscriptEntry) -> Option<Self> {
        let string = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
        let link = |value: Option<&Value>| match value {
            Some(value) => serde_json::from_value::<Option<EntityLink>>(value.clone()).ok(),
            None => Some(None),
        };
        let entity_id = string(entry.payload.get("entity_id"))?;
        let source = string(entry.payload.get("source"))?;
        match entry.kind.as_str() {
            kind::ENTITY_CREATED => Some(Self::Created {
                entity_id,
                kind: string(entry.payload.get("kind"))?,
                name: string(entry.payload.get("name")),
                appearance_anchor: string(entry.payload.get("appearance_anchor")),
                link: link(entry.payload.get("link"))?,
                source,
                created_at: entry.created_at.clone(),
            }),
            kind::ENTITY_UPDATED => {
                let before = entry.payload.get("before")?;
                let after = entry.payload.get("after")?;
                Some(Self::Updated {
                    entity_id,
                    before: NameAnchor {
                        name: string(before.get("name"))?,
                        appearance_anchor: string(before.get("appearance_anchor")),
                        link: link(before.get("link"))?,
                    },
                    after: NameAnchor {
                        name: string(after.get("name"))?,
                        appearance_anchor: string(after.get("appearance_anchor")),
                        link: link(after.get("link"))?,
                    },
                    source,
                })
            }
            kind::ENTITY_DELETED => Some(Self::Deleted {
                entity_id,
                name: string(entry.payload.get("name"))?,
                source,
            }),
            kind::ENTITY_ATTRIBUTE_CHANGED => Some(Self::AttributeChanged {
                entity_id,
                attribute_id: string(entry.payload.get("attribute_id"))?,
                attribute_name: string(entry.payload.get("attribute_name"))?,
                before: match entry.payload.get("before")? {
                    Value::Null => None,
                    value => Some(value.as_f64()?),
                },
                after: entry.payload.get("after").and_then(Value::as_f64)?,
                source,
                delta: entry.payload.get("delta").and_then(Value::as_f64),
                cause: string(entry.payload.get("cause")),
            }),
            kind::ENTITY_ATTRIBUTE_REMOVED => Some(Self::AttributeRemoved {
                entity_id,
                attribute_id: string(entry.payload.get("attribute_id"))?,
                attribute_name: string(entry.payload.get("attribute_name"))?,
                before: entry.payload.get("before").and_then(Value::as_f64)?,
                source,
            }),
            _ => None,
        }
    }

    pub fn entity_id(&self) -> &str {
        match self {
            Self::Created { entity_id, .. }
            | Self::Updated { entity_id, .. }
            | Self::Deleted { entity_id, .. }
            | Self::AttributeChanged { entity_id, .. }
            | Self::AttributeRemoved { entity_id, .. } => entity_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, payload: Value) -> TranscriptEntry {
        TranscriptEntry {
            id: "event".into(),
            story_id: "story".into(),
            seq: 0,
            kind: kind.into(),
            visibility: "hidden".into(),
            content: None,
            payload,
            target_entry_id: None,
            turn_id: None,
            created_at: "transcript-created-at".into(),
        }
    }

    #[test]
    fn payload_omits_absent_delta_and_cause() {
        let event = EntityEvent::AttributeChanged {
            entity_id: "entity".into(),
            attribute_id: "attribute".into(),
            attribute_name: "Accuracy".into(),
            before: None,
            after: 7.0,
            source: "user".into(),
            delta: None,
            cause: None,
        };

        assert_eq!(
            event.payload(),
            json!({
                "entity_id": "entity",
                "attribute_id": "attribute",
                "attribute_name": "Accuracy",
                "before": null,
                "after": 7.0,
                "source": "user"
            })
        );
    }

    #[test]
    fn parser_uses_transcript_creation_time_and_recorded_attribute_source() {
        let created = EntityEvent::from_entry(&entry(
            kind::ENTITY_CREATED,
            json!({
                "entity_id": "entity",
                "kind": "character",
                "name": "Mira",
                "appearance_anchor": null,
                "source": "user"
            }),
        ))
        .unwrap();
        assert!(matches!(
            created,
            EntityEvent::Created { name: Some(name), link: None, created_at, .. }
                if name == "Mira" && created_at == "transcript-created-at"
        ));

        let changed = EntityEvent::from_entry(&entry(
            kind::ENTITY_ATTRIBUTE_CHANGED,
            json!({
                "entity_id":"entity", "attribute_id":"attribute", "attribute_name":"Accuracy",
                "before":3.0, "after":4.0, "source":"inferred"
            }),
        ))
        .unwrap();
        assert!(matches!(
            changed,
            EntityEvent::AttributeChanged { before: Some(before), after, source, .. }
                if before == 3.0 && after == 4.0 && source == "inferred"
        ));
    }

    #[test]
    fn links_round_trip_and_character_payloads_omit_them() {
        let link = EntityLink {
            from_id: "mira".into(),
            to_id: "varro".into(),
            label: "rivals".into(),
            direction: "one_way".into(),
            description: Some("An old competition.".into()),
        };
        let created = EntityEvent::Created {
            entity_id: "relationship:mira:varro".into(),
            kind: "relationship".into(),
            name: None,
            appearance_anchor: None,
            link: Some(link.clone()),
            source: "narrator_tool".into(),
            created_at: "transcript-created-at".into(),
        };
        assert!(created.payload()["name"].is_null());
        assert_eq!(
            EntityEvent::from_entry(&entry(created.kind(), created.payload())),
            Some(created)
        );
        let updated = EntityEvent::Updated {
            entity_id: "relationship:mira:varro".into(),
            before: NameAnchor {
                name: "rivals".into(),
                appearance_anchor: None,
                link: Some(link.clone()),
            },
            after: NameAnchor {
                name: "former rivals".into(),
                appearance_anchor: None,
                link: Some(EntityLink { label: "former rivals".into(), ..link }),
            },
            source: "user".into(),
        };
        assert_eq!(
            EntityEvent::from_entry(&entry(updated.kind(), updated.payload())),
            Some(updated)
        );
        let character = EntityEvent::Created {
            entity_id: "mira".into(),
            kind: "character".into(),
            name: Some("Mira".into()),
            appearance_anchor: Some("silver hair".into()),
            link: None,
            source: "user".into(),
            created_at: "transcript-created-at".into(),
        };
        assert!(character.payload().get("link").is_none());
        assert_eq!(
            EntityEvent::from_entry(&entry(character.kind(), character.payload())),
            Some(character)
        );
        let character_update = EntityEvent::Updated {
            entity_id: "mira".into(),
            before: NameAnchor { name: "Mira".into(), appearance_anchor: None, link: None },
            after: NameAnchor { name: "Mira".into(), appearance_anchor: Some("silver hair".into()), link: None },
            source: "user".into(),
        };
        let payload = character_update.payload();
        assert!(payload["before"].get("link").is_none());
        assert!(payload["after"].get("link").is_none());
        assert_eq!(
            EntityEvent::from_entry(&entry(character_update.kind(), payload)),
            Some(character_update)
        );
    }

    #[test]
    fn parser_skips_entries_missing_required_fields() {
        assert!(EntityEvent::from_entry(&entry(
            kind::ENTITY_CREATED,
            json!({"entity_id":"entity", "name":"Mira", "source":"user"}),
        ))
        .is_none());
        let payload = json!({
            "entity_id":"entity", "source":"user",
            "before":{"name":"Mira", "appearance_anchor":null},
            "after":{"name":"Mira Vale", "appearance_anchor":null}
        });
        assert!(EntityEvent::from_entry(&entry(kind::ENTITY_UPDATED, payload.clone())).is_some());
        for field in ["before", "after", "source"] {
            let mut incomplete = payload.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            assert!(EntityEvent::from_entry(&entry(kind::ENTITY_UPDATED, incomplete)).is_none(), "{field}");
        }
    }
}
