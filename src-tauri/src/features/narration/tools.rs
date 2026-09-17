//! Tools the narrator calls mid-generation: rolling dice, illustrating scenes,
//! and reading, creating, and updating entities and their attributes. Replaces
//! the old former classify/resolve/update pipeline — the narrator
//! now discovers and records world state itself instead of being handed
//! pre-computed context.
//!
//! Story-state writes are staged in `TurnStaging` during generation and only
//! committed atomically alongside the narration. Attribute registry entries
//! and aliases resolve eagerly so every staged operation holds a canonical id.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rig_agent::tool::{DynamicTool, PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::json;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::ai::TextModelConfig;
use crate::features::dicerolls::{
    model::DiceMode,
    resolve::{self, PendingRoll, ResolveInput},
};
use crate::features::entities::{
    self,
    attributes::{self, clamp_delta},
    model::{AttributeRegistryEntry, Entity},
};
use crate::features::images::model::ImageRequest;
use crate::features::{settings, timeline};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

/// One staged write, replayed in call order at commit.
#[derive(Debug, Clone)]
enum PendingOp {
    /// `id` is generated at staging time (not commit time) so a later tool
    /// call in the same turn can reference an entity that only exists as a
    /// pending op so far.
    CreateEntity {
        id: String,
        kind: String,
        name: String,
        appearance_anchor: Option<String>,
    },
    UpdateEntity {
        id: String,
        name: String,
        appearance_anchor: Option<String>,
    },
    AdjustAttribute {
        entity_id: String,
        attribute: AttributeRegistryEntry,
        delta: f64,
        cause: String,
        dramatic: bool,
    },
    Roll(PendingRoll),
    QueryEntities {
        entity_ids: Vec<String>,
        kind_filter: Option<String>,
        name_filter: Option<String>,
    },
}

fn fold_pending_delta(
    base: f64,
    entity_id: &str,
    attribute_id: &str,
    pending: &[PendingOp],
    locked: bool,
) -> f64 {
    if locked {
        return base;
    }
    pending.iter().fold(base, |value, op| match op {
        PendingOp::AdjustAttribute {
            entity_id: op_entity,
            attribute: op_attribute,
            delta,
            dramatic,
            ..
        } if op_entity == entity_id && op_attribute.id == attribute_id => {
            clamp_delta(value, *delta, *dramatic, op_attribute)
        }
        _ => value,
    })
}

/// Everything a narrator turn's tool calls read and write, before any of it
/// is durable. Built fresh per `submit_turn` call.
pub struct TurnStaging {
    pool: Pool,
    story_id: String,
    branch_id: String,
    pending: Vec<PendingOp>,
}

impl TurnStaging {
    pub fn new(pool: Pool, story_id: String, branch_id: String) -> Self {
        Self {
            pool,
            story_id,
            branch_id,
            pending: Vec::new(),
        }
    }

    /// Committed entities plus any staged create/update replayed on top, so
    /// mid-turn tool calls see each other's not-yet-committed effects.
    fn effective_entities(&self, kind: Option<&str>, name: Option<&str>) -> AppResult<Vec<Entity>> {
        let conn = self.pool.get()?;
        let mut list = entities::list_entities_sync(&conn, &self.story_id, &self.branch_id, kind)?;
        for op in &self.pending {
            match op {
                PendingOp::CreateEntity {
                    id,
                    kind: op_kind,
                    name: op_name,
                    appearance_anchor,
                } => {
                    if kind.is_some_and(|k| k != op_kind) {
                        continue;
                    }
                    list.push(Entity {
                        id: id.clone(),
                        story_id: self.story_id.clone(),
                        branch_id: self.branch_id.clone(),
                        kind: op_kind.clone(),
                        name: op_name.clone(),
                        appearance_anchor: appearance_anchor.clone(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    });
                }
                PendingOp::UpdateEntity {
                    id,
                    name: new_name,
                    appearance_anchor,
                } => {
                    if let Some(e) = list.iter_mut().find(|e| &e.id == id) {
                        e.name = new_name.clone();
                        e.appearance_anchor = appearance_anchor.clone();
                    }
                }
                _ => {}
            }
        }
        if let Some(name) = name {
            list.retain(|e| e.name.eq_ignore_ascii_case(name));
        }
        Ok(list)
    }

    fn find_effective_entity(&self, entity_id: &str) -> AppResult<Option<Entity>> {
        Ok(self
            .effective_entities(None, None)?
            .into_iter()
            .find(|e| e.id == entity_id))
    }

    /// Case-insensitive lookup against the effective view; stages a create if
    /// absent. Returns the entity and whether it was just staged.
    fn resolve_or_stage_entity(
        &mut self,
        kind: &str,
        name: &str,
        appearance_anchor: Option<&str>,
    ) -> AppResult<(Entity, bool)> {
        if let Some(existing) = self
            .effective_entities(Some(kind), Some(name))?
            .into_iter()
            .next()
        {
            return Ok((existing, false));
        }
        let id = Uuid::new_v4().to_string();
        let entity = Entity {
            id: id.clone(),
            story_id: self.story_id.clone(),
            branch_id: self.branch_id.clone(),
            kind: kind.to_string(),
            name: name.to_string(),
            appearance_anchor: appearance_anchor.map(str::to_string),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.pending.push(PendingOp::CreateEntity {
            id,
            kind: kind.to_string(),
            name: name.to_string(),
            appearance_anchor: appearance_anchor.map(str::to_string),
        });
        Ok((entity, true))
    }

    /// Committed value plus any staged deltas for this exact (entity,
    /// attribute) pair folded on top, via the same clamp math the real write
    /// uses — a mid-turn read must not itself write an init event.
    fn effective_attribute_value(
        &self,
        entity_id: &str,
        attribute: &AttributeRegistryEntry,
    ) -> AppResult<f64> {
        Ok(self.effective_attribute_state(entity_id, attribute)?.0)
    }

    fn effective_attribute_state(
        &self,
        entity_id: &str,
        attribute: &AttributeRegistryEntry,
    ) -> AppResult<(f64, bool)> {
        let conn = self.pool.get()?;
        let (value, source) =
            attributes::peek_entity_attribute(&conn, &self.branch_id, entity_id, attribute)?;
        let locked = source.as_deref() == Some("user");
        Ok((
            fold_pending_delta(value, entity_id, &attribute.id, &self.pending, locked),
            locked,
        ))
    }

    fn attribute_snapshot_for_entities(
        &self,
        entity_ids: &[&str],
    ) -> AppResult<HashMap<String, Vec<serde_json::Value>>> {
        let mut out: HashMap<String, Vec<serde_json::Value>> = entity_ids
            .iter()
            .map(|entity_id| ((*entity_id).to_string(), Vec::new()))
            .collect();
        if entity_ids.is_empty() {
            return Ok(out);
        }

        let conn = self.pool.get()?;
        let committed = attributes::list_entity_attributes_for_entities_sync(
            &conn,
            &self.branch_id,
            entity_ids,
        )?;
        for (entity_id, entity_attributes) in committed {
            let snapshot = out.entry(entity_id.clone()).or_default();
            for attribute in entity_attributes {
                let value = fold_pending_delta(
                    attribute.value,
                    &entity_id,
                    &attribute.attribute_id,
                    &self.pending,
                    attribute.source == "user",
                );
                snapshot.push(json!({
                "name": attribute.canonical_name,
                "value": value,
                "min": attribute.min,
                "max": attribute.max,
                }));
            }
        }
        Ok(out)
    }

    /// Replays every staged op against the real transaction, using the same
    /// helpers a live write would use today. Applying each op before the
    /// next is read means chained ops (e.g. two deltas on the same
    /// attribute) compose correctly with no special-casing.
    pub fn commit(&self, tx: &rusqlite::Transaction, passage_id: &str) -> AppResult<()> {
        for op in &self.pending {
            match op {
                PendingOp::CreateEntity {
                    id,
                    kind,
                    name,
                    appearance_anchor,
                } => {
                    entities::create_entity_with_id_sync(
                        tx,
                        id,
                        &self.story_id,
                        &self.branch_id,
                        kind,
                        name,
                        appearance_anchor.as_deref(),
                        "narrator_tool",
                        Some(passage_id),
                    )?;
                }
                PendingOp::UpdateEntity {
                    id,
                    name,
                    appearance_anchor,
                } => {
                    entities::update_entity_sync(
                        tx,
                        &self.branch_id,
                        id,
                        name,
                        appearance_anchor.as_deref(),
                        "narrator_tool",
                        Some(passage_id),
                    )?;
                }
                PendingOp::AdjustAttribute {
                    entity_id,
                    attribute,
                    delta,
                    cause,
                    dramatic,
                } => {
                    attributes::apply_attribute_delta(
                        tx,
                        &self.branch_id,
                        entity_id,
                        attribute,
                        *delta,
                        cause,
                        passage_id,
                        *dramatic,
                    )?;
                }
                PendingOp::Roll(pending) => {
                    resolve::persist_roll(tx, passage_id, pending.clone())?;
                }
                PendingOp::QueryEntities {
                    entity_ids,
                    kind_filter,
                    name_filter,
                } => {
                    let wanted = entity_ids.iter().cloned().collect::<HashSet<_>>();
                    let names =
                        entities::list_entities_sync(tx, &self.story_id, &self.branch_id, None)?
                            .into_iter()
                            .filter(|entity| wanted.contains(&entity.id))
                            .map(|entity| entity.name)
                            .collect::<Vec<_>>();
                    let content = if names.is_empty() {
                        "Looked up: no matching entities".to_string()
                    } else {
                        format!("Looked up: {}", names.join(", "))
                    };
                    timeline::repository::append_entry(
                        tx,
                        &self.branch_id,
                        timeline::model::kind::ENTITY_QUERIED,
                        "hidden",
                        Some(&content),
                        &json!({
                            "entity_ids": entity_ids,
                            "kind_filter": kind_filter,
                            "name_filter": name_filter,
                        }),
                        Some(passage_id),
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn to_tool_error(e: AppError) -> ToolExecutionError {
    ToolExecutionError::other(e.to_string())
}

async fn resolve_or_stage_attribute(
    staging: &Arc<Mutex<TurnStaging>>,
    config: &TextModelConfig,
    proposed_name: &str,
    entity_kind: &str,
) -> Result<AttributeRegistryEntry, ToolExecutionError> {
    let (pool, story_id) = {
        let staging = staging.lock().await;
        (staging.pool.clone(), staging.story_id.clone())
    };

    // Attribute similarity may require a network request. Never retain the
    // staging mutex while awaiting it, or unrelated tool reads would block.
    let resolution = attributes::resolve_attribute(
        &pool,
        &config.api_key,
        proposed_name,
        entity_kind,
        &story_id,
    )
    .await
    .map_err(to_tool_error)?;

    let conn = pool.get().map_err(AppError::Pool).map_err(to_tool_error)?;
    match resolution {
        attributes::AttributeResolution::Existing(attribute) => Ok(attribute),
        attributes::AttributeResolution::AddAlias { attribute, alias } => {
            attributes::add_alias(&conn, &attribute.id, &alias).map_err(to_tool_error)?;
            Ok(attribute)
        }
        attributes::AttributeResolution::Mint(attribute) => {
            let canonical_id =
                attributes::insert_minted_attribute(&conn, &attribute).map_err(to_tool_error)?;
            attributes::find_attribute_by_id(&conn, &canonical_id).map_err(to_tool_error)
        }
    }
}

/// `roll_check` + args → a friendly, generic activity label for the frontend
/// (e.g. "Rolling for Stealth…") — kept next to the tool definitions since it
/// needs to know each tool's argument shape.
pub fn friendly_tool_label(tool_name: &str, args_json: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    let str_arg = |key: &str| args.get(key).and_then(|v| v.as_str()).map(str::to_string);
    match tool_name {
        "roll_check" => format!(
            "Rolling for {}…",
            str_arg("attribute").unwrap_or_else(|| "a check".into())
        ),
        "get_entities" => "Checking who's here…".to_string(),
        "create_entity" => format!(
            "Introducing {}…",
            str_arg("name").unwrap_or_else(|| "someone new".into())
        ),
        "update_entity" => "Updating an entity…".to_string(),
        "adjust_entity_attribute" => {
            format!(
                "Adjusting {}…",
                str_arg("attribute").unwrap_or_else(|| "an attribute".into())
            )
        }
        "illustrate_scene" => "Sketching the scene…".to_string(),
        other => format!("Running {other}…"),
    }
}

pub fn illustrate_scene_tool(image_requests: Arc<Mutex<Vec<ImageRequest>>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "illustrate_scene",
        "Illustrate this moment with a generated scene image. Use sparingly — reserve for a \
         genuinely striking visual moment (a new place revealed, a character's first appearance, \
         a dramatic turn worth seeing); most beats don't need one. Write a vivid, concrete visual \
         description: subject, setting, composition, lighting. Do not mention art style or medium; \
         that's applied separately.",
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "A vivid, concrete visual description of the scene's subject, setting, composition, and lighting."},
                "character_ids": {"type": "array", "items": {"type": "string"}, "description": "Ids of characters visible in the scene, from get_entities."}
            },
            "required": ["description"]
        }),
        move |args: serde_json::Value| {
            let image_requests = image_requests.clone();
            Box::pin(async move {
                let description = args
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("description is required"))?
                    .to_string();
                let character_ids = match args.get("character_ids") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()
                        .ok_or_else(|| {
                            ToolExecutionError::invalid_args("character_ids must be an array")
                        })?
                        .iter()
                        .map(|id| {
                            id.as_str().map(str::to_string).ok_or_else(|| {
                                ToolExecutionError::invalid_args(
                                    "character_ids must contain only strings",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };

                image_requests.lock().await.push(ImageRequest {
                    description,
                    character_ids,
                });
                Ok(ToolOutput::json(json!({"queued": true})))
            })
        },
    )
}

fn roll_check_tool(
    staging: Arc<Mutex<TurnStaging>>,
    config: TextModelConfig,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "roll_check",
        "Roll the dice for an uncertain action. Resolves the player's relevant attribute against an \
         optional opposing entity/attribute and returns the outcome. Call this before narrating the \
         result of any action whose success is genuinely in doubt.",
        json!({
            "type": "object",
            "properties": {
                "attribute": {"type": "string", "description": "The player's attribute this action draws on, e.g. \"Stealth\"."},
                "target_entity_id": {"type": "string", "description": "Id of the opposing entity, from get_entities, if any."},
                "target_attribute": {"type": "string", "description": "The opposing entity's attribute, if target_entity_id is given."},
                "modifier": {"type": "number", "description": "Situational adjustment to success probability, e.g. 0.1 for +10%."}
            },
            "required": ["attribute"]
        }),
        move |args: serde_json::Value| {
            let staging = staging.clone();
            let config = config.clone();
            Box::pin(async move {
                let attribute_name = args
                    .get("attribute")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("attribute is required"))?
                    .to_string();
                let target_entity_id = args.get("target_entity_id").and_then(|v| v.as_str()).map(str::to_string);
                let target_attribute_name = args.get("target_attribute").and_then(|v| v.as_str()).map(str::to_string);
                let modifier = args.get("modifier").and_then(|v| v.as_f64()).unwrap_or(0.0);

                let actor = {
                    let mut staging = staging.lock().await;
                    let (actor, _) = staging
                        .resolve_or_stage_entity("character", "You", None)
                        .map_err(to_tool_error)?;
                    actor
                };

                let actor_attribute = resolve_or_stage_attribute(
                    &staging,
                    &config,
                    &attribute_name,
                    "character",
                )
                .await?;

                let actor_value = staging
                    .lock()
                    .await
                    .effective_attribute_value(&actor.id, &actor_attribute)
                    .map_err(to_tool_error)?;

                let target = match &target_entity_id {
                    Some(id) => staging.lock().await.find_effective_entity(id).map_err(to_tool_error)?,
                    None => None,
                };

                let (target_value, target_attribute) = match (&target, &target_attribute_name) {
                    (Some(target), Some(target_attr_name)) => {
                        let target_attribute = resolve_or_stage_attribute(
                            &staging,
                            &config,
                            target_attr_name,
                            &target.kind,
                        )
                        .await?;
                        let value = staging
                            .lock()
                            .await
                            .effective_attribute_value(&target.id, &target_attribute)
                            .map_err(to_tool_error)?;
                        (Some(value), Some(target_attribute))
                    }
                    _ => (None, None),
                };

                let output = resolve::resolve(ResolveInput {
                    actor_value,
                    actor_min: actor_attribute.min,
                    actor_max: actor_attribute.max,
                    target: target_attribute.as_ref().zip(target_value).map(
                        |(attribute, value)| resolve::TargetAttribute {
                            value,
                            min: attribute.min,
                            max: attribute.max,
                        },
                    ),
                    modifier,
                });

                {
                    let mut staging = staging.lock().await;
                    staging.pending.push(PendingOp::Roll(PendingRoll {
                        actor_entity_id: actor.id.clone(),
                        target_entity_id: target.as_ref().map(|t| t.id.clone()),
                        actor_attribute_id: Some(actor_attribute.id.clone()),
                        target_attribute_id: target_attribute.as_ref().map(|a| a.id.clone()),
                        actor_value,
                        target_value,
                        modifier,
                        output,
                    }));
                }

                Ok(ToolOutput::json(json!({
                    "roll": output.roll, "needed": output.needed,
                    "outcome": output.outcome, "degree": output.degree,
                    "actor_value": actor_value, "target_value": target_value,
                })))
            })
        },
    )
}

fn get_entities_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "get_entities",
        "List known entities (characters, objects, locations) and their current attribute values. \
         Use this to check who or what is present before narrating, rolling, or adjusting state.",
        json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "description": "Filter by kind: character, object, location, relationship, or campaign."},
                "name": {"type": "string", "description": "Filter to an exact (case-insensitive) name match."}
            }
        }),
        move |args: serde_json::Value| {
            let staging = staging.clone();
            Box::pin(async move {
                let kind = args.get("kind").and_then(|v| v.as_str());
                let name = args.get("name").and_then(|v| v.as_str());
                let mut staging = staging.lock().await;
                let entities = staging
                    .effective_entities(kind, name)
                    .map_err(to_tool_error)?;
                let entity_ids = entities
                    .iter()
                    .map(|entity| entity.id.clone())
                    .collect::<Vec<_>>();
                let entity_id_refs = entity_ids.iter().map(String::as_str).collect::<Vec<_>>();
                let mut attributes_by_entity = staging
                    .attribute_snapshot_for_entities(&entity_id_refs)
                    .map_err(to_tool_error)?;
                let mut out = Vec::new();
                for entity in entities {
                    let attributes = attributes_by_entity.remove(&entity.id).unwrap_or_default();
                    out.push(json!({
                        "id": entity.id, "kind": entity.kind, "name": entity.name,
                        "appearance_anchor": entity.appearance_anchor, "attributes": attributes,
                    }));
                }
                let memory = settings::read_narrator_memory_settings(&staging.pool)
                    .map_err(to_tool_error)?;
                if memory.tool_call_persistence {
                    staging.pending.push(PendingOp::QueryEntities {
                        entity_ids,
                        kind_filter: kind.map(str::to_string),
                        name_filter: name.map(str::to_string),
                    });
                }
                Ok(ToolOutput::json(json!({"entities": out})))
            })
        },
    )
}

fn create_entity_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "create_entity",
        "Introduce a new entity (character, object, or location) the story just established. \
         Idempotent by name — calling this for an entity that already exists just returns it.",
        json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "description": "character, object, location, relationship, or campaign."},
                "name": {"type": "string", "description": "The entity's name, exactly as it should appear in the story."},
                "appearance_anchor": {"type": "string", "description": "A short, stable visual description to keep the entity consistent."}
            },
            "required": ["kind", "name"]
        }),
        move |args: serde_json::Value| {
            let staging = staging.clone();
            Box::pin(async move {
                let kind = args
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("kind is required"))?;
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("name is required"))?;
                let appearance_anchor = args.get("appearance_anchor").and_then(|v| v.as_str());

                let mut staging = staging.lock().await;
                let (entity, created) = staging
                    .resolve_or_stage_entity(kind, name, appearance_anchor)
                    .map_err(to_tool_error)?;
                Ok(ToolOutput::json(json!({
                    "id": entity.id, "kind": entity.kind, "name": entity.name, "created": created,
                })))
            })
        },
    )
}

fn update_entity_tool(staging: Arc<Mutex<TurnStaging>>) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "update_entity",
        "Rename an entity or update its appearance description. Look it up with get_entities first.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Entity id from get_entities/create_entity."},
                "name": {"type": "string", "description": "The entity's (possibly unchanged) name."},
                "appearance_anchor": {"type": "string", "description": "The entity's (possibly unchanged) appearance description."}
            },
            "required": ["id", "name"]
        }),
        move |args: serde_json::Value| {
            let staging = staging.clone();
            Box::pin(async move {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolExecutionError::invalid_args("id is required"))?
                    .to_string();
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("name is required"))?
                    .to_string();
                let appearance_anchor = args.get("appearance_anchor").and_then(|v| v.as_str()).map(str::to_string);

                let mut staging = staging.lock().await;
                if staging.find_effective_entity(&id).map_err(to_tool_error)?.is_none() {
                    return Err(ToolExecutionError::invalid_args(format!("no such entity: {id}")));
                }
                staging.pending.push(PendingOp::UpdateEntity {
                    id: id.clone(),
                    name: name.clone(),
                    appearance_anchor: appearance_anchor.clone(),
                });
                Ok(ToolOutput::json(json!({"id": id, "name": name, "appearance_anchor": appearance_anchor})))
            })
        },
    )
}

fn adjust_entity_attribute_tool(
    staging: Arc<Mutex<TurnStaging>>,
    config: TextModelConfig,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "adjust_entity_attribute",
        "Change an entity's attribute by a delta implied by what just happened (an injury, growing \
         trust, a depleted resource). Most changes are minor; only set dramatic for a genuinely \
         major, story-changing swing.",
        json!({
            "type": "object",
            "properties": {
                "entity_id": {"type": "string", "description": "Entity id from get_entities/create_entity. Use \"You\" for the player via get_entities first."},
                "attribute": {"type": "string", "description": "Attribute name, e.g. \"Trust\"."},
                "delta": {"type": "number", "description": "Positive or negative change, on the attribute's own scale."},
                "dramatic": {"type": "boolean", "description": "True only for a major, story-changing swing."},
                "reason": {"type": "string", "description": "Why this changed, for the audit log."}
            },
            "required": ["entity_id", "attribute", "delta", "reason"]
        }),
        move |args: serde_json::Value| {
            let staging = staging.clone();
            let config = config.clone();
            Box::pin(async move {
                let entity_id = args
                    .get("entity_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolExecutionError::invalid_args("entity_id is required"))?
                    .to_string();
                let attribute_name = args
                    .get("attribute")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("attribute is required"))?
                    .to_string();
                let delta = args
                    .get("delta")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| ToolExecutionError::invalid_args("delta is required"))?;
                let dramatic = args.get("dramatic").and_then(|v| v.as_bool()).unwrap_or(false);
                let reason = args.get("reason").and_then(|v| v.as_str()).unwrap_or("narration").to_string();

                let entity = {
                    let staging = staging.lock().await;
                    let entity = staging
                        .find_effective_entity(&entity_id)
                        .map_err(to_tool_error)?
                        .ok_or_else(|| ToolExecutionError::invalid_args(format!("no such entity: {entity_id}")))?;
                    entity
                };

                let attribute = resolve_or_stage_attribute(
                    &staging,
                    &config,
                    &attribute_name,
                    &entity.kind,
                )
                .await?;

                let mut staging = staging.lock().await;
                let (before, locked) = staging
                    .effective_attribute_state(&entity.id, &attribute)
                    .map_err(to_tool_error)?;
                if locked {
                    return Ok(ToolOutput::json(json!({
                        "before": before,
                        "after": before,
                        "applied": false,
                        "reason": "locked to a player-set value",
                    })));
                }
                let after = clamp_delta(before, delta, dramatic, &attribute);
                staging.pending.push(PendingOp::AdjustAttribute {
                    entity_id: entity.id.clone(),
                    attribute,
                    delta,
                    cause: reason,
                    dramatic,
                });
                Ok(ToolOutput::json(json!({"before": before, "after": after, "applied": true})))
            })
        },
    )
}

/// The full narrator tool set for one turn, as the canonical context-free
/// portable tools. This is the definition the tests exercise directly (Rig's
/// `DynamicTool` dispatch is crate-private, so a `DynamicTool`'s callback can't
/// be invoked from here); `narrator_tools` adapts these for the agent runner.
pub fn narrator_portable_tools(
    staging: Arc<Mutex<TurnStaging>>,
    config: TextModelConfig,
    dice_mode: DiceMode,
) -> Vec<PortableDynamicTool> {
    let roll_tool =
        (dice_mode != DiceMode::Never).then(|| roll_check_tool(staging.clone(), config.clone()));
    let mut tools = vec![
        get_entities_tool(staging.clone()),
        create_entity_tool(staging.clone()),
        update_entity_tool(staging.clone()),
        adjust_entity_attribute_tool(staging, config),
    ];
    if let Some(roll_tool) = roll_tool {
        tools.insert(0, roll_tool);
    }
    tools
}

/// The same set as runtime tools for the agent runner. `from_portable`
/// forwards each tool's `ToolOutput`/`ToolExecutionError` unchanged, so both
/// entry points execute identical logic.
pub fn narrator_tools(
    staging: Arc<Mutex<TurnStaging>>,
    config: TextModelConfig,
    dice_mode: DiceMode,
) -> Vec<DynamicTool> {
    narrator_portable_tools(staging, config, dice_mode)
        .into_iter()
        .map(DynamicTool::from_portable)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::timeline::repository::append_entry;
    use chrono::Utc;

    fn setup() -> (Pool, String, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let story_id = Uuid::new_v4().to_string();
        let branch_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json, default_branch_id) VALUES (?1, 't', ?2, ?2, '{}', ?3)",
            rusqlite::params![story_id, now, branch_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO branches (id, story_id, parent_branch_id, forked_at_entry_id, name, created_at) VALUES (?1, ?2, NULL, NULL, 'main', ?3)",
            rusqlite::params![branch_id, story_id, now],
        )
        .unwrap();
        (pool, story_id, branch_id)
    }

    fn find_attribute(conn: &rusqlite::Connection, name: &str) -> AttributeRegistryEntry {
        conn.query_row(
            "SELECT id, canonical_name, aliases_json, entity_kinds_json, min, max, category, is_user_created, created_in_story_id, created_at
             FROM attribute_registry WHERE canonical_name = ?1",
            [name],
            |row| {
                Ok(AttributeRegistryEntry {
                    id: row.get(0)?,
                    canonical_name: row.get(1)?,
                    aliases_json: row.get(2)?,
                    entity_kinds_json: row.get(3)?,
                    min: row.get(4)?,
                    max: row.get(5)?,
                    category: row.get(6)?,
                    is_user_created: row.get::<_, i64>(7)? != 0,
                    created_in_story_id: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .unwrap()
    }

    fn test_config() -> TextModelConfig {
        TextModelConfig {
            model: "test/model".into(),
            // Tool tests use seeded attribute names unless they deliberately
            // exercise the no-candidate mint path, so no embedding call needs
            // this key.
            api_key: String::new(),
            context_window: 0,
        }
    }

    /// The portable tool set, so each tool's real body (arg parsing, error
    /// mapping, staging) can be executed without Rig's private dispatch.
    fn portable_tools(staging: Arc<Mutex<TurnStaging>>) -> Vec<PortableDynamicTool> {
        narrator_portable_tools(staging, test_config(), DiceMode::Classifier)
    }

    fn tool_named<'a>(tools: &'a [PortableDynamicTool], name: &str) -> &'a PortableDynamicTool {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool named {name}"))
    }

    /// Commits staged ops against a real narration entry the way
    /// `append_narration_entry` does — one transaction, no model involved.
    fn persist_staging(pool: &Pool, branch_id: &str, staging: &TurnStaging) {
        let mut conn = pool.get().unwrap();
        let passage = append_entry(
            &conn,
            branch_id,
            "narration",
            "visible",
            Some("scene"),
            &json!({}),
            None,
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        staging.commit(&tx, &passage.id).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn never_dice_mode_omits_only_the_roll_tool() {
        let (pool, story_id, branch_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id, branch_id)));
        let tools = narrator_portable_tools(staging, test_config(), DiceMode::Never);
        let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();

        assert_eq!(
            names,
            vec![
                "get_entities",
                "create_entity",
                "update_entity",
                "adjust_entity_attribute",
            ]
        );
    }

    #[test]
    fn staged_entity_is_visible_before_commit_and_not_duplicated() {
        let (pool, story_id, branch_id) = setup();
        let mut staging = TurnStaging::new(pool, story_id, branch_id);

        let (created, was_new) = staging
            .resolve_or_stage_entity("character", "Mira", Some("silver hair"))
            .unwrap();
        assert!(was_new);
        assert!(staging
            .find_effective_entity(&created.id)
            .unwrap()
            .is_some());

        // A second lookup by name finds the already-staged entity instead of
        // creating a duplicate.
        let (found, was_new_again) = staging
            .resolve_or_stage_entity("character", "mira", None)
            .unwrap();
        assert!(!was_new_again);
        assert_eq!(found.id, created.id);
        assert_eq!(staging.pending.len(), 1);
    }

    #[test]
    fn staged_attribute_delta_folds_onto_the_registry_midpoint() {
        let (pool, story_id, branch_id) = setup();
        let attribute = find_attribute(&pool.get().unwrap(), "Trust");
        let mut staging = TurnStaging::new(pool, story_id, branch_id);
        let (entity, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();

        let midpoint = (attribute.min + attribute.max) / 2.0;
        assert_eq!(
            staging
                .effective_attribute_value(&entity.id, &attribute)
                .unwrap(),
            midpoint
        );

        staging.pending.push(PendingOp::AdjustAttribute {
            entity_id: entity.id.clone(),
            attribute: attribute.clone(),
            delta: 2.0,
            cause: "test".into(),
            dramatic: false,
        });
        assert_eq!(
            staging
                .effective_attribute_value(&entity.id, &attribute)
                .unwrap(),
            midpoint + 2.0
        );
    }

    #[tokio::test]
    async fn newly_minted_attributes_are_resolved_and_reused_immediately() {
        let (pool, story_id, branch_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(
            pool.clone(),
            story_id,
            branch_id.clone(),
        )));
        let config = test_config();

        // This kind has no committed candidates, so resolution mints locally
        // without making an embedding request.
        let first = resolve_or_stage_attribute(&staging, &config, "Resonance", "artifact")
            .await
            .unwrap();
        let second = resolve_or_stage_attribute(&staging, &config, "resonance", "artifact")
            .await
            .unwrap();
        assert_eq!(first.id, second.id);

        let registry_count: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM attribute_registry WHERE id = ?1",
                [&first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(registry_count, 1);
        assert!(staging.lock().await.pending.is_empty());
    }

    #[test]
    fn commit_applies_every_staged_op_atomically() {
        let (pool, story_id, branch_id) = setup();
        let attribute = find_attribute(&pool.get().unwrap(), "Trust");
        let mut staging = TurnStaging::new(pool.clone(), story_id, branch_id.clone());
        let (entity, _) = staging
            .resolve_or_stage_entity("character", "Mira", Some("silver hair"))
            .unwrap();
        staging.pending.push(PendingOp::AdjustAttribute {
            entity_id: entity.id.clone(),
            attribute: attribute.clone(),
            delta: 3.0,
            cause: "test".into(),
            dramatic: false,
        });

        let mut conn = pool.get().unwrap();
        let passage = append_entry(
            &conn,
            &branch_id,
            "narration",
            "visible",
            Some("scene"),
            &json!({}),
            None,
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        staging.commit(&tx, &passage.id).unwrap();
        tx.commit().unwrap();

        let conn = pool.get().unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM branch_entity_state WHERE entity_id = ?1",
                [&entity.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "Mira");
        let value: f64 = conn
            .query_row(
                "SELECT value FROM entity_attributes WHERE entity_id = ?1 AND attribute_id = ?2",
                rusqlite::params![entity.id, attribute.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value, (attribute.min + attribute.max) / 2.0 + 3.0);
    }

    #[test]
    fn query_entities_commit_writes_entity_ids_to_the_timeline() {
        let (pool, story_id, branch_id) = setup();
        let mut staging = TurnStaging::new(pool.clone(), story_id, branch_id.clone());
        let (bob, _) = staging
            .resolve_or_stage_entity("character", "Bob", Some("a weathered coat"))
            .unwrap();
        staging.pending.push(PendingOp::QueryEntities {
            entity_ids: vec![bob.id.clone()],
            kind_filter: Some("character".into()),
            name_filter: Some("Bob".into()),
        });

        let mut conn = pool.get().unwrap();
        let passage = append_entry(
            &conn,
            &branch_id,
            "narration",
            "visible",
            Some("scene"),
            &json!({}),
            None,
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        staging.commit(&tx, &passage.id).unwrap();
        tx.commit().unwrap();

        let (content, payload_json, target_entry_id): (String, String, String) = conn
            .query_row(
                "SELECT content, payload_json, target_entry_id FROM timeline_entries
                 WHERE kind = ?1",
                [timeline::model::kind::ENTITY_QUERIED],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
        assert_eq!(content, "Looked up: Bob");
        assert_eq!(payload["entity_ids"], json!([bob.id]));
        assert_eq!(payload["kind_filter"], json!("character"));
        assert_eq!(payload["name_filter"], json!("Bob"));
        assert_eq!(target_entry_id, passage.id);
    }

    #[test]
    fn friendly_labels_describe_each_tool_and_degrade_safely() {
        assert_eq!(
            friendly_tool_label("roll_check", r#"{"attribute":"Stealth"}"#),
            "Rolling for Stealth…"
        );
        assert_eq!(
            friendly_tool_label("get_entities", "{}"),
            "Checking who's here…"
        );
        assert_eq!(
            friendly_tool_label("create_entity", r#"{"name":"Mira"}"#),
            "Introducing Mira…"
        );
        assert_eq!(
            friendly_tool_label("adjust_entity_attribute", r#"{"attribute":"Trust"}"#),
            "Adjusting Trust…"
        );
        assert_eq!(
            friendly_tool_label("illustrate_scene", "{}"),
            "Sketching the scene…"
        );
        // Missing args and malformed JSON must not panic — they are only
        // labels for a transient UI line.
        assert_eq!(
            friendly_tool_label("roll_check", "{}"),
            "Rolling for a check…"
        );
        assert_eq!(
            friendly_tool_label("roll_check", "not json"),
            "Rolling for a check…"
        );
        assert_eq!(
            friendly_tool_label("mystery_tool", "{}"),
            "Running mystery_tool…"
        );
    }

    #[tokio::test]
    async fn illustrate_scene_tool_queues_requests_and_rejects_empty_descriptions() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let tool = illustrate_scene_tool(requests.clone());

        let output = tool
            .execute(json!({
                "description": "  Mira stands beneath a lightning-split sky.  ",
                "character_ids": ["mira", "watcher"]
            }))
            .await
            .unwrap();
        assert_eq!(output.as_json().unwrap()["queued"], json!(true));

        let requests_guard = requests.lock().await;
        assert_eq!(requests_guard.len(), 1);
        assert_eq!(
            requests_guard[0].description,
            "Mira stands beneath a lightning-split sky."
        );
        assert_eq!(requests_guard[0].character_ids, ["mira", "watcher"]);
        drop(requests_guard);

        let error = tool
            .execute(json!({"description": "   "}))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("description is required"),
            "{error}"
        );
        assert_eq!(requests.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn create_entity_tool_is_idempotent_by_name_and_reports_staging() {
        let (pool, story_id, branch_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id, branch_id)));
        let tools = portable_tools(staging.clone());

        let first = tool_named(&tools, "create_entity")
            .execute(
                json!({"kind": "character", "name": "Mira", "appearance_anchor": "silver hair"}),
            )
            .await
            .unwrap();
        let first = first.as_json().unwrap();
        assert_eq!(first["created"], json!(true));
        assert_eq!(first["name"], json!("Mira"));

        // Same name, different case: the second call must find the staged
        // entity rather than staging a duplicate.
        let second = tool_named(&tools, "create_entity")
            .execute(json!({"kind": "character", "name": "mira"}))
            .await
            .unwrap();
        let second = second.as_json().unwrap();
        assert_eq!(second["created"], json!(false));
        assert_eq!(second["id"], first["id"]);
        assert_eq!(staging.lock().await.pending.len(), 1);

        // A missing required argument is a recoverable error, not a panic.
        let err = tool_named(&tools, "create_entity")
            .execute(json!({"kind": "character", "name": "   "}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("name is required"), "{err}");
    }

    #[tokio::test]
    async fn update_entity_tool_rejects_unknown_ids_and_overlays_renames() {
        let (pool, story_id, branch_id) = setup();
        let mut staging = TurnStaging::new(pool, story_id, branch_id);
        let (mira, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let err = tool_named(&tools, "update_entity")
            .execute(json!({"id": "does-not-exist", "name": "Nobody"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no such entity"), "{err}");

        let out = tool_named(&tools, "update_entity")
            .execute(json!({"id": mira.id, "name": "Mira the Bold"}))
            .await
            .unwrap();
        assert_eq!(out.as_json().unwrap()["name"], json!("Mira the Bold"));

        // The overlay a later tool call sees reflects the staged rename.
        let staging = staging.lock().await;
        assert_eq!(
            staging
                .find_effective_entity(&mira.id)
                .unwrap()
                .unwrap()
                .name,
            "Mira the Bold"
        );
        assert!(staging
            .effective_entities(Some("character"), Some("mira the bold"))
            .unwrap()
            .iter()
            .any(|e| e.id == mira.id));
    }

    #[tokio::test]
    async fn adjust_attribute_tool_previews_clamped_delta_and_rejects_unknown_entity() {
        let (pool, story_id, branch_id) = setup();
        let stealth = find_attribute(&pool.get().unwrap(), "Stealth");
        let mut staging = TurnStaging::new(pool, story_id, branch_id);
        let (mira, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let err = tool_named(&tools, "adjust_entity_attribute")
            .execute(
                json!({"entity_id": "ghost", "attribute": "Stealth", "delta": 1.0, "reason": "x"}),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no such entity"), "{err}");

        // Stealth is a seeded 0-10 character attribute, so a non-dramatic
        // +9 is rate-limited to +3 off its 5.0 midpoint.
        let out = tool_named(&tools, "adjust_entity_attribute")
            .execute(
                json!({"entity_id": mira.id, "attribute": "Stealth", "delta": 9.0, "reason": "sneaking"}),
            )
            .await
            .unwrap();
        let out = out.as_json().unwrap();
        assert_eq!(out["before"], json!(5.0));
        assert_eq!(out["after"], json!(8.0));

        let staging = staging.lock().await;
        let staged = staging
            .pending
            .iter()
            .find_map(|op| match op {
                PendingOp::AdjustAttribute {
                    delta,
                    cause,
                    dramatic,
                    ..
                } => Some((*delta, cause.clone(), *dramatic)),
                _ => None,
            })
            .unwrap();
        assert_eq!(staged, (9.0, "sneaking".to_string(), false));
        assert_eq!(
            staging
                .effective_attribute_value(&mira.id, &stealth)
                .unwrap(),
            8.0
        );
    }

    #[tokio::test]
    async fn roll_check_tool_stages_a_player_roll_against_a_target() {
        let (pool, story_id, branch_id) = setup();
        let mut staging = TurnStaging::new(pool.clone(), story_id, branch_id.clone());
        let (ghoul, _) = staging
            .resolve_or_stage_entity("character", "Ghoul", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let out = tool_named(&tools, "roll_check")
            .execute(json!({
                "attribute": "Stealth",
                "target_entity_id": ghoul.id,
                "target_attribute": "Perception",
                "modifier": 0.1
            }))
            .await
            .unwrap();

        // The player is auto-resolved (staged on first use) and the roll is
        // staged rather than persisted, so a failed turn leaves no trace.
        let staging = staging.lock().await;
        let player = staging
            .effective_entities(Some("character"), Some("You"))
            .unwrap()
            .into_iter()
            .next()
            .expect("actor staged");
        let roll = staging
            .pending
            .iter()
            .find_map(|op| match op {
                PendingOp::Roll(pending) => Some(pending),
                _ => None,
            })
            .expect("roll staged");
        assert_eq!(roll.actor_entity_id, player.id);
        assert_eq!(roll.target_entity_id.as_deref(), Some(ghoul.id.as_str()));
        assert_eq!(roll.actor_value, 5.0);
        assert_eq!(roll.target_value, Some(5.0));
        assert_eq!(roll.modifier, 0.1);

        let out = out.as_json().unwrap();
        assert!(out["roll"].is_i64() || out["roll"].is_u64(), "{out}");
        assert!(out["needed"].is_i64() || out["needed"].is_u64(), "{out}");
        assert!(out["outcome"].is_string(), "{out}");

        persist_staging(&pool, &branch_id, &staging);
        drop(staging);
        let payload_json: String = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT payload_json FROM timeline_entries WHERE kind = ?1 ORDER BY seq DESC LIMIT 1",
                [crate::features::timeline::model::kind::DICEROLL],
                |row| row.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
        assert_eq!(payload["modifiers"]["situational"], json!(0.1));
    }

    #[tokio::test]
    async fn roll_check_without_target_attribute_persists_no_target_value() {
        let (pool, story_id, branch_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(
            pool.clone(),
            story_id,
            branch_id.clone(),
        )));
        let tools = portable_tools(staging.clone());

        let out = tool_named(&tools, "roll_check")
            .execute(json!({"attribute": "Stealth"}))
            .await
            .unwrap();
        assert_eq!(out.as_json().unwrap()["target_value"], json!(null));

        let staging = staging.lock().await;
        let roll = staging
            .pending
            .iter()
            .find_map(|op| match op {
                PendingOp::Roll(pending) => Some(pending),
                _ => None,
            })
            .expect("roll staged");
        assert_eq!(roll.target_value, None);
        persist_staging(&pool, &branch_id, &staging);
        drop(staging);

        let payload_json: String = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT payload_json FROM timeline_entries WHERE kind = ?1 ORDER BY seq DESC LIMIT 1",
                [crate::features::timeline::model::kind::DICEROLL],
                |row| row.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
        assert_eq!(payload["target_value"], json!(null));
    }

    #[tokio::test]
    async fn independently_staged_same_name_mints_commit_to_one_canonical_attribute() {
        let (pool, story_id, branch_id) = setup();
        let first = Arc::new(Mutex::new(TurnStaging::new(
            pool.clone(),
            story_id.clone(),
            branch_id.clone(),
        )));
        let second = Arc::new(Mutex::new(TurnStaging::new(
            pool.clone(),
            story_id,
            branch_id.clone(),
        )));

        let first_entity = first
            .lock()
            .await
            .resolve_or_stage_entity("artifact", "First Prism", None)
            .unwrap()
            .0;
        let second_entity = second
            .lock()
            .await
            .resolve_or_stage_entity("artifact", "Second Prism", None)
            .unwrap()
            .0;
        // The registry resolves eagerly, so both otherwise-independent turns
        // receive the same canonical id before either passage commits.
        let first_attribute =
            resolve_or_stage_attribute(&first, &test_config(), "Resonance", "artifact")
                .await
                .unwrap();
        let second_attribute =
            resolve_or_stage_attribute(&second, &test_config(), "resonance", "artifact")
                .await
                .unwrap();
        assert_eq!(first_attribute.id, second_attribute.id);

        for (staging, entity, attribute, modifier) in [
            (&first, &first_entity, &first_attribute, 0.1),
            (&second, &second_entity, &second_attribute, -0.1),
        ] {
            let output = resolve::resolve(ResolveInput {
                actor_value: 5.0,
                actor_min: 0.0,
                actor_max: 10.0,
                target: None,
                modifier,
            });
            let mut staging = staging.lock().await;
            staging.pending.push(PendingOp::AdjustAttribute {
                entity_id: entity.id.clone(),
                attribute: attribute.clone(),
                delta: 1.0,
                cause: "test".into(),
                dramatic: false,
            });
            staging.pending.push(PendingOp::Roll(PendingRoll {
                actor_entity_id: entity.id.clone(),
                target_entity_id: Some(entity.id.clone()),
                actor_attribute_id: Some(attribute.id.clone()),
                target_attribute_id: Some(attribute.id.clone()),
                actor_value: 5.0,
                target_value: Some(5.0),
                modifier,
                output,
            }));
        }

        {
            let staging = first.lock().await;
            persist_staging(&pool, &branch_id, &staging);
        }
        {
            let staging = second.lock().await;
            persist_staging(&pool, &branch_id, &staging);
        }

        let conn = pool.get().unwrap();
        let (registry_count, canonical_id): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), MIN(id) FROM attribute_registry WHERE lower(canonical_name) = lower('Resonance')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(registry_count, 1);

        let (value_count, distinct_attribute_ids): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT attribute_id) FROM entity_attributes
                 WHERE entity_id IN (?1, ?2)",
                rusqlite::params![first_entity.id, second_entity.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((value_count, distinct_attribute_ids), (2, 1));

        let mut stmt = conn
            .prepare("SELECT payload_json FROM timeline_entries WHERE kind = ?1 ORDER BY seq ASC")
            .unwrap();
        let roll_payloads = stmt
            .query_map([crate::features::timeline::model::kind::DICEROLL], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(roll_payloads.len(), 2);
        for payload in roll_payloads {
            let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(payload["actor_attribute_id"], json!(canonical_id));
            assert_eq!(payload["target_attribute_id"], json!(canonical_id));
        }

        let mut stmt = conn
            .prepare(
                "SELECT content, payload_json FROM timeline_entries
                 WHERE kind = ?1 AND json_extract(payload_json, '$.source') = 'inferred'
                 ORDER BY seq ASC",
            )
            .unwrap();
        let changes = stmt
            .query_map(
                [crate::features::timeline::model::kind::ENTITY_ATTRIBUTE_CHANGED],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(changes.len(), 2);
        for (content, payload) in changes {
            assert!(content.starts_with("Resonance changed"), "{content}");
            let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(payload["attribute_name"], json!("Resonance"));
            assert_eq!(payload["attribute_id"], json!(canonical_id));
        }
    }

    #[tokio::test]
    async fn get_entities_tool_reports_staged_entities_with_attributes() {
        let (pool, story_id, branch_id) = setup();
        let mut staging = TurnStaging::new(pool, story_id, branch_id);
        staging
            .resolve_or_stage_entity("location", "The Drowned Keep", None)
            .unwrap();
        let staging = Arc::new(Mutex::new(staging));
        let tools = portable_tools(staging.clone());

        let out = tool_named(&tools, "get_entities")
            .execute(json!({"kind": "location"}))
            .await
            .unwrap();
        let entities = out.as_json().unwrap()["entities"].clone();
        let entities = entities.as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["name"], json!("The Drowned Keep"));
        assert_eq!(entities[0]["kind"], json!("location"));
        assert_eq!(entities[0]["attributes"], json!([]));
        let staging = staging.lock().await;
        assert!(staging.pending.iter().any(|op| matches!(
            op,
            PendingOp::QueryEntities {
                entity_ids,
                kind_filter: Some(kind),
                name_filter: None,
            } if entity_ids.len() == 1 && kind == "location"
        )));
    }

    #[tokio::test]
    async fn get_entities_stays_ephemeral_when_persistence_is_disabled() {
        let (pool, story_id, branch_id) = setup();
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO settings (key, value) VALUES ('narrator_memory', ?1)",
                [json!({"tool_call_persistence":false,"preamble_mode":"all"}).to_string()],
            )
            .unwrap();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool, story_id, branch_id)));
        let tools = portable_tools(staging.clone());

        tool_named(&tools, "get_entities")
            .execute(json!({}))
            .await
            .unwrap();

        assert!(!staging
            .lock()
            .await
            .pending
            .iter()
            .any(|op| matches!(op, PendingOp::QueryEntities { .. })));
    }

    #[tokio::test]
    async fn committed_tool_delta_does_not_override_a_user_set_attribute() {
        let (pool, story_id, branch_id) = setup();
        let stealth = find_attribute(&pool.get().unwrap(), "Stealth");
        let mut staging = TurnStaging::new(pool.clone(), story_id.clone(), branch_id.clone());
        let (mira, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();

        // Land the entity, then set a value the player owns explicitly.
        persist_staging(&pool, &branch_id, &staging);
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO entity_attributes (branch_id, entity_id, attribute_id, value, source, updated_at, last_event_id)
             VALUES (?1, ?2, ?3, 7.0, 'user', 'now', NULL)",
            rusqlite::params![branch_id, mira.id, stealth.id],
        )
        .unwrap();

        // Even if a delta was staged before the lock became visible, every
        // preview path must mirror commit's no-op behavior.
        let mut stale_preview = TurnStaging::new(pool.clone(), story_id.clone(), branch_id.clone());
        stale_preview.pending.push(PendingOp::AdjustAttribute {
            entity_id: mira.id.clone(),
            attribute: stealth.clone(),
            delta: 5.0,
            cause: "stale preview".into(),
            dramatic: true,
        });
        assert_eq!(
            stale_preview
                .effective_attribute_value(&mira.id, &stealth)
                .unwrap(),
            7.0
        );
        let snapshot = stale_preview
            .attribute_snapshot_for_entities(&[&mira.id])
            .unwrap();
        assert_eq!(snapshot[&mira.id][0]["value"], json!(7.0));

        let staged = Arc::new(Mutex::new(TurnStaging::new(
            pool.clone(),
            story_id,
            branch_id.clone(),
        )));
        let tools = portable_tools(staged.clone());
        let preview = tool_named(&tools, "adjust_entity_attribute")
            .execute(json!({
                "entity_id": mira.id,
                "attribute": "Stealth",
                "delta": 5.0,
                "dramatic": true,
                "reason": "the narrator decided so",
            }))
            .await
            .unwrap();
        let preview = preview.as_json().unwrap();
        assert_eq!(preview["before"], json!(7.0));
        assert_eq!(preview["after"], json!(7.0));
        assert_eq!(preview["applied"], json!(false));
        assert_eq!(preview["reason"], json!("locked to a player-set value"));

        let staged = staged.lock().await;
        assert!(!staged
            .pending
            .iter()
            .any(|op| matches!(op, PendingOp::AdjustAttribute { .. })));
        persist_staging(&pool, &branch_id, &staged);

        // The preamble promises the model that user overrides win; an
        // inferred tool delta must not quietly overwrite this.
        let (value, source): (f64, String) = conn
            .query_row(
                "SELECT value, source FROM entity_attributes WHERE entity_id = ?1 AND attribute_id = ?2",
                rusqlite::params![mira.id, stealth.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(value, 7.0);
        assert_eq!(source, "user");
    }
}
