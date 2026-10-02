//! Dice mechanics and roll payload types.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RollFactor {
    pub entity_id: String,
    pub entity_name: String,
    pub attribute_id: String,
    pub attribute_name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export_to = "../../src/shared/generated/Roll.ts"))]
pub struct RollPayload {
    pub chance_percent: u8,
    #[cfg_attr(test, ts(type = "number"))]
    pub roll: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub needed: i64,
    pub outcome: &'static str,
    pub reason: Option<String>,
    #[cfg_attr(test, ts(type = "\"default\" | \"narrator\" | \"attributes\" | null"))]
    pub chance_source: Option<&'static str>,
    #[cfg_attr(test, ts(inline))]
    pub factors: Vec<RollFactor>,
    #[cfg_attr(test, ts(type = "number"))]
    pub seed: i64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RollOutcome {
    pub chance_percent: u8,
    pub seed: i64,
    pub roll: i64,
    pub needed: i64,
    pub outcome: &'static str,
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

#[cfg(test)]
mod turn_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn export_bindings() {
        use ts_rs::{Config, TS};
        RollPayload::export_all(&Config::new()).expect("failed to export roll bindings");
    }

    #[test]
    fn payload_serializes_with_exact_current_shape() {
        let payload = serde_json::to_value(RollPayload {
            chance_percent: 60,
            roll: 72,
            needed: 40,
            outcome: "success",
            reason: Some("Sneak past the guard".into()),
            chance_source: Some("attributes"),
            factors: vec![RollFactor {
                entity_id: "player".into(),
                entity_name: "You".into(),
                attribute_id: "stealth".into(),
                attribute_name: "Stealth".into(),
                value: 8.0,
                min: 0.0,
                max: 10.0,
            }],
            seed: 42,
        })
        .unwrap();
        assert_eq!(
            payload,
            json!({
                "chance_percent":60,"roll":72,"needed":40,"outcome":"success",
                "reason":"Sneak past the guard","chance_source":"attributes",
                "factors":[{"entity_id":"player","entity_name":"You","attribute_id":"stealth",
                    "attribute_name":"Stealth","value":8.0,"min":0.0,"max":10.0}],"seed":42,
            })
        );
        assert_eq!(payload.as_object().unwrap().len(), 8);
    }

    #[test]
    fn chance_roll_seed_replays_the_draw() {
        for chance in [0, 1, 25, 50, 75, 99, 100] {
            let result = resolve_roll(chance);
            let mut rng = StdRng::seed_from_u64(result.seed as u64);
            assert_eq!(rng.gen_range(0..100), result.roll);
            assert_eq!(result.outcome == "success", result.roll >= result.needed);
        }
    }
}
