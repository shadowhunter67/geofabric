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
    let outside = std::env::temp_dir().join(format!("gf-outside-{}.parquet", std::process::id()));
    std::fs::copy(data_dir().join("clustered_10mb_spatial.parquet"), &outside).unwrap();
    let missing = std::env::temp_dir().join("gf-definitely-missing.parquet");
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
