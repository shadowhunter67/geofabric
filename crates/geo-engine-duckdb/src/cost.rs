//! DuckDB cost model for the semi-join shapes only (`duckdb-semijoin-cost-v1`).
//!
//! DuckDB keeps the planner's shared, measured server formula for every other shape (median error x1.4 on the calibration corpus). That formula
//! was never fitted on spatial semi joins and was off by x26 (median) on the admin join, which made a fair comparison with PostGIS impossible: the
//! planner believed DuckDB would answer in ~150 ms what it answers in 4-60 s. This module supplies the missing classes with the same
//! interpretable per-class formulas (`geo_executor::cost_model`), fitted on the same calibration set, with provenance in the JSON.

use geo_executor::cost_model::{evaluate, workload_class, ModelFile, ValidationFile};
use geo_executor::{EngineCostEstimate, RiskVerdict, WorkloadFeatures};
use std::sync::OnceLock;

fn model() -> &'static ModelFile {
    static M: OnceLock<ModelFile> = OnceLock::new();
    M.get_or_init(|| serde_json::from_str(include_str!("cost_model_v1.json")).expect("duckdb cost_model_v1.json"))
}

fn validation() -> &'static ValidationFile {
    static V: OnceLock<ValidationFile> = OnceLock::new();
    V.get_or_init(|| serde_json::from_str(include_str!("cost_validation_v1.json")).expect("duckdb cost_validation_v1.json"))
}

pub fn estimate(f: &WorkloadFeatures) -> Option<EngineCostEstimate> {
    let class = workload_class(f);
    let cm = model().classes.get(class)?;
    let (time_ms, terms) = evaluate(cm, f);
    let mut notes = vec![format!("fitted on {} calibration cases (median error x{:.2}, P90 x{:.2}); the engine's shared formula covers all other shapes", cm.n_calibration, cm.fit_ratio_median, cm.fit_ratio_p90)];
    if let Some(v) = validation().classes.get(class) {
        notes.push(v.reason.clone());
    }
    Some(EngineCostEstimate {
        model_version: model().model_version.clone(),
        workload_class: class.into(),
        validation: validation().level(class),
        time_ms,
        terms,
        // DuckDB reads the files itself: no database index to rely on, and a slow semi join is a cost, not a hazard (it is interrupted at the timeout)
        risk: RiskVerdict { blocked: false, level: "low".into(), reason: String::new() },
        index_state: "available".into(),
        notes,
    })
}
