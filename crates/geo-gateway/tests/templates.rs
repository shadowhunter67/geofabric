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
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!(
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
    // four clusters (south-west, south-east, north-west, north-east corners of the viewport), 400 buildings each, rows stored
    // cluster after cluster: exactly the order of a spatially sorted file, where "the first N rows" is one corner of the map
    let mut cluster_ids = Vec::new();
    let mut cluster_geoms = Vec::new();
    for (k, (cx, cy)) in [(105.992, 10.792), (106.028, 10.792), (105.992, 10.818), (106.028, 10.818)].into_iter().enumerate() {
        for i in 0..400i64 {
            let (dx, dy) = ((i % 20) as f64 * 0.00008, (i / 20) as f64 * 0.00008);
            cluster_ids.push(k as i64 * 400 + i);
            cluster_geoms.push(rect(cx + dx, cy + dy, cx + dx + 0.00004, cy + dy + 0.00004));
        }
    }
    write(&root.join("clusters.parquet"), 4326, cluster_ids, vec![None; 1600], vec![None; 1600], cluster_geoms);
    // 120 groups of exactly 3 buildings each: every group ties on the count, and only 50 groups are returned
    let tie_names: Vec<String> = (0..120).map(|g| format!("g{g:03}")).flat_map(|n| std::iter::repeat(n).take(3)).collect();
    write(
        &root.join("ties.parquet"),
        4326,
        (0..360).collect(),
        tie_names.iter().map(|n| Some(n.as_str())).collect(),
        vec![None; 360],
        (0..360i64).map(|i| rect(106.0 + i as f64 * 0.00005, 10.8, 106.0 + i as f64 * 0.00005 + 0.00002, 10.8002)).collect(),
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

/// ids of the returned rows, sorted
fn ids(value: &Value) -> Vec<i64> {
    let mut rows = value["rows"].as_array().expect("rows").iter().map(|row| row["id"].as_i64().expect("id")).collect::<Vec<_>>();
    rows.sort_unstable();
    rows
}

// Fixture geometry (all near lon 106.0, lat 10.8; UTM zone 48N, 1 degree of latitude = about 110.6 km there):
//   road 1 at lat 10.8000 (class a), road 2 at lat 10.8025 (class b), road 3 far away at lat 10.83 (class a).
//   b1 straddles road 1 (0 m); b2 is 138 m from road 1 and 116 m from road 2; b3 is 127 m from BOTH roads;
//   b5 is 44 m from road 1 and 210 m from road 2; b4 is far from every road.

#[tokio::test]
async fn near_is_a_metric_semi_join_each_building_once() {
    let st = state();
    let mut request = base("near");
    request["with"] = json!({"source": "roads", "distance_m": 200});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(ids(&response), vec![1, 2, 3, 5], "b3 is near two roads but appears once");
    assert_eq!(response["geojson"]["type"], "FeatureCollection");
    assert_eq!(response["metric_semantics"]["distance_m"], json!(200.0));
    assert_eq!(response["metric_semantics"]["crs"], json!("EPSG:32648"));

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
async fn near_count_equals_the_number_of_rows_near_returns() {
    let st = state();
    for (d, expected) in [(200, 4), (100, 2), (30, 1)] {
        let mut request = base("near_count");
        request["with"] = json!({"source": "roads", "distance_m": d});
        let (status, response) = call(&st, request).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["rows"], json!([{"count": expected}]), "distance {d} m");
    }
}

#[tokio::test]
async fn sampling_thins_rows_deterministically_and_never_touches_counts() {
    let st = state();
    let mut full = base("features");
    full["source"] = json!("roads");
    let (_, all) = call(&st, full.clone()).await;
    let total = all["rows_total"].as_i64().expect("rows_total");
    let mut a = full.clone();
    a["sample_modulus"] = json!(2);
    let (status, first) = call(&st, a.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (_, second) = call(&st, a).await;
    assert_eq!(ids(&first), ids(&second), "same request, same sample");
    assert!(first["rows_total"].as_i64().expect("rows") <= total);
    let mut quarter = full.clone();
    quarter["sample_modulus"] = json!(4);
    let (_, q) = call(&st, quarter).await;
    for id in ids(&q) {
        assert!(ids(&first).contains(&id), "the sample at modulus 4 is contained in the one at modulus 2");
    }
    // counts and aggregates refuse sampling
    let mut count = base("count");
    count["sample_modulus"] = json!(2);
    let (status, _) = call(&st, count).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut bad = full.clone();
    bad["sample_modulus"] = json!(3);
    let (status, _) = call(&st, bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "not a power of two");
}

#[tokio::test]
async fn metric_distance_is_refused_where_no_metric_crs_exists() {
    let st = state();
    // a viewport 20 degrees wide at the equator: no single UTM zone is accurate enough (scale error about 1.5 %)
    let mut request = base("near");
    request["bbox"] = json!([95.0, -1.0, 115.0, 1.0]);
    request["with"] = json!({"source": "roads", "distance_m": 200});
    let (status, response) = call(&st, request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["error"], json!("metric_crs_unavailable"));
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

/// Which of the four corner clusters (SW, SE, NW, NE) a returned feature belongs to.
fn cluster_of(feature: &Value) -> usize {
    let ring = &feature["geometry"]["coordinates"][0][0];
    let (x, y) = (ring[0].as_f64().expect("x"), ring[1].as_f64().expect("y"));
    (if x > 106.01 { 1 } else { 0 }) + (if y > 10.805 { 2 } else { 0 })
}

fn clusters_request() -> Value {
    json!({"source": "clusters", "kind": "features", "bbox": [105.99, 10.79, 106.03, 10.82], "crs": "EPSG:4326"})
}

#[tokio::test]
async fn cutting_at_the_first_rows_shows_one_corner_but_the_thinned_answer_shows_all_four() {
    let st = state();
    // OLD behaviour (LIMIT = first rows of the file): 400 rows are exactly one cluster
    let mut cut = clusters_request();
    cut["limit"] = json!(400);
    let (status, cut) = call(&st, cut).await;
    assert_eq!(status, StatusCode::OK, "{cut}");
    let features = cut["geojson"]["features"].as_array().expect("features");
    assert_eq!(features.len(), 400);
    assert!(features.iter().all(|f| cluster_of(f) == 0), "file order is one corner of the map");

    // NEW behaviour: the full count is asked, then the same request thinned (1600 / 4 = 400 rows expected)
    let mut count = clusters_request();
    count["kind"] = json!("count");
    let (_, count) = call(&st, count).await;
    assert_eq!(count["rows"], json!([{"count": 1600}]), "the count is the FULL logical count, never sampled");
    let mut sampled = clusters_request();
    sampled["sample_modulus"] = json!(4);
    let (status, sampled) = call(&st, sampled).await;
    assert_eq!(status, StatusCode::OK, "{sampled}");
    let features = sampled["geojson"]["features"].as_array().expect("features");
    let mut per = [0usize; 4];
    for f in features {
        per[cluster_of(f)] += 1;
    }
    assert!(features.len() > 300 && features.len() < 500, "about 1/4 of 1600, got {}", features.len());
    for (k, n) in per.iter().enumerate() {
        // binomial(400, 1/4): mean 100, sd 8.7; 55..145 is more than 5 sd wide
        assert!((55..=145).contains(n), "cluster {k} has {n} of {} sampled features: every corner must be represented", features.len());
    }
    // deterministic: the same request returns the same rows
    let mut again = clusters_request();
    again["sample_modulus"] = json!(4);
    let (_, again) = call(&st, again).await;
    assert_eq!(sampled["result_digest"], again["result_digest"]);
}

#[tokio::test]
async fn the_sample_of_a_filtered_query_is_a_subset_of_the_filtered_result() {
    let st = state();
    let mut full = clusters_request();
    full["filter"] = json!({"column": "id", "op": "lt", "value": 800});
    let (_, all) = call(&st, full.clone()).await;
    let all_ids: Vec<i64> = all["geojson"]["features"].as_array().expect("f").iter().map(|f| f["properties"]["id"].as_i64().expect("id")).collect();
    assert_eq!(all_ids.len(), 800);
    let mut sampled = full;
    sampled["sample_modulus"] = json!(8);
    let (_, s8) = call(&st, sampled).await;
    for f in s8["geojson"]["features"].as_array().expect("f") {
        assert!(all_ids.contains(&f["properties"]["id"].as_i64().expect("id")));
    }
}

/// Regression (final planner validation): `count_by` cut at 50 groups with ties on the count must cut at the same place in every
/// engine. Ties are ordered by the group key, ascending; the browser SQL has the same ORDER BY (golden fixture `count_by_sorted`).
/// Before the tie-break, 8 of 72 cross-engine digests of a held-out run differed on exactly this.
#[tokio::test]
async fn count_by_ties_are_cut_by_the_group_key_so_every_engine_returns_the_same_top_50() {
    let st = state();
    let req = json!({"source": "ties", "kind": "count_by", "group_by": "class", "bbox": [105.99, 10.79, 106.03, 10.82], "crs": "EPSG:4326", "browser": false});
    let (status, first) = call(&st, req.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let rows = first["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 50);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["class"], json!(format!("g{i:03}")), "row {i} must be the {i}th group by key among equal counts");
        assert_eq!(row["count"].as_i64(), Some(3));
    }
    for _ in 0..3 {
        let (_, again) = call(&st, req.clone()).await;
        assert_eq!(again["result_digest"], first["result_digest"], "stable digest across runs");
    }
}
