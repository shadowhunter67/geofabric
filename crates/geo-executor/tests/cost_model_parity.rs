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
    assert_eq!(model.model_version, "postgis-cost-v2");
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
