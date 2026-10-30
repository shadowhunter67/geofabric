//! The workload classes and the model formulas are defined twice on purpose: in Python (benchmarks/postgis_cost_fit.py, where the coefficients are
//! fitted) and in Rust (geo_executor::cost_model, where the planner uses them). These tests pin the two together on the committed evidence, so a change to
//! a threshold or a term on one side fails here instead of silently shifting every prediction.

use geo_executor::cost_model::{evaluate, workload_class, ModelFile};
use geo_executor::{SemiJoinFeatures, WorkloadFeatures};
use serde_json::Value;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))).unwrap()
}

fn semi(sj: &Value) -> Option<SemiJoinFeatures> {
    if sj.is_null() {
        return None;
    }
    Some(SemiJoinFeatures {
        left_uri: String::new(),
        right_uri: String::new(),
        left_rows: sj["left_rows"].as_f64().unwrap_or(0.0),
        right_rows: sj["right_rows"].as_f64().unwrap_or(0.0),
        left_total_rows: 0,
        right_total_rows: 0,
        metric: sj["metric"].as_bool().unwrap_or(true),
        predicate: sj["predicate"].as_str().unwrap_or("within_distance").to_string(),
        distance_m: sj["distance_m"].as_f64(),
        expected_probes: sj["expected_probes"].as_f64().unwrap(),
        ..Default::default()
    })
}

#[test]
fn rust_and_python_assign_the_same_class_to_every_calibration_case() {
    let doc = read("benchmarks/results/postgis-cost/calibration-cases-r2.json");
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 240);
    for c in cases {
        let f = &c["features"];
        let features = WorkloadFeatures {
            cardinality: f["cardinality"].as_u64().unwrap_or(0),
            has_aggregate: f["has_aggregate"].as_bool().unwrap(),
            has_intersects: f["has_intersects"].as_bool().unwrap(),
            has_transform: f["has_transform"].as_bool().unwrap(),
            has_sort: f["has_sort"].as_bool().unwrap(),
            limit: f["limit"].as_u64(),
            semi_join: semi(&c["semi_join"]),
            ..Default::default()
        };
        assert_eq!(workload_class(&features), c["workload_class"].as_str().unwrap(), "case {}", c["id"]);
    }
}

#[test]
fn the_rust_evaluation_reproduces_the_python_prediction_of_the_shipped_model() {
    let model: ModelFile = serde_json::from_value(read("crates/geo-engine-postgis/src/cost_model_v1.json")).unwrap();
    assert!(model.model_version.starts_with("postgis-cost-v"), "{}", model.model_version);
    let fx = read("benchmarks/results/postgis-cost/model-fixtures.json");
    let rows = fx["rows"].as_array().unwrap();
    assert!(rows.len() >= 40);
    for r in rows {
        let features = WorkloadFeatures {
            cardinality: r["cardinality"].as_u64().unwrap_or(0),
            result_rows: r["result_rows"].as_u64().unwrap_or(0),
            result_geom_bytes: r["result_geom_bytes"].as_f64().unwrap_or(0.0),
            has_sort: r["has_sort"].as_bool().unwrap(),
            has_aggregate: r["has_aggregate"].as_bool().unwrap(),
            has_intersects: r["has_intersects"].as_bool().unwrap(),
            has_transform: r["has_transform"].as_bool().unwrap(),
            limit: r["limit"].as_u64(),
            semi_join: semi(&r["semi_join"]),
            ..Default::default()
        };
        let class = workload_class(&features);
        assert_eq!(class, r["class"].as_str().unwrap());
        if model.classes[class].loglinear.is_some() {
            continue; // the metric classes are pinned by the log-linear parity test below (these fixtures are the additive v2 classes, unchanged since)
        }
        let (ms, _) = evaluate(&model.classes[class], &features);
        let want = r["expected_ms"].as_f64().unwrap();
        assert!((ms - want).abs() <= 1e-6 * want.max(1.0), "class {class}: rust {ms} vs python {want}");
    }
}

#[test]
fn every_shipped_class_has_provenance_and_a_validation_entry_or_is_unvalidated_by_default() {
    for (engine, file) in [("postgis", "crates/geo-engine-postgis/src/cost_model_v1.json"), ("duckdb", "crates/geo-engine-duckdb/src/cost_model_v1.json")] {
        let m: ModelFile = serde_json::from_value(read(file)).unwrap();
        assert_eq!(m.engine, engine);
        assert!(!m.provenance.benchmark_commit.is_empty() && m.provenance.calibration_cases > 0 && !m.provenance.machine_class.is_empty(), "{engine}: provenance");
        for (class, cm) in &m.classes {
            assert!(cm.n_calibration >= 6, "{engine}/{class}: fitted on {} cases", cm.n_calibration);
            assert!(cm.terms.values().all(|v| *v >= 0.0), "{engine}/{class}: coefficients are non-negative");
        }
    }
}

fn features_of(v: &Value) -> WorkloadFeatures {
    let n = |k: &str| v[k].as_f64();
    WorkloadFeatures {
        semi_join: Some(SemiJoinFeatures {
            expected_probes: n("expected_probes").unwrap(),
            right_rows: n("right_rows").unwrap_or(0.0),
            right_rows_filtered: n("right_rows_filtered"),
            distance_m: n("distance_m"),
            candidate_pairs: n("candidate_pairs"),
            candidates_per_probe: n("candidates_per_probe"),
            right_density_per_km2: n("right_density_per_km2"),
            overlap_ratio: n("overlap_ratio"),
            right_window_km2: n("right_window_km2"),
            left_window_km2: n("left_window_km2"),
            metric: v["metric"].as_bool().unwrap_or(true),
            predicate: v["predicate"].as_str().unwrap_or("within_distance").to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn the_metric_models_answer_in_rust_exactly_what_the_python_fit_says() {
    use geo_executor::cost_model::{timeout_verdict, LogLinearAnswer};
    let fx = read("benchmarks/results/postgis-cost/metric-fixtures.json");
    let models: Vec<(&str, ModelFile)> =
        [("postgis", "crates/geo-engine-postgis/src/cost_model_v1.json"), ("duckdb", "crates/geo-engine-duckdb/src/cost_model_v1.json")].map(|(e, f)| (e, serde_json::from_value(read(f)).unwrap())).to_vec();
    for (e, m) in &models {
        assert_eq!(fx["models"][e].as_str().unwrap(), m.model_version, "the fixtures were made for another model version: run benchmarks/make_metric_fixtures.py");
    }
    let rows = fx["rows"].as_array().unwrap();
    assert!(rows.len() >= 30);
    let mut levels = std::collections::BTreeSet::new();
    for r in rows {
        let f = features_of(&r["features"]);
        let class = workload_class(&f);
        assert_eq!(class, r["class"].as_str().unwrap(), "{r}");
        for (e, m) in &models {
            let want = &r[*e];
            let ll = m.classes[class].loglinear.as_ref().expect("metric class has a log-linear model");
            match (ll.answer(&f, 60_000.0), want["time_ms"].as_f64()) {
                (LogLinearAnswer::Known { time_ms, .. }, Some(w)) => assert!((time_ms - w).abs() <= 1e-6 * w.max(1.0), "{e} {class}: rust {time_ms} vs python {w}"),
                (LogLinearAnswer::Unknown(_), None) => {}
                (a, w) => panic!("{e}: rust {a:?} vs python {w:?}"),
            }
            let v = timeout_verdict(m, class, &f).expect("a timeout verdict for a metric class");
            assert_eq!(v.level, want["risk"]["level"].as_str().unwrap(), "{e} {class} {r}");
            match (v.timeout_probability, want["risk"]["p"].as_f64()) {
                (Some(p), Some(w)) => assert!((p - w).abs() < 2e-6, "{e}: P(timeout) rust {p} vs python {w}"),
                (None, None) => {}
                (a, b) => panic!("{e}: probability {a:?} vs {b:?}"),
            }
            levels.insert(format!("{e}:{}", v.level));
        }
    }
    assert!(levels.contains("duckdb:timeout_risk") && levels.contains("duckdb:low"), "the sample must exercise both ends: {levels:?}");
}

/// The ten DuckDB executions that hit the 60 s timeout among the non-confirmation metric semi joins (including the two held-out cases on which the previous
/// planner chose DuckDB, DuckDB timed out and PostGIS finished in 12-18 s). They are FIXED regression cases: the shipped models must keep calling each of
/// them a DuckDB timeout risk, and must never call PostGIS a timeout risk for them (it completed all ten in 4.6-18 s).
#[test]
fn every_observed_duckdb_timeout_is_rated_timeout_risk_and_postgis_is_not() {
    use geo_executor::cost_model::timeout_verdict;
    let fx = read("benchmarks/results/postgis-cost/timeout-regression-cases.json");
    let load = |f: &str| -> ModelFile { serde_json::from_value(read(f)).unwrap() };
    let (duck, pg) = (load("crates/geo-engine-duckdb/src/cost_model_v1.json"), load("crates/geo-engine-postgis/src/cost_model_v1.json"));
    let rows = fx["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 10);
    for r in rows {
        let f = features_of(&r["sj"]);
        let class = workload_class(&f);
        let d = timeout_verdict(&duck, class, &f).unwrap();
        assert_eq!(d.level, "timeout_risk", "{} ({class}): {d:?}", r["id"]);
        assert!(d.timeout_probability.unwrap() >= duck.timeout_risk.as_ref().unwrap().timeout_risk_at);
        let p = timeout_verdict(&pg, class, &f).unwrap();
        assert_ne!(p.level, "timeout_risk", "{}: PostGIS completed this case in {} ms: {p:?}", r["id"], r["postgis_actual_ms"]);
    }
}

/// Eleven cases the 0.20 threshold of `duckdb-timeout-risk-v1` missed (DuckDB timeout probability 0.10-0.20, DuckDB hit/passed the 60 s timeout, PostGIS finished in
/// 2.6-22 s). They were found by the threshold confirmation (frozen candidates 0.10/0.15/0.20, fresh seed) and fix `duckdb-timeout-risk-v2` at 0.10: each must be a
/// DuckDB timeout risk, PostGIS must never be one.
#[test]
fn duckdb_timeouts_missed_by_the_old_threshold_are_timeout_risk_in_v2() {
    use geo_executor::cost_model::timeout_verdict;
    let fx = read("benchmarks/results/postgis-cost/timeout-regression-cases-r2.json");
    let load = |f: &str| -> ModelFile { serde_json::from_value(read(f)).unwrap() };
    let (duck, pg) = (load("crates/geo-engine-duckdb/src/cost_model_v1.json"), load("crates/geo-engine-postgis/src/cost_model_v1.json"));
    assert_eq!(duck.timeout_risk.as_ref().unwrap().version, "duckdb-timeout-risk-v2");
    let rows = fx["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 11);
    for r in rows {
        let f = features_of(&r["sj"]);
        let class = workload_class(&f);
        let d = timeout_verdict(&duck, class, &f).unwrap();
        assert_eq!(d.level, "timeout_risk", "{} ({class}): {d:?}", r["id"]);
        let p = timeout_verdict(&pg, class, &f).unwrap();
        assert_ne!(p.level, "timeout_risk", "{}: PostGIS completed this case in {} ms: {p:?}", r["id"], r["postgis_actual_ms"]);
    }
}
