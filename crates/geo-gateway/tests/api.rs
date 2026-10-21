//! Gateway tests: in-process router (no sockets), real runtime + DuckDB, benchmark data.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use geo_executor::ExecutionContext;
use geo_gateway::{api_router, AppState, GatewayConfig};
use geo_ir::*;
use geo_runtime::Runtime;
use geo_types::*;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data").canonicalize().expect("benchmarks/data (run gen_synthetic.py)")
}

fn state(cfg: GatewayConfig) -> Arc<AppState> {
    Arc::new(AppState::new(Runtime::with_duckdb(), data_dir(), cfg))
}

async fn call(st: &Arc<AppState>, method: &str, uri: &str, body: Option<String>, ct: bool) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if ct {
        b = b.header("content-type", "application/json");
    }
    let req = b.body(body.map(Body::from).unwrap_or_else(Body::empty)).unwrap();
    let resp = api_router(st.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn q(src: &str) -> String {
    json!({"source": src, "kind": "features", "bbox": [104.0, 12.0, 105.0, 13.0], "crs": "EPSG:4326", "limit": 50}).to_string()
}

#[tokio::test]
async fn health_and_sources() {
    let st = state(GatewayConfig::default());
    let (s, j) = call(&st, "GET", "/api/health", None, false).await;
    assert_eq!((s, j["ok"].clone()), (StatusCode::OK, json!(true)));
    let (s, j) = call(&st, "GET", "/api/sources", None, false).await;
    assert_eq!(s, StatusCode::OK);
    let list = j["sources"].as_array().unwrap();
    assert!(list.iter().any(|x| x["id"] == "clustered_10mb_spatial" && x["crs"] == "EPSG:4326" && x["row_groups"] == 26));
}

#[tokio::test]
async fn valid_query_response_schema_and_metric_provenance() {
    let st = state(GatewayConfig::default());
    let (s, j) = call(&st, "POST", "/api/query", Some(q("clustered_10mb_spatial")), true).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    for k in ["plan_id", "ir", "physical", "lineage", "columns", "rows", "rows_total", "geojson", "metrics", "timing"] {
        assert!(j.get(k).is_some(), "missing key {k}");
    }
    assert_eq!(j["physical"]["engine"], "duckdb");
    assert_eq!(j["geojson"]["type"], "FeatureCollection");
    assert_eq!(j["rows_total"], j["geojson"]["features"].as_array().unwrap().len());
    let m = &j["metrics"];
    assert_eq!(m["rows_returned"]["source"], "engine_observed");
    assert_eq!(m["row_groups_total"]["source"], "source_observed");
    assert_eq!(m["row_groups_skipped"]["source"], "source_estimated");
    assert_eq!(m["estimated_bytes_upper_bound"]["source"], "source_estimated");
    // bytes_read must NOT be presented as a fact
    assert_eq!(m["bytes_read"]["source"], "unknown");
    assert!(m["bytes_read"]["value"].is_null());
    assert!(m["row_groups_skipped"]["value"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn invalid_and_malformed_requests() {
    let st = state(GatewayConfig::default());
    // not JSON at all
    let (s, _) = call(&st, "POST", "/api/query", Some("{not json".into()), true).await;
    assert!(s.is_client_error(), "{s}");
    // missing content-type
    let (s, _) = call(&st, "POST", "/api/query", Some(q("clustered_10mb_spatial")), false).await;
    assert!(s.is_client_error(), "{s}");
    // valid JSON, wrong shape
    let (s, _) = call(&st, "POST", "/api/query", Some(json!({"hello": 1}).to_string()), true).await;
    assert!(s.is_client_error(), "{s}");
    // IR that is JSON but not a plan
    let (s, j) = call(&st, "POST", "/api/ir/run", Some(json!({"ir": {"nope": true}}).to_string()), true).await;
    assert_eq!((s, j["error"].clone()), (StatusCode::BAD_REQUEST, json!("bad_ir")));
    // unsupported IR version
    let src = {
        let st2 = state(GatewayConfig::default());
        let ctx = ExecutionContext::new(vec![data_dir()]);
        st2.rt.open_source(data_dir().join("clustered_10mb_spatial.parquet").to_str().unwrap(), &ctx).unwrap()
    };
    let mut ir = serde_json::to_value(Plan::new(src)).unwrap();
    ir["ir_version"] = json!("9.9");
    let (s, j) = call(&st, "POST", "/api/ir/run", Some(json!({"ir": ir}).to_string()), true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(j["message"].as_str().unwrap().contains("unsupported ir_version"));
    // bad source id / unknown kind / CRS mismatch
    let (s, _) = call(&st, "POST", "/api/query", Some(q("../etc")), true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let mut bad: Value = serde_json::from_str(&q("clustered_10mb_spatial")).unwrap();
    bad["kind"] = json!("drop_table");
    let (s, _) = call(&st, "POST", "/api/query", Some(bad.to_string()), true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let mut bad: Value = serde_json::from_str(&q("clustered_10mb_spatial")).unwrap();
    bad["crs"] = json!("EPSG:3857");
    let (s, j) = call(&st, "POST", "/api/query", Some(bad.to_string()), true).await;
    assert_eq!((s, j["error"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("crs_mismatch")));
}

#[tokio::test]
async fn execution_error_is_controlled_and_runtime_stays_usable() {
    let st = state(GatewayConfig::default());
    let mut bad: Value = serde_json::from_str(&q("clustered_10mb_spatial")).unwrap();
    bad["filter"] = json!({"column": "no_such_column", "op": "eq", "value": 1});
    let (s, j) = call(&st, "POST", "/api/query", Some(bad.to_string()), true).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{j}");
    assert_eq!(j["error"], "invalid_plan");
    let (s, _) = call(&st, "POST", "/api/query", Some(q("clustered_10mb_spatial")), true).await;
    assert_eq!(s, StatusCode::OK);
}

fn ir_source_outside(path: &str) -> Value {
    // a hand-written IR that points at a file outside the data root
    json!({"ir": {"ir_version": "0.1", "root": {"op": "source", "uri": path, "format": "geo_parquet",
        "schema": {"fields": [], "geometry_columns": []}}}})
}

#[tokio::test]
async fn unauthorized_source_is_denied_without_an_existence_oracle() {
    let st = state(GatewayConfig::default());
    // a *valid* parquet outside the root (copy of a benchmark file in temp)
    let outside = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("gf-outside-{}.parquet", std::process::id()));
    std::fs::copy(data_dir().join("clustered_10mb_spatial.parquet"), &outside).unwrap();
    let missing = std::fs::canonicalize(std::env::temp_dir()).unwrap().join("gf-definitely-missing.parquet");
    let (s1, j1) = call(&st, "POST", "/api/ir/run", Some(ir_source_outside(outside.to_str().unwrap()).to_string()), true).await;
    let (s2, j2) = call(&st, "POST", "/api/ir/run", Some(ir_source_outside(missing.to_str().unwrap()).to_string()), true).await;
    let (s3, j3) = call(&st, "POST", "/api/ir/explain", Some(ir_source_outside(outside.to_str().unwrap()).to_string()), true).await;
    for (s, j) in [(s1, &j1), (s2, &j2), (s3, &j3)] {
        assert_eq!(s, StatusCode::FORBIDDEN, "{j}");
        assert_eq!(j["error"], "access_denied");
        assert_eq!(j["message"], "source not permitted");
    }
    assert_eq!(j1, j2, "existing and missing outside files must be indistinguishable");
    let _ = std::fs::remove_file(outside);
    // dot-dot and URL sources
    let dd = format!("{}/../gf-x.parquet", data_dir().display());
    for uri in [dd.as_str(), "http://example.com/x.parquet", "s3://b/k.parquet"] {
        let (s, _) = call(&st, "POST", "/api/ir/run", Some(ir_source_outside(uri).to_string()), true).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}");
    }
}

fn heavy_join_plan(st: &AppState) -> Option<Plan> {
    let p = data_dir().join("clustered_100mb_spatial.parquet");
    if !p.exists() {
        eprintln!("SKIP: 100mb data missing");
        return None;
    }
    let ctx = ExecutionContext::new(vec![data_dir()]);
    let src = || st.rt.open_source(p.to_str().unwrap(), &ctx).unwrap();
    Some(Plan::new(Node::Aggregate {
        input: Box::new(Node::SpatialJoin {
            left: Box::new(src()),
            right: Box::new(src()),
            left_column: "geometry".into(),
            right_column: "geometry".into(),
            predicate: SpatialPredicate::Intersects,
            join_type: JoinType::Inner,
            right_prefix: "r_".into(),
        }),
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
    }))
}

#[tokio::test]
async fn execution_timeout_really_interrupts_and_runtime_recovers() {
    let cfg = GatewayConfig { query_timeout: Duration::from_millis(400), ..GatewayConfig::default() };
    let st = state(cfg);
    let Some(plan) = heavy_join_plan(&st) else { return };
    let t0 = Instant::now();
    let (s, j) = call(&st, "POST", "/api/ir/run", Some(json!({"ir": plan}).to_string()), true).await;
    let took = t0.elapsed();
    assert_eq!(s, StatusCode::GATEWAY_TIMEOUT, "{j}");
    assert_eq!(j["error"], "execution_timeout");
    assert!(took < Duration::from_secs(8), "interrupt must be prompt, took {took:?}");
    // the slot is free again and the runtime still works
    assert_eq!(st.slots.available_permits(), GatewayConfig::default().max_concurrent_queries);
    let (s, _) = call(&st, "POST", "/api/query", Some(q("clustered_10mb_spatial")), true).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn client_disconnect_cancels_the_running_query() {
    // generous timeout: only the cancel path can stop the query early
    let cfg = GatewayConfig { query_timeout: Duration::from_secs(120), ..GatewayConfig::default() };
    let st = state(cfg);
    let Some(plan) = heavy_join_plan(&st) else { return };
    let max = GatewayConfig::default().max_concurrent_queries;
    let body = json!({"ir": plan}).to_string();
    // the client gives up after 300 ms: the request future is dropped
    let fut = call(&st, "POST", "/api/ir/run", Some(body), true);
    assert!(tokio::time::timeout(Duration::from_millis(300), fut).await.is_err(), "query should still be running");
    // the engine must unwind soon after (slot released only when the query truly ended)
    let t0 = Instant::now();
    while st.slots.available_permits() < max {
        assert!(t0.elapsed() < Duration::from_secs(15), "query kept running after the client left");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (s, _) = call(&st, "POST", "/api/query", Some(q("clustered_10mb_spatial")), true).await;
    assert_eq!(s, StatusCode::OK);
}

// ---------- planner hardening: probe, reports, health, network (ADR-009/010) ----------

fn probe_json(engine: &str, session: &str, version: &str) -> Value {
    json!({
        "engine": engine, "session": session, "engine_version": version, "runtime_version": "v1", "captured_at_ms": 1, "duration_ms": 5.0,
        "checks": [{"name": "init", "ok": true, "required": true, "detail": "ok", "ms": 1.0}],
        "memory_class": "high", "device_memory_gb": 8.0, "js_heap_limit_mb": 4096, "supported_ops": ["source", "limit"]
    })
}

#[tokio::test]
async fn probe_is_stored_validated_and_visible_in_state() {
    let st = state(GatewayConfig::default());
    let (s, j) = call(&st, "POST", "/api/probe", Some(probe_json("duckdb-wasm", "s1", "1.0").to_string()), true).await;
    assert_eq!((s, j["stored"].clone()), (StatusCode::OK, json!(true)), "{j}");
    let (_, j) = call(&st, "GET", "/api/state", None, false).await;
    assert_eq!(j["probes"]["duckdb-wasm"]["session"], "s1");
    // a new page load replaces the session; a new engine version is reported
    let (_, j) = call(&st, "POST", "/api/probe", Some(probe_json("duckdb-wasm", "s2", "2.0").to_string()), true).await;
    assert_eq!((j["replaced_session"].clone(), j["version_changed"].clone()), (json!(true), json!(true)));
    // malformed / hostile probes are refused
    let mut bad = probe_json("", "s", "1");
    let (s, _) = call(&st, "POST", "/api/probe", Some(bad.to_string()), true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    bad = probe_json("x", "s", "1");
    bad["memory_class"] = json!("enormous");
    let (s, _) = call(&st, "POST", "/api/probe", Some(bad.to_string()), true).await;
    assert!(s.is_client_error());
    let (_, j) = call(&st, "POST", "/api/probe/invalidate", Some(json!({"engine": "duckdb-wasm"}).to_string()), true).await;
    assert_eq!(j["invalidated"], json!(true));
}

#[tokio::test]
async fn client_reports_are_classified_and_drive_health_and_incompatibilities() {
    let st = state(GatewayConfig::default());
    let rep = |msg: &str| json!({"engine": "duckdb-wasm", "source": "clustered_10mb_spatial", "ok": false, "message": msg}).to_string();
    let (_, j) = call(&st, "POST", "/api/report", Some(rep("Target crashed")), true).await;
    assert_eq!(j["class"], "ENGINE_CRASH");
    let (_, j) = call(&st, "POST", "/api/report", Some(rep("TransactionContext::ActiveTransaction called without active transaction")), true).await;
    assert_eq!(j["class"], "UNSUPPORTED_FORMAT");
    let (_, j) = call(&st, "POST", "/api/report", Some(rep("browser query cancelled")), true).await;
    assert_eq!(j["class"], "CANCELLED");
    let (_, s) = call(&st, "GET", "/api/state", None, false).await;
    assert_eq!(s["incompat"].as_array().unwrap().len(), 1, "only the deterministic failure is remembered per source");
    // a successful run with telemetry feeds the calibration
    let ok = json!({"engine": "duckdb-wasm", "source": "clustered_10mb_spatial", "ok": true, "kind": "features", "raw_selected": 226.0,
        "total_rows": 130000, "est_rows": 226, "actual_rows": 2, "limit": 2000, "actual_ms": 40.0}).to_string();
    call(&st, "POST", "/api/report", Some(ok), true).await;
    let (_, t) = call(&st, "GET", "/api/telemetry", None, false).await;
    assert_eq!(t["observations"].as_array().unwrap().len(), 1);
    // path-like source ids are refused (no log/state poisoning through the report endpoint)
    let bad = json!({"engine": "duckdb-wasm", "source": "../x", "ok": true}).to_string();
    let (s, _) = call(&st, "POST", "/api/report", Some(bad), true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, j) = call(&st, "POST", "/api/state/reset", Some("{}".into()), true).await;
    assert_eq!(j["reset"], json!(true));
    let (_, s) = call(&st, "GET", "/api/state", None, false).await;
    assert!(s["incompat"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn server_runs_feed_telemetry_and_health_and_templates_cover_count() {
    let st = state(GatewayConfig::default());
    let body = json!({"source": "clustered_10mb_spatial", "kind": "count", "bbox": [104.0, 12.0, 105.0, 13.0], "crs": "EPSG:4326"}).to_string();
    let (s, j) = call(&st, "POST", "/api/query", Some(body), true).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["rows_total"], 1, "ungrouped aggregate = one row");
    let (_, t) = call(&st, "GET", "/api/telemetry", None, false).await;
    let o = &t["observations"][0];
    assert_eq!((o["kind"].clone(), o["engine"].clone(), o["actual_rows"].clone()), (json!("count"), json!("duckdb"), json!(1)));
    let (_, s) = call(&st, "GET", "/api/state", None, false).await;
    assert_eq!(s["health"]["duckdb"]["events"], 1);
}

#[tokio::test]
async fn network_override_and_unknown_profiles() {
    let st = state(GatewayConfig::default());
    let mk = |net: Value| json!({"source": "clustered_10mb_spatial", "kind": "features", "bbox": [104.0, 12.0, 105.0, 13.0], "crs": "EPSG:4326", "limit": 50, "network": net}).to_string();
    let (s, j) = call(&st, "POST", "/api/query/plan", Some(mk(json!("MOBILE"))), true).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["physical"]["assessment"]["network"]["name"], "MOBILE");
    let (s, _) = call(&st, "POST", "/api/query/plan", Some(mk(json!("DIALUP"))), true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, j) = call(&st, "POST", "/api/network", Some(json!({"profile": "FAST_LAN"}).to_string()), true).await;
    assert_eq!((s, j["network"]["rtt_ms"].clone()), (StatusCode::OK, json!(5.0)));
}

#[tokio::test]
async fn simulated_network_delays_responses_by_latency_and_bandwidth() {
    let slow = Arc::new(AppState::new(Runtime::with_duckdb(), data_dir(), GatewayConfig::default()).with_simulated_network(geo_planner::NetworkProfile::by_name("HIGH_LATENCY").unwrap()));
    let t0 = Instant::now();
    let (s, _) = call(&slow, "GET", "/api/health", None, false).await;
    assert_eq!(s, StatusCode::OK);
    assert!(t0.elapsed() >= Duration::from_millis(150), "rtt 150 ms + overhead: {:?}", t0.elapsed());
    let fast = state(GatewayConfig::default());
    let t1 = Instant::now();
    call(&fast, "GET", "/api/health", None, false).await;
    assert!(t1.elapsed() < Duration::from_millis(100));
}
