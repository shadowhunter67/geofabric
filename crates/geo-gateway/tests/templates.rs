use arrow_array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use geo_gateway::{api_router, AppState, GatewayConfig};
use geo_runtime::Runtime;
use http_body_util::BodyExt;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<u8> {
    let mut out = vec![1];
    out.extend(3u32.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(5u32.to_le_bytes());
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] {
        out.extend(x.to_le_bytes());
        out.extend(y.to_le_bytes());
    }
    out
}

fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<u8> {
    let mut out = vec![1];
    out.extend(2u32.to_le_bytes());
    out.extend(2u32.to_le_bytes());
    for (x, y) in [(x0, y0), (x1, y1)] {
        out.extend(x.to_le_bytes());
        out.extend(y.to_le_bytes());
    }
    out
}

fn write(
    path: &Path,
    crs: u32,
    ids: Vec<i64>,
    classes: Vec<Option<&str>>,
    names: Vec<Option<&str>>,
    geometries: Vec<Vec<u8>>,
) {
    let geo = format!(
        r#"{{"version":"1.1.0","primary_column":"geometry","columns":{{"geometry":{{"encoding":"WKB","geometry_types":[],"crs":{{"id":{{"authority":"EPSG","code":{crs}}}}}}}}}}}"#
    );
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("class", DataType::Utf8, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("geometry", DataType::Binary, false),
    ]));
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".into(), geo)]))
        .build();
    let mut writer = ArrowWriter::try_new(std::fs::File::create(path).expect("fixture"), schema.clone(), Some(props)).expect("writer");
    let refs = geometries.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(classes)) as ArrayRef,
            Arc::new(StringArray::from(names)) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
}

fn fixtures() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "geo-gateway-templates-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("directory");
    let none = || vec![None, None, None, None, None];
    write(
        &root.join("buildings.parquet"),
        4326,
        vec![1, 2, 3, 4, 5],
        none(),
        none(),
        vec![
            rect(106.0019, 10.7999, 106.0021, 10.8001),
            rect(106.0039, 10.801247, 106.0041, 10.801447),
            rect(106.0059, 10.80115, 106.0061, 10.80135),
            rect(106.0139, 10.8100, 106.0141, 10.8102),
            rect(106.0099, 10.8004, 106.0101, 10.8006),
        ],
    );
    write(
        &root.join("roads.parquet"),
        4326,
        vec![1, 2, 3],
        vec![Some("a"), Some("b"), Some("a")],
        vec![None, None, None],
        vec![
            line(105.99, 10.8000, 106.03, 10.8000),
            line(105.99, 10.8025, 106.03, 10.8025),
            line(105.99, 10.8300, 106.03, 10.8300),
        ],
    );
    write(
        &root.join("wards.parquet"),
        4326,
        vec![1, 2],
        vec![None, None],
        vec![Some("west"), Some("east")],
        vec![
            rect(105.99, 10.79, 106.01, 10.82),
            rect(106.01, 10.79, 106.03, 10.82),
        ],
    );
    write(
        &root.join("projected.parquet"),
        3857,
        vec![1],
        vec![None],
        vec![None],
        vec![rect(0.0, 0.0, 1.0, 1.0)],
    );
    root.canonicalize().expect("canonical")
}

fn state() -> Arc<AppState> {
    Arc::new(AppState::new(Runtime::with_duckdb(), fixtures(), GatewayConfig::default()))
}

async fn call(st: &Arc<AppState>, value: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/query")
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .expect("request");
    let response = api_router(st.clone()).oneshot(req).await.expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn base(kind: &str) -> Value {
    json!({
        "source": "buildings",
        "kind": kind,
        "bbox": [105.99, 10.79, 106.03, 10.82],
        "crs": "EPSG:4326"
    })
}

/// ids of the returned rows, sorted (`near` returns one row per feature/road pair: a feature near two roads appears twice)
fn ids(value: &Value) -> Vec<i64> {
    let mut rows = value["rows"].as_array().expect("rows").iter().map(|row| row["id"].as_i64().expect("id")).collect::<Vec<_>>();
    rows.sort_unstable();
    rows
}

#[tokio::test]
async fn near_honors_distance_and_road_filter() {
    let st = state();
    let mut request = base("near");
    request["with"] = json!({"source": "roads", "distance_m": 200});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    // distances in metres (1 degree = 111320 m): b1 0 m to road 1; b2 139 m to road 1 and 117 m to road 2; b3 128 m to both;
    // b5 44 m to road 1 (211 m to road 2); b4 and road 3 are far. Pairs within 200 m: b1 x1, b2 x2, b3 x2, b5 x1.
    assert_eq!(ids(&response), vec![1, 2, 2, 3, 3, 5]);
    assert_eq!(response["geojson"]["type"], "FeatureCollection");
    assert_eq!(response["approximation"]["distance_m"], json!(200.0));

    let mut request = base("near");
    request["with"] = json!({"source": "roads", "distance_m": 100});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(ids(&response), vec![1, 5], "within 100 m only b1 (0 m) and b5 (44 m)");

    let mut request = base("near");
    request["with"] = json!({"source": "roads", "distance_m": 200, "filter": {"column": "class", "op": "eq", "value": "a"}});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(ids(&response), vec![1, 2, 3, 5]);
}

#[tokio::test]
async fn near_count_counts_each_feature_once() {
    let st = state();
    let mut request = base("near_count");
    request["with"] = json!({"source": "roads", "distance_m": 200});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["rows"], json!([{"count": 4}]), "building 3 is near two roads but is one building");

    let mut request = base("near_count");
    request["with"] = json!({"source": "roads", "distance_m": 100});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["rows"][0]["count"], json!(2));
}

#[tokio::test]
async fn join_count_by_counts_boundary_buildings_for_each_intersected_ward() {
    let st = state();
    let mut request = base("join_count_by");
    request["with"] = json!({"source": "wards", "group_by": "name"});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(response["geojson"].is_null());
    assert_eq!(response["rows"], json!([
        {"r_name": "west", "count": 4},
        {"r_name": "east", "count": 2}
    ]));
}

#[tokio::test]
async fn template_validation_and_in_filter_errors_are_controlled() {
    let st = state();

    let request = base("near");
    let (status, _) = call(&st, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut request = base("features");
    request["with"] = json!({"source": "roads", "distance_m": 200});
    let (status, _) = call(&st, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for distance_m in [0, 6000] {
        let mut request = base("near");
        request["with"] = json!({"source": "roads", "distance_m": distance_m});
        let (status, _) = call(&st, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let mut request = base("near");
    request["with"] = json!({"source": "roads", "distance_m": 200, "unexpected": true});
    let (status, _) = call(&st, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut request = base("near");
    request["with"] = json!({"source": "missing", "distance_m": 200});
    let (status, _) = call(&st, request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let mut request = base("near");
    request["source"] = json!("projected");
    request["bbox"] = json!([0.0, 0.0, 2.0, 2.0]);
    request["crs"] = json!("EPSG:3857");
    request["with"] = json!({"source": "projected", "distance_m": 200});
    let (status, _) = call(&st, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let request = json!({
        "source": "roads",
        "kind": "features",
        "bbox": [105.99, 10.79, 106.03, 10.82],
        "crs": "EPSG:4326",
        "filter": {"column": "class", "op": "in", "values": ["a"]}
    });
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["rows_total"], json!(1), "class a = roads 1 and 3; road 3 lies outside the viewport");

    let request = json!({
        "source": "roads",
        "kind": "features",
        "bbox": [105.99, 10.79, 106.03, 10.82],
        "crs": "EPSG:4326",
        "filter": {"column": "class", "op": "in", "values": []}
    });
    let (status, _) = call(&st, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
