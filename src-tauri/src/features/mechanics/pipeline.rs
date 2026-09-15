//! Stages 1 and 4 of the turn pipeline: the two cheap/fast structured-output
//! model calls that bracket the (deterministic, model-free) resolve step and
//! the (expensive, streamed) narrate step. Both go through
//! `narrator::prompt_typed`, Rig's native structured-output mode — never
//! parsed out of prose.
//!
//! Scoped for a first pass to `submit_turn` (do/say actions only): the actor
//! is always the player, via a synthetic per-story "You" entity, since there
//! is no character-to-player binding yet.

use rusqlite::OptionalExtension;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::attributes::{
    apply_attribute_delta, get_or_init_entity_attribute, resolve_or_create_attribute,
};
use super::resolve::{resolve, ResolveInput, ResolveOutput};
use crate::ai::{self, TextModelConfig};
use crate::features::entities::model::Entity;
use crate::features::{
    entities,
    timeline::{model::kind as timeline_kind, repository as timeline_repository},
};
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiceMode {
    Always,
    Classifier,
    Never,
}

impl DiceMode {
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::Classifier,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Classifier => "classifier",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Attack,
    Persuade,
    Stealth,
    ManipulateObject,
    Observe,
    Move,
    Freeform,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct ClassifyOutput {
    action_type: ActionType,
    /// Name of the entity being acted on, if any — must match one of the
    /// characters listed in the prompt, or be null.
    target_name: Option<String>,
    /// The player's own attribute this action draws on, e.g. "Accuracy".
    actor_attribute: Option<String>,
    /// The target's opposing attribute, e.g. "Evasion".
    target_attribute: Option<String>,
    is_contested: bool,
    needs_roll: bool,
    confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct AttributeDeltaProposal {
    /// "You" for the player, or a character's name exactly as given.
    entity_name: String,
    attribute_name: String,
    /// Positive or negative change, on the attribute's own scale.
    delta: f64,
    /// True only for a genuinely major, story-changing swing — everything
    /// else is rate-limited to a fraction of the attribute's range.
    dramatic: bool,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct UpdateOutput {
    deltas: Vec<AttributeDeltaProposal>,
}

const CLASSIFY_PREAMBLE: &str = "You are a fast, terse classifier for an interactive story's mechanics \
engine. Given the recent scene and the player's action, decide what kind of action it is, who or what \
it targets (if anyone), which attributes are involved, and whether the outcome is uncertain enough to \
warrant a dice roll. Only name a target from the character list you're given, or leave it null. Respond \
with the structured output only.";

const UPDATE_PREAMBLE: &str = "You are a fast, terse classifier for an interactive story's mechanics \
engine. Given a passage of narration that just happened, propose small attribute changes implied by it \
— only for things the narration actually shows changing (injuries, growing trust, a depleted resource, \
and so on). Most changes are minor; only mark `dramatic` for a genuinely major, story-changing swing \
(e.g. a character's death, a total loss of trust). Propose nothing if nothing changed. Respond with the \
structured output only.";

/// What Stage 3 (narrate) needs from Stages 1-2: extra context to fold into
/// the preamble (attribute snapshot + roll outcome, if any), and — if a roll
/// happened — the record to persist once the passage itself is saved.
pub struct StageOneResult {
    pub extra_preamble: String,
    pub roll_to_persist: Option<PendingRoll>,
}

pub struct PendingRoll {
    pub actor_entity_id: String,
    pub target_entity_id: Option<String>,
    pub actor_attribute_id: Option<String>,
    pub target_attribute_id: Option<String>,
    pub actor_value: f64,
    pub target_value: f64,
    pub output: ResolveOutput,
}

fn row_to_entity(row: &rusqlite::Row) -> rusqlite::Result<Entity> {
    Ok(Entity {
        id: row.get(0)?,
        story_id: row.get(1)?,
        branch_id: row.get(2)?,
        kind: row.get(3)?,
        name: row.get(4)?,
        appearance_anchor: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Fetches a character entity by name (case-insensitive), or mints a bare
/// one on the spot. The mechanics engine treats "the world" as generic
/// entities from the start — the player is one ("You"), and any character
/// or target the classifier names that doesn't exist yet becomes one too,
/// rather than requiring it be added by hand first. Appearance/card details
/// can still be filled in later from the Characters panel.
fn get_or_create_character(
    pool: &Pool,
    story_id: &str,
    branch_id: &str,
    name: &str,
    source_entry_id: &str,
) -> AppResult<Entity> {
    let mut conn = pool.get()?;
    let existing: Option<Entity> = conn
        .query_row(
            "SELECT entities.id, entities.story_id, branch_entity_state.branch_id, entities.kind,
                    branch_entity_state.name, branch_entity_state.appearance_anchor, entities.created_at
             FROM entities JOIN branch_entity_state ON branch_entity_state.entity_id = entities.id
             WHERE entities.story_id = ?1 AND branch_entity_state.branch_id = ?2 AND entities.kind = 'character'
               AND branch_entity_state.is_present = 1 AND branch_entity_state.name = ?3 COLLATE NOCASE",
            rusqlite::params![story_id, branch_id, name],
            row_to_entity,
        )
        .optional()?;
    if let Some(e) = existing {
        return Ok(e);
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let entity = entities::create_entity_sync(
        &tx,
        story_id,
        branch_id,
        "character",
        name,
        None,
        "mechanics",
        Some(source_entry_id),
    )?;
    tx.commit()?;
    Ok(entity)
}

fn get_or_create_player_entity(
    pool: &Pool,
    story_id: &str,
    branch_id: &str,
    source_entry_id: &str,
) -> AppResult<Entity> {
    get_or_create_character(pool, story_id, branch_id, "You", source_entry_id)
}

fn list_characters(pool: &Pool, story_id: &str, branch_id: &str) -> AppResult<Vec<Entity>> {
    let conn = pool.get()?;
    entities::list_entities_sync(&conn, story_id, branch_id, Some("character"))
}

fn format_attribute_line(name: &str, value: f64, min: f64, max: f64) -> String {
    format!(
        "{name} {value:.0}/{max:.0}",
        max = max,
        value = value.max(min)
    )
}

/// Stage 1 (classify, cheap) + Stage 2 (resolve, deterministic — no model
/// call). Returns extra preamble text for Stage 3 and a roll to persist
/// alongside the narrated passage, if one happened.
#[allow(clippy::too_many_arguments)]
pub async fn run_classify_and_resolve(
    pool: &Pool,
    config: &TextModelConfig,
    story_id: &str,
    branch_id: &str,
    source_entry_id: &str,
    dice_mode: DiceMode,
    recent_context: &str,
    player_action: &str,
) -> AppResult<StageOneResult> {
    let characters = list_characters(pool, story_id, branch_id)?;
    let player = get_or_create_player_entity(pool, story_id, branch_id, source_entry_id)?;

    let character_names: Vec<&str> = characters
        .iter()
        .filter(|c| c.id != player.id)
        .map(|c| c.name.as_str())
        .collect();
    let classify_prompt = format!(
        "Recent scene:\n{recent_context}\n\nPlayer action: {player_action}\n\nKnown characters in scene (choose target_name from these, or null): {names}",
        names = if character_names.is_empty() { "(none yet)".to_string() } else { character_names.join(", ") }
    );

    let classify: ClassifyOutput =
        match ai::prompt_typed(config, CLASSIFY_PREAMBLE, classify_prompt).await {
            Ok(c) => c,
            Err(_) => {
                // Schema-validated call failed even after Rig's own retry handling —
                // degrade to plain narration rather than blocking the turn.
                return Ok(StageOneResult {
                    extra_preamble: String::new(),
                    roll_to_persist: None,
                });
            }
        };

    let needs_roll = match dice_mode {
        DiceMode::Always => true,
        DiceMode::Never => false,
        DiceMode::Classifier => classify.needs_roll,
    };

    if !needs_roll {
        return Ok(StageOneResult {
            extra_preamble: String::new(),
            roll_to_persist: None,
        });
    }

    let Some(actor_attribute_name) = classify.actor_attribute.clone() else {
        return Ok(StageOneResult {
            extra_preamble: String::new(),
            roll_to_persist: None,
        });
    };

    // Auto-create the target first (sync, cheap) so its attribute lookup —
    // an independent OpenRouter embedding call — can be resolved concurrently
    // with the actor's below instead of paying both round trips in sequence.
    let target_entity = match classify
        .target_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(name) => Some(get_or_create_character(
            pool,
            story_id,
            branch_id,
            name,
            source_entry_id,
        )?),
        None => None,
    };
    let target_attr_name = target_entity
        .as_ref()
        .and(classify.target_attribute.as_ref());

    let (actor_attribute, target_attribute) = if let Some(target_attr_name) = target_attr_name {
        let (actor, target) = tokio::try_join!(
            resolve_or_create_attribute(
                pool,
                &config.api_key,
                &actor_attribute_name,
                "character",
                story_id
            ),
            resolve_or_create_attribute(
                pool,
                &config.api_key,
                target_attr_name,
                "character",
                story_id
            ),
        )?;
        (actor, Some(target))
    } else {
        let actor = resolve_or_create_attribute(
            pool,
            &config.api_key,
            &actor_attribute_name,
            "character",
            story_id,
        )
        .await?;
        (actor, None)
    };

    let actor_value = {
        let conn = pool.get()?;
        get_or_init_entity_attribute(
            &conn,
            branch_id,
            &player.id,
            &actor_attribute,
            source_entry_id,
        )?
    };

    let (target_value, target_attribute, target_entity_id) = if let (
        Some(target),
        Some(target_attribute),
    ) =
        (target_entity.as_ref(), target_attribute)
    {
        let value = {
            let conn = pool.get()?;
            get_or_init_entity_attribute(
                &conn,
                branch_id,
                &target.id,
                &target_attribute,
                source_entry_id,
            )?
        };
        (value, Some(target_attribute), Some(target.id.clone()))
    } else {
        // No identifiable opposing entity/attribute — resolve against a
        // neutral midpoint so a roll can still happen (e.g. picking a lock).
        (
            actor_attribute.min.midpoint(actor_attribute.max),
            None,
            None,
        )
    };

    let output = resolve(ResolveInput {
        actor_value,
        target_value,
        min: actor_attribute.min,
        max: actor_attribute.max,
        modifier: 0.0,
    });

    let target_label = target_entity
        .as_ref()
        .map(|t| format!(" vs {}", t.name))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "[Mechanical outcome — do not contradict: {} check{} needed {}+, rolled {} → {} ({}). Narrate consistent with this; do not describe a different result.]",
        actor_attribute.canonical_name, target_label, output.needed, output.roll, output.outcome, output.degree
    )];

    let mut known = format!(
        "You: {}",
        format_attribute_line(
            &actor_attribute.canonical_name,
            actor_value,
            actor_attribute.min,
            actor_attribute.max
        )
    );
    if let (Some(target), Some(target_attr)) = (target_entity.as_ref(), target_attribute.as_ref()) {
        known.push_str(&format!(
            " | {}: {}",
            target.name,
            format_attribute_line(
                &target_attr.canonical_name,
                target_value,
                target_attr.min,
                target_attr.max
            )
        ));
    }
    lines.push(format!(
        "Known attributes — reflect these in tone/detail where natural: {known}"
    ));

    Ok(StageOneResult {
        extra_preamble: lines.join("\n"),
        roll_to_persist: Some(PendingRoll {
            actor_entity_id: player.id,
            target_entity_id,
            actor_attribute_id: Some(actor_attribute.id),
            target_attribute_id: target_attribute.map(|a| a.id),
            actor_value,
            target_value,
            output,
        }),
    })
}

pub fn persist_roll(
    conn: &rusqlite::Connection,
    entry_id: &str,
    pending: PendingRoll,
) -> AppResult<()> {
    let base = timeline_repository::get_entry(conn, entry_id)?;
    timeline_repository::append_entry(
        conn,
        &base.branch_id,
        timeline_kind::MECHANICAL_RESULT,
        "hidden",
        Some(&format!(
            "Mechanical outcome: rolled {} and got {} ({}).",
            pending.output.roll, pending.output.outcome, pending.output.degree
        )),
        &serde_json::json!({
            "actor_entity_id": pending.actor_entity_id, "target_entity_id": pending.target_entity_id,
            "actor_attribute_id": pending.actor_attribute_id, "target_attribute_id": pending.target_attribute_id,
            "actor_value": pending.actor_value, "target_value": pending.target_value,
            "p_success": pending.output.p_success, "seed": pending.output.seed, "roll": pending.output.roll,
            "outcome": pending.output.outcome, "degree": pending.output.degree, "modifiers": {}
        }),
        Some(entry_id),
    )?;
    Ok(())
}

/// Stage 4 (update, cheap): proposes attribute deltas implied by the just-
/// narrated passage, resolves each proposal's entity/attribute names —
/// auto-creating the entity if the narration introduced someone new — and
/// applies them with clamping and rate-limiting. Best-effort — an
/// attribute that still fails to resolve is skipped, not fatal.
pub async fn run_update(
    pool: &Pool,
    config: &TextModelConfig,
    story_id: &str,
    branch_id: &str,
    entry_id: &str,
    narrated_text: &str,
) -> AppResult<()> {
    let player = get_or_create_player_entity(pool, story_id, branch_id, entry_id)?;

    let prompt = format!("Passage:\n{narrated_text}");
    let update: UpdateOutput = match ai::prompt_typed(config, UPDATE_PREAMBLE, prompt).await {
        Ok(u) => u,
        Err(_) => return Ok(()),
    };

    for proposal in update.deltas {
        let name = proposal.entity_name.trim();
        if name.is_empty() {
            continue;
        }
        let entity = if name.eq_ignore_ascii_case("you") || name.eq_ignore_ascii_case("player") {
            player.clone()
        } else {
            match get_or_create_character(pool, story_id, branch_id, name, entry_id) {
                Ok(e) => e,
                Err(_) => continue,
            }
        };

        let attribute = match resolve_or_create_attribute(
            pool,
            &config.api_key,
            &proposal.attribute_name,
            "character",
            story_id,
        )
        .await
        {
            Ok(a) => a,
            Err(_) => continue,
        };

        let conn = pool.get()?;
        let cause = if proposal.reason.trim().is_empty() {
            "narration".to_string()
        } else {
            proposal.reason.trim().to_string()
        };
        let _ = apply_attribute_delta(
            &conn,
            branch_id,
            &entity.id,
            &attribute,
            proposal.delta,
            &cause,
            entry_id,
            proposal.dramatic,
        );
    }

    Ok(())
}
