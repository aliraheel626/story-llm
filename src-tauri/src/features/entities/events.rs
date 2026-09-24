use serde_json::{json, Value};

use crate::features::ledger::model::{kind, LedgerEntry};

#[derive(Debug, Clone, PartialEq)]
pub struct NameAnchor {
    pub name: String,
    pub appearance_anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntityEvent {
    Created {
        entity_id: String,
        kind: String,
        name: String,
        appearance_anchor: Option<String>,
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
                source,
                ..
            } => json!({
                "entity_id": entity_id,
                "kind": kind,
                "name": name,
                "appearance_anchor": appearance_anchor,
                "source": source
            }),
            Self::Updated {
                entity_id,
                before,
                after,
                source,
            } => json!({
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
            }),
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

    pub fn from_entry(entry: &LedgerEntry) -> Option<Self> {
        let string = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
        let entity_id = string(entry.payload.get("entity_id"))?;
        match entry.kind.as_str() {
            kind::ENTITY_CREATED => Some(Self::Created {
                entity_id,
                kind: string(entry.payload.get("kind"))?,
                name: string(entry.payload.get("name"))?,
                appearance_anchor: string(entry.payload.get("appearance_anchor")),
                source: string(entry.payload.get("source")).unwrap_or_default(),
                created_at: entry.created_at.clone(),
            }),
            kind::ENTITY_UPDATED => {
                let after = entry.payload.get("after")?;
                let after_name = string(after.get("name"))?;
                let before = entry.payload.get("before");
                Some(Self::Updated {
                    entity_id,
                    before: NameAnchor {
                        name: string(before.and_then(|value| value.get("name")))
                            .unwrap_or_else(|| after_name.clone()),
                        appearance_anchor: string(
                            before.and_then(|value| value.get("appearance_anchor")),
                        ),
                    },
                    after: NameAnchor {
                        name: after_name,
                        appearance_anchor: string(after.get("appearance_anchor")),
                    },
                    source: string(entry.payload.get("source")).unwrap_or_default(),
                })
            }
            kind::ENTITY_DELETED => Some(Self::Deleted {
                entity_id,
                name: string(entry.payload.get("name")).unwrap_or_default(),
                source: string(entry.payload.get("source")).unwrap_or_default(),
            }),
            kind::ENTITY_ATTRIBUTE_CHANGED => Some(Self::AttributeChanged {
                entity_id,
                attribute_id: string(entry.payload.get("attribute_id"))?,
                attribute_name: string(entry.payload.get("attribute_name")).unwrap_or_default(),
                before: entry.payload.get("before").and_then(Value::as_f64),
                after: entry.payload.get("after").and_then(Value::as_f64)?,
                source: string(entry.payload.get("source")).unwrap_or_else(|| "inferred".into()),
                delta: entry.payload.get("delta").and_then(Value::as_f64),
                cause: string(entry.payload.get("cause")),
            }),
            kind::ENTITY_ATTRIBUTE_REMOVED => Some(Self::AttributeRemoved {
                entity_id,
                attribute_id: string(entry.payload.get("attribute_id"))?,
                attribute_name: string(entry.payload.get("attribute_name")).unwrap_or_default(),
                before: entry
                    .payload
                    .get("before")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                source: string(entry.payload.get("source")).unwrap_or_default(),
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

    fn entry(kind: &str, payload: Value) -> LedgerEntry {
        LedgerEntry {
            id: "event".into(),
            story_id: "story".into(),
            seq: 0,
            kind: kind.into(),
            visibility: "hidden".into(),
            content: None,
            payload,
            target_entry_id: None,
            turn_id: None,
            created_at: "ledger-created-at".into(),
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
    fn parser_uses_ledger_creation_time_and_legacy_attribute_source() {
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
            EntityEvent::Created { created_at, .. } if created_at == "ledger-created-at"
        ));

        let changed = EntityEvent::from_entry(&entry(
            kind::ENTITY_ATTRIBUTE_CHANGED,
            json!({"entity_id":"entity", "attribute_id":"attribute", "after":4.0}),
        ))
        .unwrap();
        assert!(matches!(
            changed,
            EntityEvent::AttributeChanged { source, .. } if source == "inferred"
        ));
    }

    #[test]
    fn parser_skips_entries_missing_required_fields() {
        assert!(EntityEvent::from_entry(&entry(
            kind::ENTITY_CREATED,
            json!({"entity_id":"entity", "name":"Mira"}),
        ))
        .is_none());
        assert!(EntityEvent::from_entry(&entry(
            kind::ENTITY_UPDATED,
            json!({"entity_id":"entity", "after":{}}),
        ))
        .is_none());
    }
}
