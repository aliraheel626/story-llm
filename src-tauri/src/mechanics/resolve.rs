//! Stage 2 of the turn pipeline: pure deterministic resolution, no model
//! call. Given actor/target attribute values (already scaled to their
//! registered min/max) and a combined modifier, computes a success
//! probability, rolls against a fresh seeded RNG, and classifies the degree
//! of the outcome by margin.
//!
//! Roll-high-succeeds (d20-style): the roll (0-99) must land at or above
//! `needed`, where `needed` is set so that exactly `p_success` of the range
//! is a win. `needed` is derived from `p_success` alone, so it isn't
//! persisted separately — display code recomputes it identically.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Never mechanically impossible or guaranteed, however lopsided the gap.
pub const P_FLOOR: f64 = 0.05;
pub const P_CEIL: f64 = 0.95;

pub struct ResolveInput {
    pub actor_value: f64,
    pub target_value: f64,
    /// The attribute's registered range — the gap is normalized against this
    /// so the curve behaves consistently whether an attribute runs 0-10 or
    /// -10-10, rather than assuming a fixed scale.
    pub min: f64,
    pub max: f64,
    /// Combined campaign difficulty + scene override + situational
    /// modifiers, expressed directly as probability points (e.g. 0.05 = +5%).
    pub modifier: f64,
}

pub struct ResolveOutput {
    pub p_success: f64,
    pub seed: i64,
    pub roll: i64,
    /// The roll needed to succeed, e.g. 50 means "50 or higher wins" —
    /// derived from `p_success`, kept alongside it for convenience.
    pub needed: i64,
    pub outcome: &'static str,
    pub degree: &'static str,
}

/// Recomputes `needed` from `p_success` — the frontend calls the equivalent
/// of this in JS so the two never need to agree via a persisted column.
pub fn needed_roll(p_success: f64) -> i64 {
    (100.0 - p_success * 100.0).round() as i64
}

/// Standard logistic-style curve, moderate steepness: the full width of an
/// attribute's own range maps to the full 50-point swing around a 50/50
/// baseline, then modifiers and floor/ceiling clamp it. A one-point edge on
/// a 0-10 attribute barely matters; maxing it out against a bottomed-out
/// opponent approaches (but never reaches) certainty.
pub fn resolve(input: ResolveInput) -> ResolveOutput {
    let range = (input.max - input.min).max(1e-6);
    let gap_normalized = (input.actor_value - input.target_value) / range;
    let p_success = (0.5 + gap_normalized * 0.5 + input.modifier).clamp(P_FLOOR, P_CEIL);

    let seed: i64 = rand::random::<u32>() as i64;
    let mut rng = StdRng::seed_from_u64(seed as u64);
    let roll: i64 = rng.gen_range(0..100);

    let needed = needed_roll(p_success);
    let success = roll >= needed;

    let degree = if success {
        let headroom = (99 - needed).max(1) as f64;
        let margin = (roll - needed) as f64;
        if margin > headroom * 0.6 {
            "critical"
        } else if margin > headroom * 0.25 {
            "strong"
        } else {
            "marginal"
        }
    } else {
        let failroom = needed.max(1) as f64;
        let margin = (needed - roll) as f64;
        if margin > failroom * 0.6 { "critical_failure" } else { "failure" }
    };

    ResolveOutput { p_success, seed, roll, needed, outcome: if success { "success" } else { "failure" }, degree }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_match_is_fifty_fifty() {
        let out = resolve(ResolveInput { actor_value: 5.0, target_value: 5.0, min: 0.0, max: 10.0, modifier: 0.0 });
        assert!((out.p_success - 0.5).abs() < 1e-9);
        assert_eq!(out.needed, 50);
    }

    #[test]
    fn max_gap_clamps_to_ceiling_not_certainty() {
        let out = resolve(ResolveInput { actor_value: 10.0, target_value: 0.0, min: 0.0, max: 10.0, modifier: 0.0 });
        assert!((out.p_success - P_CEIL).abs() < 1e-9);
    }

    #[test]
    fn max_gap_reversed_clamps_to_floor_not_impossibility() {
        let out = resolve(ResolveInput { actor_value: 0.0, target_value: 10.0, min: 0.0, max: 10.0, modifier: 0.0 });
        assert!((out.p_success - P_FLOOR).abs() < 1e-9);
    }

    #[test]
    fn negative_range_scales_correctly() {
        // Trust-style attribute on -10..10: a full-range gap should behave
        // the same as a 0..10 full-range gap.
        let out = resolve(ResolveInput { actor_value: 10.0, target_value: -10.0, min: -10.0, max: 10.0, modifier: 0.0 });
        assert!((out.p_success - P_CEIL).abs() < 1e-9);
    }

    #[test]
    fn seed_reproduces_the_same_roll() {
        let out = resolve(ResolveInput { actor_value: 7.0, target_value: 3.0, min: 0.0, max: 10.0, modifier: 0.0 });
        let mut rng = StdRng::seed_from_u64(out.seed as u64);
        let replayed: i64 = rng.gen_range(0..100);
        assert_eq!(replayed, out.roll);
    }

    #[test]
    fn roll_at_or_above_needed_succeeds() {
        let out = resolve(ResolveInput { actor_value: 5.0, target_value: 5.0, min: 0.0, max: 10.0, modifier: 0.0 });
        assert_eq!(out.outcome == "success", out.roll >= out.needed);
    }
}
