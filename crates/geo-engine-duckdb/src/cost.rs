//! DuckDB cost and timeout-risk model for the semi-join shapes only (`duckdb-semijoin-cost-v*`, risk `duckdb-timeout-risk-v*`).
//!
//! DuckDB keeps the planner's shared, measured server formula for every other shape (median error x1.4 on the calibration corpus). That formula
//! was never fitted on spatial semi joins and was off by x26 (median) on the admin join, which made a fair comparison with PostGIS impossible.
//! This module supplies the missing classes with interpretable per-class formulas (`geo_executor::cost_model`), fitted on the calibration sets, with
//! provenance in the JSON.
//!
//! For the METRIC semi-join classes the model is log-linear with timeouts as censored observations, so one fit answers two different questions:
//! how long will it take (cost), and how likely is it to exceed the execution timeout (risk). They are reported separately: a slow-but-finishing plan is
//! a cost; a plan likely to time out is a risk, and the planner must not treat "the other engine is not validated" as "this engine is safe"
//! (policy `metric-semijoin-policy-v1`, ADR-015).

use geo_executor::cost_model::{evaluate, timeout_verdict, workload_class, ModelFile, ValidationFile};
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
    // DuckDB reads the files itself: no database index to rely on. Its risk is the TIMEOUT risk of the plan (a hazard, not just a cost): the execution timeout
    // interrupts the query, and 11 of ~700 measured metric semi joins hit it.
    let risk = timeout_verdict(model(), class, f).unwrap_or_else(RiskVerdict::low);
    Some(EngineCostEstimate {
        model_version: model().model_version.clone(),
        workload_class: class.into(),
        validation: validation().level(class),
        time_ms,
        terms,
        risk,
        index_state: "available".into(),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_model_parses_and_names_its_risk_spec_when_it_has_one() {
        let m = model();
        assert_eq!(m.engine, "duckdb");
        if let Some(spec) = &m.timeout_risk {
            assert!(spec.version.starts_with("duckdb-timeout-risk-v"), "{}", spec.version);
            assert!(spec.elevated_at < spec.timeout_risk_at && spec.timeout_ms == 60_000.0);
            for class in ["semijoin_metric_small", "semijoin_metric_large"] {
                assert!(m.classes[class].loglinear.is_some(), "{class} needs the log-linear model the risk is derived from");
            }
        }
    }
}
