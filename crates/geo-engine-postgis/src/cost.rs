//! PostGIS cost model v1 (`postgis-cost-v1`): transparent per-workload-class formulas, a hard semi-join risk policy, and catalog-derived index state.
//!
//! What lives where (RFC-003 / ADR-015):
//! * cardinality estimation   - the planner (`WorkloadFeatures`), shared by every engine, unchanged by this model;
//! * the formulas             - `cost_model_v1.json`: coefficients per class with provenance, produced by `benchmarks/postgis_cost_fit.py` from the
//!                              CALIBRATION set only (seed 20261103);
//! * how far to trust them    - `cost_validation_v1.json`: per class `validated | limited | unvalidated`, produced by the held-out evaluation against the
//!                              gates in `benchmarks/postgis_cost_gates.json`. A class that is not `validated` is explained but never chosen on predicted
//!                              cost over a measured engine (R5d); an unknown index state is never treated as available;
//! * the HARD risk policy     - below, separate from the cost: a blocked workload is refused for automatic selection, not "expensive".
//!
//! The model owes nothing to dataset names: classes come from plan-time features (`geo_executor::cost_model::workload_class`) and the index state
//! from the catalog the operator loaded (`spatially_indexed`, `metric_indexes`).

use crate::catalog::PostGisCatalog;
use geo_executor::cost_model::{evaluate, workload_class, ModelFile, ValidationFile, CLASS_SEMIJOIN_METRIC_LARGE, CLASS_SEMIJOIN_METRIC_SMALL, PROBES_SMALL};
use geo_executor::{EngineCostEstimate, ModelValidation, RiskVerdict, WorkloadFeatures};
use geo_ir::{Plan, PlanNode};
use geo_types::Crs;
use std::sync::OnceLock;

/// Hard policy: a metric semi join with more right-side probes than this is refused for AUTOMATIC selection (an explicit engine request still runs).
/// Calibration: above ~5 k probes the winner flips with right-side density and distance; above 50 k PostGIS took 2-35 s in most measured cases where
/// DuckDB took 0.3-20 s, and the largest known case (189,772 ids) took 12-17 s against 1.0-1.9 s. No validated efficient strategy exists for it
/// (a two-stage bbox prefilter or a generated projected column were not part of this phase).
pub const BLOCK_PROBES: f64 = 50_000.0;

fn model() -> &'static ModelFile {
    static M: OnceLock<ModelFile> = OnceLock::new();
    M.get_or_init(|| serde_json::from_str(include_str!("cost_model_v1.json")).expect("cost_model_v1.json"))
}

fn validation() -> &'static ValidationFile {
    static V: OnceLock<ValidationFile> = OnceLock::new();
    V.get_or_init(|| serde_json::from_str(include_str!("cost_validation_v1.json")).expect("cost_validation_v1.json"))
}

/// `available | unavailable | unknown`: can the access path the model assumed be relied on? Derived from the catalog only (no EXPLAIN at
/// planning time). `unknown` is never treated as `available`.
pub fn index_state(catalog: &PostGisCatalog, plan: &Plan, f: &WorkloadFeatures) -> &'static str {
    let mut sources = Vec::new();
    for n in plan.nodes.values() {
        if let PlanNode::Source { uri, .. } = n {
            match catalog.get(uri) {
                Some(s) => sources.push(s),
                None => return "unknown",
            }
        }
    }
    if sources.is_empty() {
        return "unknown";
    }
    if sources.iter().any(|s| !s.spatially_indexed) {
        return "unavailable";
    }
    if f.semi_join.as_ref().is_some_and(|s| s.metric) {
        let targets: Vec<i32> = plan
            .nodes
            .values()
            .filter_map(|n| match n {
                PlanNode::Transform { output_crs: Crs::Epsg(c), .. } => Some(*c as i32),
                _ => None,
            })
            .collect();
        if targets.is_empty() || sources.iter().any(|s| targets.iter().any(|t| !s.metric_indexes.contains(t))) {
            return "unavailable";
        }
    }
    "available"
}

fn risk(class: &str, f: &WorkloadFeatures, index: &str) -> RiskVerdict {
    let probes = f.semi_join.as_ref().map_or(0.0, |s| s.expected_probes);
    if (class == CLASS_SEMIJOIN_METRIC_SMALL || class == CLASS_SEMIJOIN_METRIC_LARGE) && probes > BLOCK_PROBES {
        return RiskVerdict {
            blocked: true,
            level: "high".into(),
            reason: format!(
                "repeated-probe risk: a metric semi join over ~{probes:.0} left rows makes that many right-side index probes (limit {BLOCK_PROBES:.0}); measured PostGIS times at this scale were seconds, often several times DuckDB's."
            ),
        };
    }
    if class == CLASS_SEMIJOIN_METRIC_LARGE || (class == CLASS_SEMIJOIN_METRIC_SMALL && probes > PROBES_SMALL) {
        return RiskVerdict { blocked: false, level: "elevated".into(), reason: format!("~{probes:.0} right-side probes: the faster engine depends on right-side density and distance") };
    }
    if index != "available" {
        return RiskVerdict { blocked: false, level: "elevated".into(), reason: format!("index state '{index}': the indexed access path is not guaranteed") };
    }
    RiskVerdict { blocked: false, level: "low".into(), reason: String::new() }
}

/// The model's verdict for `plan`; `None` when the class has no formula (the planner then uses its shared server model).
pub fn estimate(catalog: &PostGisCatalog, plan: &Plan, f: &WorkloadFeatures) -> Option<EngineCostEstimate> {
    let class = workload_class(f);
    let cm = model().classes.get(class)?;
    let (time_ms, terms) = evaluate(cm, f);
    let index = index_state(catalog, plan, f);
    // an index state that is not `available` can never validate a class: the formula was fitted on indexed access paths
    let level = if index == "available" { validation().level(class) } else { ModelValidation::Unvalidated };
    let mut notes = vec![format!("fitted on {} calibration cases (median error x{:.2}, P90 x{:.2})", cm.n_calibration, cm.fit_ratio_median, cm.fit_ratio_p90)];
    if let Some(v) = validation().classes.get(class) {
        notes.push(v.reason.clone());
    }
    Some(EngineCostEstimate { model_version: model().model_version.clone(), workload_class: class.into(), validation: level, time_ms, terms, risk: risk(class, f, index), index_state: index.into(), notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo_executor::SemiJoinFeatures;

    fn metric(probes: f64) -> WorkloadFeatures {
        WorkloadFeatures {
            semi_join: Some(SemiJoinFeatures {
                left_uri: "l".into(),
                right_uri: "r".into(),
                left_rows: probes,
                right_rows: 1000.0,
                left_total_rows: 1,
                right_total_rows: 1,
                metric: true,
                predicate: "within_distance".into(),
                distance_m: Some(200.0),
                expected_probes: probes,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_repeated_probe_policy_is_a_hard_refusal_separate_from_the_cost() {
        let f = metric(BLOCK_PROBES + 1.0);
        let r = risk(workload_class(&f), &f, "available");
        assert!(r.blocked && r.level == "high" && r.reason.contains("repeated-probe"), "{r:?}");
        let f = metric(PROBES_SMALL + 1.0);
        let r = risk(workload_class(&f), &f, "available");
        assert!(!r.blocked && r.level == "elevated", "{r:?}");
        let f = metric(PROBES_SMALL);
        let r = risk(workload_class(&f), &f, "available");
        assert!(!r.blocked && r.level == "low", "{r:?}");
    }

    #[test]
    fn an_index_state_that_is_not_available_raises_the_risk_and_is_never_read_as_available() {
        let f = WorkloadFeatures::default();
        for state in ["unavailable", "unknown"] {
            let r = risk(workload_class(&f), &f, state);
            assert!(!r.blocked && r.level == "elevated" && r.reason.contains(state), "{state}: {r:?}");
        }
        assert_eq!(risk(workload_class(&f), &f, "available").level, "low");
    }

    #[test]
    fn the_shipped_model_covers_every_class_and_names_its_version() {
        let m = model();
        assert!(m.model_version.starts_with("postgis-cost-v1"));
        for c in ["indexed_bbox", "indexed_spatial", "aggregate", "broad_scan", "semijoin_intersects", "semijoin_metric_small", "semijoin_metric_large"] {
            assert!(m.classes.contains_key(c), "class {c} has no formula");
        }
    }
}
