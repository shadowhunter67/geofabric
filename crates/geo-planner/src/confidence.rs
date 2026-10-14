//! Confidence score (ADR-010): how much the planner trusts its own decision about one engine.
//!
//! A weighted geometric mean of seven factors in (0, 1]. Geometric, because one terrible input (say, the
//! runtime was never probed) must drag the whole score down instead of being averaged away. Every factor
//! carries its value, weight and a reason so the UI can show *why* a plan is low-confidence.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ConfidenceFactor {
    pub name: &'static str,
    pub value: f64,
    pub weight: f64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Confidence {
    pub score: f64,
    pub label: &'static str,
    pub factors: Vec<ConfidenceFactor>,
}

pub const HIGH: f64 = 0.75;
pub const MEDIUM: f64 = 0.5;

pub fn label_for(score: f64) -> &'static str {
    if score >= HIGH {
        "high"
    } else if score >= MEDIUM {
        "medium"
    } else {
        "low"
    }
}

/// Factor weights; they sum to 1. Estimate quality and health dominate: they are the two things that most
/// often made the previous planner wrong or unsafe.
pub const W_ESTIMATE: f64 = 0.25;
pub const W_COVERING: f64 = 0.10;
pub const W_PROBE: f64 = 0.15;
pub const W_HEALTH: f64 = 0.15;
pub const W_NETWORK: f64 = 0.15;
pub const W_FAMILIARITY: f64 = 0.10;
pub const W_OPS: f64 = 0.10;

pub fn factor(name: &'static str, value: f64, weight: f64, detail: impl Into<String>) -> ConfidenceFactor {
    ConfidenceFactor { name, value: value.clamp(0.01, 1.0), weight, detail: detail.into() }
}

pub fn combine(factors: Vec<ConfidenceFactor>) -> Confidence {
    let wsum: f64 = factors.iter().map(|f| f.weight).sum();
    let score = if wsum <= 0.0 { 0.0 } else { (factors.iter().map(|f| f.weight * f.value.ln()).sum::<f64>() / wsum).exp() };
    Confidence { score, label: label_for(score), factors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one() {
        let s = W_ESTIMATE + W_COVERING + W_PROBE + W_HEALTH + W_NETWORK + W_FAMILIARITY + W_OPS;
        assert!((s - 1.0).abs() < 1e-12);
    }

    #[test]
    fn one_bad_factor_drags_the_score_down() {
        let all = |bad: f64| {
            combine(vec![
                factor("a", 1.0, 0.5, ""),
                factor("b", bad, 0.5, ""),
            ])
        };
        assert!((all(1.0).score - 1.0).abs() < 1e-12);
        let mixed = all(0.3).score;
        assert!(mixed < 0.55, "geometric mean 0.548, arithmetic would be 0.65: {mixed}");
        assert_eq!(all(0.3).label, "medium");
        assert_eq!(all(0.01).label, "low");
    }
}
