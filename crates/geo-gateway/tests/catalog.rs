//! Gateway catalog and multi-directory tests.  Fixtures are deliberately tiny
//! byte files: these endpoints do not require a valid Parquet or TIFF payload.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use geo_gateway::{api_router, AppState, GatewayConfig};
use geo_runtime::Runtime;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("geo-gateway-{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()));
    std::fs::create_dir_all(&path).expect("create temp directory");
    path
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, bytes).expect("write fixture");
}

fn state(data_dirs: Vec<PathBuf>, manifests: PathBuf, root: PathBuf) -> Arc<AppState> {
    Arc::new(
        AppState::new_with_dirs(Runtime::with_duckdb(), data_dirs, GatewayConfig::default())
            .with_catalog_paths(manifests, root),
    )
}

async fn response(st: &Arc<AppState>, uri: &str, range: Option<&str>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder().uri(uri);
    if let Some(range) = range {
        request = request.header("range", range);
    }
    let response = api_router(st.clone()).oneshot(request.body(Body::empty()).expect("request")).await.expect("router");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.expect("body").to_bytes().to_vec();
    (status, headers, body)
}

#[tokio::test]
async fn data_directories_have_priority_and_file_ranges_work_in_later_directories() {
    let root = temp_dir("dirs");
    let first = root.join("first");
    let second = root.join("second");
    write(&first.join("same.parquet"), b"first");
    write(&first.join("first_only.parquet"), b"first-only");
    write(&second.join("same.parquet"), b"second");
    write(&second.join("second_only.parquet"), b"abcdef");

    let st = state(
        vec![first.canonicalize().expect("first"), second.canonicalize().expect("second")],
        root.join("missing-manifests"),
        root.canonicalize().expect("root"),
    );

    let (status, _, body) = response(&st, "/api/sources", None).await;
    assert_eq!(status, StatusCode::OK);
    let catalog: Value = serde_json::from_slice(&body).expect("sources json");
    let ids: Vec<_> = catalog["sources"].as_array().expect("source array").iter().map(|source| source["id"].as_str().expect("id")).collect();
    assert_eq!(ids.iter().filter(|id| **id == "same").count(), 1);
    assert!(ids.contains(&"first_only"));
    assert!(ids.contains(&"second_only"));

    let (status, headers, body) = response(&st, "/api/files/second_only", Some("bytes=1-3")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(body, b"bcd");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn catalog_and_raster_endpoint_use_only_manifested_repository_files() {
    let root = temp_dir("catalog");
    let data = root.join("data/local");
    let manifests = root.join("data/manifests");
    write(&data.join("vector.parquet"), b"vector");
    write(&data.join("dem.tif"), b"tiff-data");
    write(&data.join("unlisted.tif"), b"unlisted");

    write(
        &manifests.join("vector.json"),
        br#"{
            "id":"vector","title":{"vi":"Vector","en":"Vector"},"group":"places","provider":"Test",
            "license":{"name":"Test","url":"https://example.test/license","attribution":"Shared attribution","redistribution":"allowed"},
            "coverage":{"name":"Test","bbox":[0,0,1,1]},"crs":"EPSG:4326","geometry_type":"Point",
            "format_normalized":"GeoParquet","files":[{"path":"data/local/vector.parquet","size_bytes":6,"rows":1}],"notes":"present"
        }"#,
    );
    write(
        &manifests.join("missing.json"),
        br#"{
            "id":"missing","title":{"vi":"Missing","en":"Missing"},"group":"places","provider":"Test",
            "license":{"name":"Test","url":"","attribution":"Shared attribution","redistribution":"allowed"},
            "coverage":{"name":"Test","bbox":[0,0,1,1]},"crs":"EPSG:4326","geometry_type":"Point",
            "format_normalized":"GeoParquet","files":[{"path":"data/local/missing.parquet","size_bytes":null,"rows":null}],"notes":"missing"
        }"#,
    );
    write(
        &manifests.join("dem.json"),
        br#"{
            "id":"dem","title":{"vi":"DEM","en":"DEM"},"group":"raster","provider":"Test",
            "license":{"name":"Test","url":"","attribution":"Raster attribution","redistribution":"allowed"},
            "coverage":{"name":"Test","bbox":[0,0,1,1]},"crs":"EPSG:4326","geometry_type":"Raster",
            "format_normalized":"COG","files":[{"path":"data/local/dem.tif","size_bytes":9,"rows":null}],"notes":"raster"
        }"#,
    );
    write(&manifests.join("broken.json"), b"{");
    write(&manifests.join("example.json"), b"{not valid");

    let st = state(
        vec![data.canonicalize().expect("data")],
        manifests.canonicalize().expect("manifests"),
        root.canonicalize().expect("root"),
    );
    let (status, _, body) = response(&st, "/api/catalog", None).await;
    assert_eq!(status, StatusCode::OK);
    let catalog: Value = serde_json::from_slice(&body).expect("catalog json");
    assert_eq!(catalog["datasets"].as_array().expect("datasets").len(), 3);
    assert_eq!(catalog["datasets"].as_array().expect("datasets")[0]["id"], "dem");
    assert!(catalog["datasets"].as_array().expect("datasets").iter().any(|dataset| dataset["id"] == "vector" && dataset["present"] == true && dataset["source_ids"] == json!(["vector"])));
    assert!(catalog["datasets"].as_array().expect("datasets").iter().any(|dataset| dataset["id"] == "missing" && dataset["present"] == false));
    assert_eq!(catalog["attributions"], json!(["Raster attribution", "Shared attribution"]));
    assert_eq!(catalog["errors"].as_array().expect("errors").len(), 1);
    assert_eq!(catalog["errors"][0]["manifest"], "broken.json");

    let (status, headers, body) = response(&st, "/api/rasters/dem/dem.tif", Some("bytes=0-3")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(headers["content-type"], "image/tiff");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(body, b"tiff");

    let (status, _, _) = response(&st, "/api/rasters/dem/unlisted.tif", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = response(&st, "/api/rasters/../dem.tif", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = response(&st, "/api/rasters/dem/..%5Cdem.tif", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn one_data_directory_remains_a_valid_configuration() {
    let root = temp_dir("single");
    let data = root.join("data");
    write(&data.join("only.parquet"), b"single");

    let st = Arc::new(AppState::new(
        Runtime::with_duckdb(),
        data.canonicalize().expect("data"),
        GatewayConfig::default(),
    ));
    let (status, _, body) = response(&st, "/api/files/only", Some("bytes=0-2")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"sin");

    let _ = std::fs::remove_dir_all(root);
}
