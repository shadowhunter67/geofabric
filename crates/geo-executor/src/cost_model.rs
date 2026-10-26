//! Shared machinery for ENGINE cost models: workload classes, the model-file format, and the evaluation of an interpretable per-class formula.
//!
//! Two concerns stay apart (RFC-003): *cardinality estimation* (the planner's estimator, producing [`WorkloadFeatures`]) and *engine cost*
//! (an engine's `cost_estimate`, using only those plan-time features). This module is the second concern's plumbing; it knows no SQL, no
//! index and no dataset. Which classes a model has been VALIDATED for is data (the validation file), not code.
//!
//! A workload class is derived from the features, never from a dataset name. The two thresholds below were read off the CALIBRATION set
//! (seed 20261103; benchmarks/results/postgis-cost) and are fixed before the held-out sets were run:
//!   * `CARDINALITY_BROAD`: above this many matching rows a query is a broad scan, whose time is dominated by sequential/parallel table work
//!     rather than index probes (PostGIS lost its constant advantage there: it won 8 of 12 calibration cases, by much less);
//!   * `PROBES_SMALL`: a metric semi join with at most this many right-side probes is the regime in which PostGIS' nested-loop index probing won
//!     16 of 17 calibration cases; beyond it the winner flips with right-side density and distance (DuckDB won 17 of 30).

use crate::{CostTerm, ModelValidation, WorkloadFeatures};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const CARDINALITY_BROAD: u64 = 300_000;
pub const PROBES_SMALL: f64 = 5_000.0;

pub const CLASS_INDEXED_BBOX: &str = "indexed_bbox";
pub const CLASS_INDEXED_SPATIAL: &str = "indexed_spatial";
pub const CLASS_AGGREGATE: &str = "aggregate";
pub const CLASS_BROAD_SCAN: &str = "broad_scan";
pub const CLASS_SEMIJOIN_INTERSECTS: &str = "semijoin_intersects";
pub const CLASS_SEMIJOIN_METRIC_SMALL: &str = "semijoin_metric_small";
pub const CLASS_SEMIJOIN_METRIC_LARGE: &str = "semijoin_metric_large";

/// Rows the engine has to read and filter. An UNORDERED, non-aggregate LIMIT stops at `limit` rows (measured: 16 ms at 900 k matching rows);
/// anything that sorts or aggregates must see every matching row. `limit_certain` is deliberately not consulted: whether the limit is
/// reached does not change that the scan can stop there.
pub fn scan_rows(f: &WorkloadFeatures) -> f64 {
    match f.limit {
        Some(n) if !f.has_sort && !f.has_aggregate && f.semi_join.is_none() => (f.cardinality as f64).min(n as f64),
        _ => f.cardinality as f64,
    }
}

/// The workload class of a plan, from its plan-time features only.
pub fn workload_class(f: &WorkloadFeatures) -> &'static str {
    if let Some(sj) = &f.semi_join {
        return if sj.predicate == "intersects" {
            CLASS_SEMIJOIN_INTERSECTS
        } else if sj.expected_probes <= PROBES_SMALL {
            CLASS_SEMIJOIN_METRIC_SMALL
        } else {
            CLASS_SEMIJOIN_METRIC_LARGE
        };
    }
    if scan_rows(f) > CARDINALITY_BROAD as f64 {
        CLASS_BROAD_SCAN
    } else if f.has_aggregate {
        CLASS_AGGREGATE
    } else if f.has_intersects || f.has_transform {
        CLASS_INDEXED_SPATIAL
    } else {
        CLASS_INDEXED_BBOX
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provenance {
    pub benchmark_commit: String,
    pub calibration_seed: u64,
    #[serde(default)]
    pub calibration_seeds: Vec<u64>,
    pub calibration_cases: usize,
    pub dataset_versions: String,
    pub machine_class: String,
    pub build_profile: String,
    pub fitted_at: String,
    pub method: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClassModel {
    /// term name -> coefficient (see [`evaluate`] for the unit of each term)
    pub terms: BTreeMap<String, f64>,
    pub n_calibration: usize,
    pub fit_ratio_median: f64,
    pub fit_ratio_p90: f64,
    /// A log-linear time model (metric semi joins). When present it REPLACES `terms` for the time and also yields a timeout probability.
    #[serde(default)]
    pub loglinear: Option<LogLinear>,
}

/// `ln(time_ms) ~ Normal(intercept + sum(coef_i * x_i), sigma)` with `x_i` the named plan-time features (natural logs). Fitted on measured times with
/// timeouts as CENSORED observations (benchmarks/metric_semijoin_fit.py), so the same model gives the expected time and the probability of exceeding the
/// execution timeout. A feature the planner could not estimate makes the answer UNKNOWN (never a silent zero); a plan outside the measured envelope too.
#[derive(Debug, Clone, Deserialize)]
pub struct LogLinear {
    pub features: Vec<String>,
    pub intercept: f64,
    pub coefficients: Vec<f64>,
    pub sigma: f64,
    /// Measured [min, max] of each feature (in its own log units) over the calibration data.
    pub envelope: BTreeMap<String, [f64; 2]>,
    /// A feature this far (in ln units) outside its envelope makes the prediction `unknown`.
    pub extrapolation_ln: f64,
}

/// A feature of the plan by name, in natural log; `None` = the planner could not estimate it.
pub fn feature_ln(name: &str, f: &WorkloadFeatures) -> Option<f64> {
    let sj = f.semi_join.as_ref()?;
    let ln = |v: f64| v.max(1e-9).ln();
    match name {
        "ln_probes" => Some(ln(sj.expected_probes)),
        "ln_right_rows" => Some(ln(sj.right_rows)),
        "ln_right_rows_filtered" => sj.right_rows_filtered.map(ln),
        "ln_distance_m" => sj.distance_m.map(ln),
        "ln_candidate_pairs" => sj.candidate_pairs.map(ln),
        "ln_candidates_per_probe" => sj.candidates_per_probe.map(ln),
        "ln_right_density" => sj.right_density_per_km2.map(ln),
        "ln_overlap_ratio" => sj.overlap_ratio.map(ln),
        "ln_right_window_km2" => sj.right_window_km2.map(ln),
        "ln_left_window_km2" => sj.left_window_km2.map(ln),
        _ => None,
    }
}

/// Standard normal CDF (Abramowitz & Stegun 7.1.26 for erf; |error| < 1.5e-7). The Python fit uses math.erf: a parity test pins the two.
pub fn normal_cdf(z: f64) -> f64 {
    let x = z / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let poly = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    let erf = 1.0 - poly * (-x * x).exp();
    0.5 * (1.0 + if x >= 0.0 { erf } else { -erf })
}

/// What a log-linear model says about one plan.
#[derive(Debug, Clone)]
pub enum LogLinearAnswer {
    /// expected time (ms) = exp(mu), and P(time > timeout_ms)
    Known { mu: f64, time_ms: f64, timeout_probability: f64 },
    /// not answerable, and why (missing feature / outside the measured envelope)
    Unknown(String),
}

impl LogLinear {
    pub fn answer(&self, f: &WorkloadFeatures, timeout_ms: f64) -> LogLinearAnswer {
        let mut mu = self.intercept;
        for (name, c) in self.features.iter().zip(&self.coefficients) {
            let Some(x) = feature_ln(name, f) else {
                return LogLinearAnswer::Unknown(format!("feature '{name}' could not be estimated at planning time"));
            };
            if let Some([lo, hi]) = self.envelope.get(name) {
                if x < lo - self.extrapolation_ln || x > hi + self.extrapolation_ln {
                    return LogLinearAnswer::Unknown(format!("feature '{name}' (e^{x:.1} = {:.3e}) is outside the measured range [{:.3e}, {:.3e}]", x.exp(), lo.exp(), hi.exp()));
                }
            }
            mu += c * x;
        }
        let p = 1.0 - normal_cdf((timeout_ms.ln() - mu) / self.sigma.max(1e-6));
        LogLinearAnswer::Known { mu, time_ms: mu.exp(), timeout_probability: p }
    }
}

/// Probability thresholds that turn a timeout probability into the risk states of the planner policy.
#[derive(Debug, Clone, Deserialize)]
pub struct TimeoutRiskSpec {
    /// e.g. `duckdb-timeout-risk-v1`: versioned apart from the cost model and from the planner policy
    pub version: String,
    pub timeout_ms: f64,
    pub elevated_at: f64,
    pub timeout_risk_at: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub model_version: String,
    pub engine: String,
    pub provenance: Provenance,
    pub classes: BTreeMap<String, ClassModel>,
    /// Present when the engine's model can express a timeout probability (needs `loglinear` classes).
    #[serde(default)]
    pub timeout_risk: Option<TimeoutRiskSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClassValidation {
    pub level: ModelValidation,
    pub heldout_cases: usize,
    pub p90_regret: Option<f64>,
    pub p90_prediction_ratio: Option<f64>,
    pub median_prediction_ratio: Option<f64>,
    pub unsafe_selections: usize,
    pub reason: String,
}

/// Which classes the held-out evaluation validated (benchmarks/postgis_cost_eval.py writes it; the gates are benchmarks/postgis_cost_gates.json).
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationFile {
    pub model_version: String,
    pub heldout_seed: u64,
    pub classes: BTreeMap<String, ClassValidation>,
}

impl ValidationFile {
    pub fn level(&self, class: &str) -> ModelValidation {
        self.classes.get(class).map_or(ModelValidation::Unvalidated, |c| c.level)
    }
}

/// The timeout risk of `class` for this plan, from the class' log-linear model and the file's thresholds. `None` when the engine has no such model for
/// the class (then the engine states no timeout opinion at all - that is different from `unknown`, which means "I would, but I cannot for this plan").
pub fn timeout_verdict(model: &ModelFile, class: &str, f: &WorkloadFeatures) -> Option<crate::RiskVerdict> {
    let spec = model.timeout_risk.as_ref()?;
    let ll = model.classes.get(class)?.loglinear.as_ref()?;
    let mut v = match ll.answer(f, spec.timeout_ms) {
        LogLinearAnswer::Known { time_ms, timeout_probability: p, .. } => {
            let level = if p >= spec.timeout_risk_at {
                "timeout_risk"
            } else if p >= spec.elevated_at {
                "elevated"
            } else {
                "low"
            };
            let mut v = crate::RiskVerdict::new(
                level,
                format!("{}: expected ~{:.1} s, P(exceeding the {:.0} s timeout) ~ {:.0} %", spec.version, time_ms / 1000.0, spec.timeout_ms / 1000.0, p * 100.0),
            );
            v.timeout_probability = Some(p);
            v
        }
        LogLinearAnswer::Unknown(why) => crate::RiskVerdict::new("unknown", format!("{}: {why}; unknown is not low", spec.version)),
    };
    v.blocked = false;
    Some(v)
}

/// `time_ms = sum(coefficient * term)` over the class' terms:
/// * `startup`          1                                          (ms: one statement - connection checkout, planning, dispatch)
/// * `scan`             matching rows / 1000                        (ms per 1000 rows read and filtered)
/// * `result`           returned rows / 1000                        (ms per 1000 rows materialised)
/// * `result_geometry`  returned geometry MB                        (ms per MB serialised)
/// * `probe`            semi-join right-side probes / 1000          (ms per 1000 probes)
/// * `right`            semi-join right-side rows / 1000            (ms per 1000 rows prepared)
/// Returns the total (floored at 1 ms) and the non-zero terms in ms, in that order.
pub fn evaluate(model: &ClassModel, f: &WorkloadFeatures) -> (f64, Vec<CostTerm>) {
    if let Some(ll) = &model.loglinear {
        // a log-linear class: the time is the model's expected time; an unanswerable plan falls back to the plan's own additive terms below
        if let LogLinearAnswer::Known { time_ms, .. } = ll.answer(f, 60_000.0) {
            return (time_ms.max(1.0), vec![CostTerm { name: "loglinear".into(), ms: time_ms }]);
        }
    }
    let (probes, right) = f.semi_join.as_ref().map_or((0.0, 0.0), |s| (s.expected_probes, s.right_rows));
    let value = |name: &str| match name {
        "startup" => 1.0,
        "scan" => scan_rows(f) / 1000.0,
        "result" => f.result_rows as f64 / 1000.0,
        "result_geometry" => f.result_geom_bytes / 1_048_576.0,
        "probe" => probes / 1000.0,
        "right" => right / 1000.0,
        _ => 0.0,
    };
    let mut terms = Vec::new();
    let mut total = 0.0;
    for (name, coef) in &model.terms {
        let ms = coef * value(name);
        if ms != 0.0 {
            terms.push(CostTerm { name: name.clone(), ms });
        }
        total += ms;
    }
    (total.max(1.0), terms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SemiJoinFeatures;

    fn base() -> WorkloadFeatures {
        WorkloadFeatures { shape: "features".into(), ..Default::default() }
    }

    fn semi(predicate: &str, probes: f64) -> WorkloadFeatures {
        WorkloadFeatures {
            semi_join: Some(SemiJoinFeatures {
                left_uri: "l".into(),
                right_uri: "r".into(),
                left_rows: probes,
                right_rows: 100.0,
                left_total_rows: 1,
                right_total_rows: 1,
                metric: predicate != "intersects",
                predicate: predicate.into(),
                distance_m: Some(200.0),
                expected_probes: probes,
                ..Default::default()
            }),
            ..base()
        }
    }

    #[test]
    fn classes_come_from_features_not_names() {
        assert_eq!(workload_class(&base()), CLASS_INDEXED_BBOX);
        assert_eq!(workload_class(&WorkloadFeatures { has_aggregate: true, cardinality: 10, ..base() }), CLASS_AGGREGATE);
        assert_eq!(workload_class(&WorkloadFeatures { has_transform: true, ..base() }), CLASS_INDEXED_SPATIAL);
        assert_eq!(workload_class(&WorkloadFeatures { has_intersects: true, ..base() }), CLASS_INDEXED_SPATIAL);
        assert_eq!(workload_class(&WorkloadFeatures { cardinality: CARDINALITY_BROAD + 1, has_aggregate: true, ..base() }), CLASS_BROAD_SCAN);
        // an unordered LIMIT stops early; a sorted one cannot
        let big = WorkloadFeatures { cardinality: 5_000_000, limit: Some(2000), ..base() };
        assert_eq!(workload_class(&big), CLASS_INDEXED_BBOX);
        assert_eq!(workload_class(&WorkloadFeatures { has_sort: true, ..big }), CLASS_BROAD_SCAN);
        assert_eq!(workload_class(&semi("intersects", 1e9)), CLASS_SEMIJOIN_INTERSECTS);
        assert_eq!(workload_class(&semi("within_distance", PROBES_SMALL)), CLASS_SEMIJOIN_METRIC_SMALL);
        assert_eq!(workload_class(&semi("within_distance", PROBES_SMALL + 1.0)), CLASS_SEMIJOIN_METRIC_LARGE);
    }

    #[test]
    fn evaluation_is_the_documented_sum() {
        let m = ClassModel { terms: [("startup".to_string(), 10.0), ("scan".to_string(), 2.0), ("probe".to_string(), 3.0)].into(), n_calibration: 1, fit_ratio_median: 1.0, fit_ratio_p90: 1.0, loglinear: None };
        let mut f = semi("within_distance", 2000.0);
        f.cardinality = 5000;
        let (t, terms) = evaluate(&m, &f);
        assert!((t - (10.0 + 2.0 * 5.0 + 3.0 * 2.0)).abs() < 1e-9);
        assert_eq!(terms.len(), 3);
        // never below the timer floor
        assert_eq!(evaluate(&ClassModel { terms: Default::default(), ..m }, &base()).0, 1.0);
    }

    fn ll() -> LogLinear {
        LogLinear {
            features: vec!["ln_probes".into(), "ln_candidate_pairs".into()],
            intercept: 1.0,
            coefficients: vec![0.5, 0.3],
            sigma: 0.8,
            envelope: [("ln_probes".to_string(), [5.0, 13.0]), ("ln_candidate_pairs".to_string(), [5.0, 17.0])].into(),
            extrapolation_ln: 1.0,
        }
    }

    fn with_pairs(probes: f64, pairs: Option<f64>) -> WorkloadFeatures {
        let mut f = semi("within_distance", probes);
        f.semi_join.as_mut().unwrap().candidate_pairs = pairs;
        f
    }

    #[test]
    fn the_normal_cdf_matches_known_values() {
        for (z, want) in [(0.0, 0.5), (1.0, 0.841344746), (-1.0, 0.158655254), (1.96, 0.975002105), (-3.0, 0.001349898), (4.0, 0.999968329)] {
            assert!((normal_cdf(z) - want).abs() < 2e-7, "z={z}: {} vs {want}", normal_cdf(z));
        }
    }

    #[test]
    fn a_loglinear_model_gives_time_and_timeout_probability_and_is_unknown_when_it_cannot_know() {
        let m = ll();
        // ln(probes)=ln(20000)=9.9, ln(pairs)=ln(1e6)=13.8 -> mu = 1 + 0.5*9.9 + 0.3*13.8 = 10.09 -> ~24 s
        match m.answer(&with_pairs(20_000.0, Some(1e6)), 60_000.0) {
            LogLinearAnswer::Known { time_ms, timeout_probability, .. } => {
                assert!((time_ms / 1000.0 - 24.0).abs() < 2.0, "{time_ms}");
                assert!(timeout_probability > 0.05 && timeout_probability < 0.3, "{timeout_probability}");
            }
            other => panic!("{other:?}"),
        }
        // a feature the planner could not estimate is UNKNOWN, never zero
        assert!(matches!(m.answer(&with_pairs(20_000.0, None), 60_000.0), LogLinearAnswer::Unknown(r) if r.contains("ln_candidate_pairs")));
        // far outside the measured envelope is UNKNOWN too
        assert!(matches!(m.answer(&with_pairs(1e9, Some(1e6)), 60_000.0), LogLinearAnswer::Unknown(r) if r.contains("outside the measured range")));
    }

    #[test]
    fn unknown_class_is_unvalidated() {
        let v = ValidationFile { model_version: "x".into(), heldout_seed: 1, classes: Default::default() };
        assert_eq!(v.level("indexed_bbox"), ModelValidation::Unvalidated);
    }
}
