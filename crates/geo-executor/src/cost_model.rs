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
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub model_version: String,
    pub engine: String,
    pub provenance: Provenance,
    pub classes: BTreeMap<String, ClassModel>,
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

/// `time_ms = sum(coefficient * term)` over the class' terms:
/// * `startup`          1                                          (ms: one statement - connection checkout, planning, dispatch)
/// * `scan`             matching rows / 1000                        (ms per 1000 rows read and filtered)
/// * `result`           returned rows / 1000                        (ms per 1000 rows materialised)
/// * `result_geometry`  returned geometry MB                        (ms per MB serialised)
/// * `probe`            semi-join right-side probes / 1000          (ms per 1000 probes)
/// * `right`            semi-join right-side rows / 1000            (ms per 1000 rows prepared)
/// Returns the total (floored at 1 ms) and the non-zero terms in ms, in that order.
pub fn evaluate(model: &ClassModel, f: &WorkloadFeatures) -> (f64, Vec<CostTerm>) {
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
        let m = ClassModel { terms: [("startup".to_string(), 10.0), ("scan".to_string(), 2.0), ("probe".to_string(), 3.0)].into(), n_calibration: 1, fit_ratio_median: 1.0, fit_ratio_p90: 1.0 };
        let mut f = semi("within_distance", 2000.0);
        f.cardinality = 5000;
        let (t, terms) = evaluate(&m, &f);
        assert!((t - (10.0 + 2.0 * 5.0 + 3.0 * 2.0)).abs() < 1e-9);
        assert_eq!(terms.len(), 3);
        // never below the timer floor
        assert_eq!(evaluate(&ClassModel { terms: Default::default(), ..m }, &base()).0, 1.0);
    }

    #[test]
    fn unknown_class_is_unvalidated() {
        let v = ValidationFile { model_version: "x".into(), heldout_seed: 1, classes: Default::default() };
        assert_eq!(v.level("indexed_bbox"), ModelValidation::Unvalidated);
    }
}
