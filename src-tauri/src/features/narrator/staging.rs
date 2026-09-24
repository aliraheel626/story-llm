//! In-memory world-state changes for a narrator turn, committed with its passage.

use std::{collections::HashMap, sync::Arc};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::features::entities::{
    self,
    attributes::{self, clamp_delta},
    model::{AttributeRegistryEntry, Entity},
};
use crate::features::ledger;
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollFactor {
    pub entity_id: String,
    pub entity_name: String,
    pub attribute_id: String,
    pub attribute_name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RollOutcome {
    pub chance_percent: u8,
    pub seed: i64,
    pub roll: i64,
    pub needed: i64,
    pub outcome: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct PendingRoll {
    pub output: RollOutcome,
    pub reason: Option<String>,
    pub chance_source: &'static str,
    pub factors: Vec<RollFactor>,
}

#[derive(Debug, Clone)]
struct PendingToolCall {
    tool: String,
    args: serde_json::Value,
    result: serde_json::Value,
    ok: bool,
    label: String,
}

pub(super) fn chance_from_factors(factors: &[RollFactor]) -> u8 {
    let normalized = |factor: &RollFactor| (factor.value - factor.min) / (factor.max - factor.min);
    let actor = normalized(&factors[0]);
    let opponent = factors.get(1).map(normalized).unwrap_or(0.5);
    (50.0 + 50.0 * (actor - opponent)).round().clamp(0.0, 100.0) as u8
}

pub(super) fn resolve_roll(chance_percent: u8) -> RollOutcome {
    assert!(chance_percent <= 100, "chance must be between 0 and 100");
    let seed = rand::random::<u32>() as i64;
    let mut rng = StdRng::seed_from_u64(seed as u64);
    let roll = rng.gen_range(0..100);
    let needed = 100 - i64::from(chance_percent);
    RollOutcome {
        chance_percent,
        seed,
        roll,
        needed,
        outcome: if roll >= needed { "success" } else { "failure" },
    }
}

fn persist_roll(
    conn: &rusqlite::Connection,
    entry_id: &str,
    turn_id: &str,
    pending: &PendingRoll,
) -> AppResult<()> {
    let base = ledger::repository::get_entry(conn, entry_id)?;
    ledger::repository::append_entry(
        conn,
        &base.story_id,
        ledger::model::kind::DICEROLL,
        "hidden",
        Some(&format!(
            "Dice-roll outcome: rolled {} with {}% chance and got {}.",
            pending.output.roll, pending.output.chance_percent, pending.output.outcome
        )),
        &json!({
            "chance_percent": pending.output.chance_percent,
            "roll": pending.output.roll,
            "needed": pending.output.needed,
            "outcome": pending.output.outcome,
            "reason": pending.reason,
            "chance_source": pending.chance_source,
            "factors": pending.factors,
            "seed": pending.output.seed,
        }),
        Some(entry_id),
        Some(turn_id),
    )?;
    Ok(())
}

/// One staged write, replayed in call order at commit.
#[derive(Debug, Clone)]
pub(super) enum PendingOp {
    MintAttribute(AttributeRegistryEntry),
    AddAlias {
        attribute: AttributeRegistryEntry,
        alias: String,
    },
    /// Generated at staging time so later calls can reference pending entities.
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

/// Everything a narrator turn's tool calls read and write before becoming durable.
pub struct TurnStaging {
    pub(super) pool: Pool,
    story_id: String,
    pub(super) pending: Vec<PendingOp>,
    tool_calls: Vec<PendingToolCall>,
}

impl TurnStaging {
    pub fn new(pool: Pool, story_id: String) -> Self {
        Self {
            pool,
            story_id,
            pending: Vec::new(),
            tool_calls: Vec::new(),
        }
    }

    /// Committed entities plus staged creates/updates, visible mid-turn.
    pub(super) fn effective_entities(
        &self,
        kind: Option<&str>,
        name: Option<&str>,
    ) -> AppResult<Vec<Entity>> {
        let conn = self.pool.get()?;
        let mut list = entities::list_entities_sync(&conn, &self.story_id, kind)?;
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

    pub(super) fn find_effective_entity(&self, entity_id: &str) -> AppResult<Option<Entity>> {
        Ok(self
            .effective_entities(None, None)?
            .into_iter()
            .find(|e| e.id == entity_id))
    }

    /// Find by effective name or stage a new entity with a stable ID.
    pub(super) fn resolve_or_stage_entity(
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

    /// Committed value plus staged deltas, without initializing a new row.
    pub(super) fn effective_attribute_state(
        &self,
        entity_id: &str,
        attribute: &AttributeRegistryEntry,
    ) -> AppResult<(f64, bool)> {
        let conn = self.pool.get()?;
        let (value, source) =
            attributes::peek_entity_attribute(&conn, &self.story_id, entity_id, attribute)?;
        let locked = source.as_deref() == Some("user");
        Ok((
            fold_pending_delta(value, entity_id, &attribute.id, &self.pending, locked),
            locked,
        ))
    }

    pub(super) fn stage_attribute_delta(
        &mut self,
        entity_id: &str,
        attribute: AttributeRegistryEntry,
        delta: f64,
        cause: String,
        dramatic: bool,
    ) -> AppResult<(f64, f64, bool)> {
        let (before, locked) = self.effective_attribute_state(entity_id, &attribute)?;
        if locked {
            return Ok((before, before, false));
        }
        let after = clamp_delta(before, delta, dramatic, &attribute);
        self.pending.push(PendingOp::AdjustAttribute {
            entity_id: entity_id.to_string(),
            attribute,
            delta,
            cause,
            dramatic,
        });
        Ok((before, after, true))
    }

    pub(super) fn stage_entity_update(
        &mut self,
        id: String,
        name: String,
        appearance_anchor: Option<String>,
    ) {
        self.pending.push(PendingOp::UpdateEntity {
            id,
            name,
            appearance_anchor,
        });
    }

    pub(crate) fn stage_tool_call(
        &mut self,
        tool: String,
        args: serde_json::Value,
        result: serde_json::Value,
        ok: bool,
        label: String,
    ) {
        self.tool_calls.push(PendingToolCall {
            tool,
            args,
            result,
            ok,
            label,
        });
    }

    pub(super) fn stage_roll(
        &mut self,
        chance_percent: u8,
        reason: Option<String>,
        chance_source: &'static str,
        factors: Vec<RollFactor>,
    ) -> RollOutcome {
        let output = resolve_roll(chance_percent);
        self.pending.push(PendingOp::Roll(PendingRoll {
            output,
            reason,
            chance_source,
            factors,
        }));
        output
    }

    pub(super) fn roll_factor(
        &self,
        entity_id: &str,
        attribute_name: &str,
    ) -> AppResult<RollFactor> {
        let entity = self.find_effective_entity(entity_id)?.ok_or_else(|| {
            AppError::NotFound(format!("entity {entity_id} not found in this story"))
        })?;
        let (committed, registry_match) = {
            let conn = self.pool.get()?;
            let committed = attributes::list_entity_attributes_for_entities_sync(
                &conn,
                &self.story_id,
                &[entity_id],
            )?;
            (
                committed,
                attributes::find_exact_match(&conn, attribute_name)?,
            )
        };
        let staged_match = self.pending.iter().rev().find_map(|op| match op {
            PendingOp::MintAttribute(attribute) | PendingOp::AdjustAttribute { attribute, .. }
                if attribute
                    .canonical_name
                    .eq_ignore_ascii_case(attribute_name) =>
            {
                Some(&attribute.id)
            }
            PendingOp::AddAlias { attribute, alias }
                if alias.eq_ignore_ascii_case(attribute_name) =>
            {
                Some(&attribute.id)
            }
            _ => None,
        });
        let resolved_id = registry_match
            .as_ref()
            .map(|entry| &entry.id)
            .or(staged_match);
        let committed_value = committed.get(entity_id).and_then(|values| {
            values.iter().find(|value| {
                resolved_id.is_some_and(|id| id == &value.attribute_id)
                    || (resolved_id.is_none()
                        && value.canonical_name.eq_ignore_ascii_case(attribute_name))
            })
        });
        let (attribute_id, canonical_name, value, min, max) =
            if let Some(attribute) = committed_value {
                (
                    attribute.attribute_id.clone(),
                    attribute.canonical_name.clone(),
                    fold_pending_delta(
                        attribute.value,
                        entity_id,
                        &attribute.attribute_id,
                        &self.pending,
                        attribute.source == "user",
                    ),
                    attribute.min,
                    attribute.max,
                )
            } else if let Some(attribute) = self.pending.iter().rev().find_map(|op| match op {
                PendingOp::AdjustAttribute {
                    entity_id: id,
                    attribute,
                    ..
                } if id == entity_id
                    && resolved_id.is_some_and(|resolved| resolved == &attribute.id) =>
                {
                    Some(attribute)
                }
                _ => None,
            }) {
                let (value, _) = self.effective_attribute_state(entity_id, attribute)?;
                (
                    attribute.id.clone(),
                    attribute.canonical_name.clone(),
                    value,
                    attribute.min,
                    attribute.max,
                )
            } else {
                return Err(AppError::NotFound(format!(
                    "attribute {attribute_name} is not set on {}",
                    entity.name
                )));
            };
        if !value.is_finite()
            || !min.is_finite()
            || !max.is_finite()
            || min >= max
            || value < min
            || value > max
        {
            return Err(AppError::Invalid(format!(
                "invalid value or range for {}'s {canonical_name}",
                entity.name
            )));
        }
        Ok(RollFactor {
            entity_id: entity.id,
            entity_name: entity.name,
            attribute_id,
            attribute_name: canonical_name,
            value,
            min,
            max,
        })
    }

    pub(super) fn attribute_snapshot_for_entities(
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
            &self.story_id,
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

    /// Replays operations in order inside the narration transaction.
    pub fn commit(
        &self,
        tx: &rusqlite::Transaction,
        passage_id: &str,
        turn_id: &str,
    ) -> AppResult<()> {
        // Concurrent turns can mint the same name; remap dependent deltas.
        let mut canonical_ids = HashMap::new();
        for op in &self.pending {
            match op {
                PendingOp::MintAttribute(attribute) => {
                    let id = attributes::insert_minted_attribute(tx, attribute)?;
                    canonical_ids.insert(attribute.id.clone(), id);
                }
                PendingOp::AddAlias { attribute, alias } => {
                    let id = canonical_ids.get(&attribute.id).unwrap_or(&attribute.id);
                    attributes::add_alias(tx, id, alias)?;
                }
                PendingOp::CreateEntity {
                    id,
                    kind,
                    name,
                    appearance_anchor,
                } => {
                    entities::create_entity_with_id_in_turn(
                        tx,
                        id,
                        &self.story_id,
                        kind,
                        name,
                        appearance_anchor.as_deref(),
                        "narrator_tool",
                        Some(passage_id),
                        Some(turn_id),
                    )?;
                }
                PendingOp::UpdateEntity {
                    id,
                    name,
                    appearance_anchor,
                } => {
                    entities::update_entity_in_turn(
                        tx,
                        &self.story_id,
                        id,
                        name,
                        appearance_anchor.as_deref(),
                        "narrator_tool",
                        Some(passage_id),
                        Some(turn_id),
                    )?;
                }
                PendingOp::AdjustAttribute {
                    entity_id,
                    attribute,
                    delta,
                    cause,
                    dramatic,
                } => {
                    let attribute = if let Some(id) = canonical_ids.get(&attribute.id) {
                        attributes::find_attribute_by_id(tx, id)?
                    } else {
                        attribute.clone()
                    };
                    attributes::apply_attribute_delta_in_turn(
                        tx,
                        &self.story_id,
                        entity_id,
                        &attribute,
                        *delta,
                        cause,
                        passage_id,
                        *dramatic,
                        Some(turn_id),
                    )?;
                }
                PendingOp::Roll(pending) => {
                    let mut pending = pending.clone();
                    for factor in &mut pending.factors {
                        if let Some(id) = canonical_ids.get(&factor.attribute_id) {
                            let canonical = attributes::find_attribute_by_id(tx, id)?;
                            if factor.min != canonical.min || factor.max != canonical.max {
                                return Err(AppError::Invalid(format!(
                                    "{}'s attribute range changed while rolling; retry this turn",
                                    factor.entity_name
                                )));
                            }
                            factor.attribute_id = id.clone();
                            factor.attribute_name = canonical.canonical_name;
                        }
                    }
                    persist_roll(tx, passage_id, turn_id, &pending)?;
                }
            }
        }
        for call in &self.tool_calls {
            ledger::repository::append_entry(
                tx,
                &self.story_id,
                ledger::model::kind::TOOL_CALL,
                "hidden",
                Some(&call.label),
                &json!({
                    "tool": call.tool,
                    "args": call.args,
                    "result": call.result,
                    "ok": call.ok,
                }),
                Some(passage_id),
                Some(turn_id),
            )?;
        }
        Ok(())
    }

    fn staged_attribute(&self, name: &str) -> Option<AttributeRegistryEntry> {
        let name = name.trim();
        self.pending.iter().find_map(|op| match op {
            PendingOp::MintAttribute(attribute)
                if attribute.canonical_name.eq_ignore_ascii_case(name) =>
            {
                Some(attribute.clone())
            }
            PendingOp::AddAlias { attribute, alias } if alias.eq_ignore_ascii_case(name) => {
                Some(attribute.clone())
            }
            _ => None,
        })
    }
}

pub(super) async fn resolve_or_stage_attribute(
    staging: &Arc<Mutex<TurnStaging>>,
    embedding_api_key: &str,
    proposed_name: &str,
    entity_kind: &str,
) -> AppResult<AttributeRegistryEntry> {
    let (pool, story_id) = {
        let staging = staging.lock().await;
        if let Some(attribute) = staging.staged_attribute(proposed_name) {
            return Ok(attribute);
        }
        (staging.pool.clone(), staging.story_id.clone())
    };

    // Similarity may make a network request; do not hold the mutex across it.
    // The caller resolves the OpenRouter embedding key separately from the text-model key.
    let resolution = attributes::resolve_attribute(
        &pool,
        embedding_api_key,
        proposed_name,
        entity_kind,
        &story_id,
    )
    .await?;

    let mut staging = staging.lock().await;
    if let Some(attribute) = staging.staged_attribute(proposed_name) {
        return Ok(attribute);
    }
    match resolution {
        attributes::AttributeResolution::Existing(attribute) => Ok(attribute),
        attributes::AttributeResolution::AddAlias { attribute, alias } => {
            staging.pending.push(PendingOp::AddAlias {
                attribute: attribute.clone(),
                alias,
            });
            Ok(attribute)
        }
        attributes::AttributeResolution::Mint(attribute) => {
            staging
                .pending
                .push(PendingOp::MintAttribute(attribute.clone()));
            Ok(attribute)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::repository::append_entry;
    use chrono::Utc;

    fn setup() -> (Pool, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let story_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json) VALUES (?1, 't', ?2, ?2, '{}')",
            rusqlite::params![story_id, now],
        ).unwrap();
        (pool, story_id)
    }

    fn find_attribute(conn: &rusqlite::Connection, name: &str) -> AttributeRegistryEntry {
        let id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        attributes::find_attribute_by_id(conn, &id).unwrap()
    }

    fn persist_staging(pool: &Pool, story_id: &str, staging: &TurnStaging) {
        let mut conn = pool.get().unwrap();
        let turn_id = ledger::turns::create_turn(&conn, story_id).unwrap();
        let passage = append_entry(
            &conn,
            story_id,
            "narration",
            "visible",
            Some("scene"),
            &json!({}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        staging.commit(&tx, &passage.id, &turn_id).unwrap();
        ledger::turns::set_status(&tx, &turn_id, ledger::turns::COMPLETE).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn staged_entity_is_visible_before_commit_and_not_duplicated() {
        let (pool, story_id) = setup();
        let mut staging = TurnStaging::new(pool, story_id);
        let (created, was_new) = staging
            .resolve_or_stage_entity("character", "Mira", Some("silver hair"))
            .unwrap();
        assert!(was_new);
        assert!(staging
            .find_effective_entity(&created.id)
            .unwrap()
            .is_some());
        let (found, was_new_again) = staging
            .resolve_or_stage_entity("character", "mira", None)
            .unwrap();
        assert!(!was_new_again);
        assert_eq!(found.id, created.id);
        assert_eq!(staging.pending.len(), 1);
    }

    #[test]
    fn staged_attribute_delta_folds_onto_the_registry_midpoint() {
        let (pool, story_id) = setup();
        let attribute = find_attribute(&pool.get().unwrap(), "Trust");
        let mut staging = TurnStaging::new(pool, story_id);
        let (entity, _) = staging
            .resolve_or_stage_entity("character", "Mira", None)
            .unwrap();
        let midpoint = (attribute.min + attribute.max) / 2.0;
        assert_eq!(
            staging
                .effective_attribute_state(&entity.id, &attribute)
                .unwrap()
                .0,
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
                .effective_attribute_state(&entity.id, &attribute)
                .unwrap()
                .0,
            midpoint + 2.0
        );
    }

    #[tokio::test]
    async fn newly_minted_attributes_are_staged_and_reused_without_leaking() {
        let (pool, story_id) = setup();
        let staging = Arc::new(Mutex::new(TurnStaging::new(pool.clone(), story_id)));
        let first = resolve_or_stage_attribute(&staging, "", "Resonance", "artifact")
            .await
            .unwrap();
        let second = resolve_or_stage_attribute(&staging, "", "resonance", "artifact")
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        let count: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM attribute_registry WHERE id = ?1",
                [&first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert!(matches!(
            staging.lock().await.pending.as_slice(),
            [PendingOp::MintAttribute(_)]
        ));
    }

    #[test]
    fn aliases_are_staged_until_the_narration_commits() {
        let (pool, story_id) = setup();
        let attribute = find_attribute(&pool.get().unwrap(), "Stealth");
        let alias = "furtive movement";
        let mut staging = TurnStaging::new(pool.clone(), story_id.clone());
        staging.pending.push(PendingOp::AddAlias {
            attribute: attribute.clone(),
            alias: alias.into(),
        });
        assert_eq!(staging.staged_attribute(alias).unwrap().id, attribute.id);
        assert!(attributes::find_exact_match(&pool.get().unwrap(), alias)
            .unwrap()
            .is_none());
        drop(staging);
        assert!(attributes::find_exact_match(&pool.get().unwrap(), alias)
            .unwrap()
            .is_none());

        let mut successful = TurnStaging::new(pool.clone(), story_id.clone());
        successful.pending.push(PendingOp::AddAlias {
            attribute: attribute.clone(),
            alias: alias.into(),
        });
        persist_staging(&pool, &story_id, &successful);
        assert_eq!(
            attributes::find_exact_match(&pool.get().unwrap(), alias)
                .unwrap()
                .unwrap()
                .id,
            attribute.id
        );
    }

    #[test]
    fn commit_applies_every_staged_op_atomically() {
        let (pool, story_id) = setup();
        let attribute = find_attribute(&pool.get().unwrap(), "Trust");
        let mut staging = TurnStaging::new(pool.clone(), story_id.clone());
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
        persist_staging(&pool, &story_id, &staging);
        let conn = pool.get().unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM story_entity_state WHERE entity_id = ?1",
                [&entity.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Mira");
        let value: f64 = conn
            .query_row(
                "SELECT value FROM entity_attributes WHERE entity_id = ?1 AND attribute_id = ?2",
                rusqlite::params![entity.id, attribute.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, (attribute.min + attribute.max) / 2.0 + 3.0);
    }

    #[test]
    fn tool_calls_commit_exact_payloads_in_order_with_target_and_turn() {
        let (pool, story_id) = setup();
        let mut staging = TurnStaging::new(pool.clone(), story_id.clone());
        staging.stage_tool_call(
            "roll_check".into(),
            json!({"chance_percent": 40}),
            json!({"outcome": "failure"}),
            true,
            "Rolling for escape".into(),
        );
        staging.stage_tool_call(
            "adjust_entity_attribute".into(),
            json!({"delta": 1}),
            json!({"applied": false}),
            true,
            "Adjusting Trust".into(),
        );
        staging.stage_tool_call(
            "get_entities".into(),
            json!({}),
            json!("database unavailable"),
            false,
            "Checking who's here".into(),
        );
        let mut conn = pool.get().unwrap();
        let turn_id = ledger::turns::create_turn(&conn, &story_id).unwrap();
        let passage = append_entry(
            &conn,
            &story_id,
            "narration",
            "visible",
            Some("scene"),
            &json!({}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        staging.commit(&tx, &passage.id, &turn_id).unwrap();
        ledger::turns::set_status(&tx, &turn_id, ledger::turns::COMPLETE).unwrap();
        tx.commit().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content, payload_json, target_entry_id, turn_id
                 FROM ledger_entries WHERE kind = ?1 ORDER BY seq",
            )
            .unwrap();
        let calls = stmt
            .query_map([ledger::model::kind::TOOL_CALL], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls
                .iter()
                .map(|(content, _, _, _)| content.as_str())
                .collect::<Vec<_>>(),
            [
                "Rolling for escape",
                "Adjusting Trust",
                "Checking who's here"
            ]
        );
        let payloads = calls
            .iter()
            .map(|(_, payload, target, owner)| {
                assert_eq!(target, &passage.id);
                assert_eq!(owner, &turn_id);
                serde_json::from_str::<serde_json::Value>(payload).unwrap()
            })
            .collect::<Vec<_>>();
        for payload in &payloads {
            assert_eq!(payload.as_object().unwrap().len(), 4);
            assert!(payload.get("tool").is_some());
            assert!(payload.get("args").is_some());
            assert!(payload.get("result").is_some());
            assert!(payload.get("ok").is_some());
        }
        assert_eq!(payloads[0]["result"]["outcome"], json!("failure"));
        assert_eq!(payloads[0]["ok"], json!(true));
        assert_eq!(payloads[1]["result"]["applied"], json!(false));
        assert_eq!(payloads[1]["ok"], json!(true));
        assert_eq!(payloads[2]["result"], json!("database unavailable"));
        assert_eq!(payloads[2]["ok"], json!(false));
    }

    #[test]
    fn chance_roll_has_exact_zero_and_hundred_percent_bounds() {
        for _ in 0..100 {
            let impossible = resolve_roll(0);
            assert!((0..100).contains(&impossible.roll));
            assert_eq!(impossible.needed, 100);
            assert_eq!(impossible.outcome, "failure");
            let certain = resolve_roll(100);
            assert!((0..100).contains(&certain.roll));
            assert_eq!(certain.needed, 0);
            assert_eq!(certain.outcome, "success");
        }
    }

    #[test]
    fn chance_roll_seed_replays_the_draw() {
        for chance in [1, 25, 50, 75, 99] {
            let result = resolve_roll(chance);
            let mut rng = StdRng::seed_from_u64(result.seed as u64);
            assert_eq!(rng.gen_range(0..100), result.roll);
            assert_eq!(result.needed, 100 - i64::from(chance));
            assert_eq!(result.outcome == "success", result.roll >= result.needed);
        }
    }
}
