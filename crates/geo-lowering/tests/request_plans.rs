//! Golden request -> PlanGraph fixtures (RFC-004, second layer). Each fixture is a user request, the Source nodes it names, and the exact PlanGraph
//! (or the structured error) the canonical builder produces, plus the browser capability verdict. They are generated from the NATIVE builder and
//! checked two ways: here (native `build_plan`, and the very function the WebAssembly module exports, `build_plan_wire`) and by
//! `web/test/plan.test.ts`, which loads the real wasm and requires the same bytes. Same request, same bytes, in every environment.
//!
//! Regenerate after an INTENDED change:  GEN_PLANS=1 cargo test -p geo-lowering --test request_plans

use geo_ir::{Node, SourceFormat};
use geo_plan::{build_plan, PlanRequest, Sources};
use geo_types::{BoundingBox, Crs, Field, FieldType, GeometryColumn, GeometryType, SpatialSchema};
use serde_json::{json, Value};
use std::path::PathBuf;

fn field(n: &str, t: FieldType) -> Field {
    Field { name: n.into(), data_type: t }
}

fn source(uri: &str, gt: GeometryType, mut fields: Vec<Field>) -> Node {
    fields.push(field("geometry", FieldType::Other("geometry".into())));
    fields.push(field("bbox", FieldType::Other("struct".into())));
    Node::Source {
        uri: uri.into(),
        format: SourceFormat::GeoParquet,
        schema: SpatialSchema { fields, geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: gt, crs: Crs::OgcCrs84 }] },
        statistics: geo_ir::SourceStats {
            row_count: Some(1000),
            total_bytes: Some(123_456),
            row_groups: Some(4),
            bbox: Some(BoundingBox::new(106.0, 10.0, 107.0, 11.0, Crs::OgcCrs84).unwrap()),
        },
    }
}

fn sources() -> Value {
    let s = Sources::from([
        ("places".to_string(), source("demo/places.parquet", GeometryType::Point, vec![field("id", FieldType::Utf8), field("name", FieldType::Utf8), field("category", FieldType::Utf8), field("confidence", FieldType::Float64)])),
        ("buildings".to_string(), source("demo/buildings.parquet", GeometryType::Polygon, vec![field("id", FieldType::Utf8), field("class", FieldType::Utf8), field("height", FieldType::Float64)])),
        ("roads".to_string(), source("demo/roads.parquet", GeometryType::LineString, vec![field("id", FieldType::Utf8), field("class", FieldType::Utf8)])),
        ("admin".to_string(), source("demo/admin.parquet", GeometryType::Polygon, vec![field("id", FieldType::Utf8), field("name", FieldType::Utf8), field("subtype", FieldType::Utf8)])),
    ]);
    serde_json::to_value(&s).unwrap()
}

fn base(kind: &str, source: &str) -> Value {
    json!({"kind": kind, "source": source, "bbox": [106.69, 10.77, 106.71, 10.79], "crs": "OGC:CRS84"})
}

fn with(mut v: Value, extra: Value) -> Value {
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn cases() -> Vec<(&'static str, Value)> {
    let roads = json!({"source": "roads", "distance_m": 200, "filter": {"column": "class", "op": "in", "values": ["motorway", "trunk", "primary"]}});
    vec![
        ("viewport_buildings", base("features", "buildings")),
        ("project_fields_of_places", base("features", "places")),
        ("attribute_filter_eq", with(base("features", "places"), json!({"filter": {"column": "category", "op": "eq", "value": "restaurant"}}))),
        ("attribute_filter_in", with(base("features", "places"), json!({"filter": {"column": "category", "op": "in", "values": ["cafe", "restaurant"]}}))),
        ("attribute_filter_numeric", with(base("features", "places"), json!({"filter": {"column": "confidence", "op": "ge", "value": 0.5}}))),
        ("limit", with(base("features", "places"), json!({"limit": 500}))),
        ("limit_is_capped", with(base("features", "places"), json!({"limit": 5000000}))),
        ("filter_and_limit", with(base("features", "places"), json!({"limit": 10, "filter": {"column": "category", "op": "ne", "value": "x"}}))),
        ("bbox_count", base("count", "buildings")),
        ("count_with_filter", with(base("count", "places"), json!({"filter": {"column": "category", "op": "eq", "value": "cafe"}}))),
        ("count_by_category_sorted", with(base("count_by", "places"), json!({"group_by": "category"}))),
        ("count_by_with_filter", with(base("count_by", "places"), json!({"group_by": "category", "filter": {"column": "confidence", "op": "gt", "value": 0.1}}))),
        ("render_sample", with(base("features", "buildings"), json!({"sample_modulus": 8}))),
        ("render_sample_one_is_no_thinning", with(base("features", "buildings"), json!({"sample_modulus": 1}))),
        ("intersects_viewport", base("intersects_features", "places")),
        ("engine_fields_are_not_part_of_the_plan", with(base("features", "places"), json!({"engine": "duckdb-wasm", "browser": true, "client": {"warm": true}}))),
        ("near_buildings_main_roads", with(base("near", "buildings"), json!({"with": roads}))),
        ("near_count_buildings_main_roads", with(base("near_count", "buildings"), json!({"with": roads}))),
        ("near_with_limit", with(base("near", "buildings"), json!({"limit": 100, "with": {"source": "roads", "distance_m": 50}}))),
        ("join_count_by_ward", with(base("join_count_by", "buildings"), json!({"with": {"source": "admin", "group_by": "name", "filter": {"column": "subtype", "op": "eq", "value": "locality"}}}))),
        // refusals: every one is structured and found before any node is built
        ("err_unknown_dataset", base("features", "nope")),
        ("err_unknown_kind", base("teleport", "places")),
        ("err_crs_mismatch", with(base("features", "places"), json!({"crs": "EPSG:32648"}))),
        ("err_bbox_order", with(base("features", "places"), json!({"bbox": [106.71, 10.77, 106.69, 10.79]}))),
        ("err_filter_op", with(base("features", "places"), json!({"filter": {"column": "category", "op": "like", "value": "x"}}))),
        ("err_filter_empty_in", with(base("features", "places"), json!({"filter": {"column": "category", "op": "in", "values": []}}))),
        ("err_filter_unknown_column", with(base("features", "places"), json!({"filter": {"column": "no_such", "op": "eq", "value": "x"}}))),
        ("err_filter_on_geometry", with(base("features", "places"), json!({"filter": {"column": "geometry", "op": "eq", "value": "x"}}))),
        ("err_filter_non_scalar", with(base("features", "places"), json!({"filter": {"column": "category", "op": "eq", "value": [1, 2]}}))),
        ("err_group_by_missing", base("count_by", "places")),
        ("err_group_by_unknown_column", with(base("count_by", "places"), json!({"group_by": "no_such"}))),
        ("err_sample_on_count", with(base("count", "places"), json!({"sample_modulus": 4}))),
        ("err_sample_not_power_of_two", with(base("features", "buildings"), json!({"sample_modulus": 6}))),
        ("err_limit_zero", with(base("features", "places"), json!({"limit": 0}))),
        ("err_with_on_features", with(base("features", "places"), json!({"with": {"source": "roads", "distance_m": 1}}))),
        ("err_near_without_with", base("near", "buildings")),
        ("err_near_distance_too_large", with(base("near", "buildings"), json!({"with": {"source": "roads", "distance_m": 99999}}))),
        ("err_near_distance_zero", with(base("near", "buildings"), json!({"with": {"source": "roads", "distance_m": 0}}))),
        ("err_join_group_not_a_column", with(base("join_count_by", "buildings"), json!({"with": {"source": "admin", "group_by": "confidence"}}))),
        ("err_unsupported_request_version", with(base("features", "places"), json!({"request_schema_version": 99}))),
    ]
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../geo-plan/fixtures/requests")
}

/// What the fixture records for one request, computed by the native builder + the browser capability list.
fn expected(req: &Value, sources: &Value) -> Value {
    let parsed: Result<PlanRequest, _> = serde_json::from_value(req.clone());
    let srcs: Sources = serde_json::from_value(sources.clone()).unwrap();
    match parsed.map_err(|e| geo_plan::PlanError { code: geo_plan::ErrorCode::InvalidRequest, message: e.to_string() }).and_then(|r| build_plan(&r, &srcs)) {
        Ok(plan) => json!({"plan_json": plan.to_json(), "browser_missing_ops": geo_lowering::browser::missing_ops(&plan)}),
        Err(e) => json!({"error": {"code": e.code, "status": e.http_status(), "legacy": e.legacy_error()}}),
    }
}

#[test]
fn fixtures_match_the_native_builder_and_the_wasm_interface() {
    let generate = std::env::var("GEN_PLANS").is_ok();
    let sources = sources();
    std::fs::create_dir_all(dir()).unwrap();
    let sp = dir().join("_sources.json");
    if generate {
        std::fs::write(&sp, serde_json::to_string_pretty(&sources).unwrap() + "\n").unwrap();
    } else {
        let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&sp).expect("missing _sources.json (GEN_PLANS=1)")).unwrap();
        assert_eq!(on_disk, sources, "_sources.json is out of date");
    }
    let mut seen = Vec::new();
    for (name, req) in cases() {
        seen.push(format!("{name}.json"));
        let exp = expected(&req, &sources);
        let fixture = json!({"name": name, "request": req, "expected": exp});
        let path = dir().join(format!("{name}.json"));
        if generate {
            std::fs::write(&path, serde_json::to_string_pretty(&fixture).unwrap() + "\n").unwrap();
            continue;
        }
        let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing fixture {name} (GEN_PLANS=1)"))).unwrap();
        assert_eq!(on_disk, fixture, "fixture {name} is out of date with the native builder");

        // the exact interface the browser calls: same bytes as the native builder
        let wire = geo_lowering::wasm::build_plan_wire(&json!({"request": req, "sources": sources}).to_string());
        let wire: Value = serde_json::from_str(&wire).unwrap();
        match exp.get("plan_json") {
            Some(plan_json) => {
                assert_eq!(wire["ok"], json!(true), "{name}: {wire}");
                assert_eq!(&wire["plan_json"], plan_json, "{name}: the wasm interface built different bytes");
                assert_eq!(wire["browser_missing_ops"], exp["browser_missing_ops"], "{name}");
            }
            None => {
                assert_eq!(wire["ok"], json!(false), "{name}: {wire}");
                assert_eq!(wire["code"], exp["error"]["code"], "{name}");
                assert_eq!(wire["status"], exp["error"]["status"], "{name}");
            }
        }
    }
    if !generate {
        let mut on_disk: Vec<String> = std::fs::read_dir(dir()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| !n.starts_with('_')).collect();
        on_disk.sort();
        seen.sort();
        assert_eq!(on_disk, seen, "a fixture file has no case (or the other way round)");
    }
}

#[test]
fn the_browser_never_receives_a_plan_it_would_silently_approximate() {
    let sources = sources();
    for (name, req) in cases() {
        let exp = expected(&req, &sources);
        let missing: Vec<String> = exp.get("browser_missing_ops").map(|m| serde_json::from_value(m.clone()).unwrap()).unwrap_or_default();
        match name {
            "near_buildings_main_roads" | "near_count_buildings_main_roads" | "near_with_limit" => {
                assert!(missing.contains(&"transform".to_string()) && missing.contains(&"spatial_semi_join".to_string()), "{name}: {missing:?}")
            }
            "join_count_by_ward" => assert!(missing.contains(&"spatial_join".to_string()), "{name}: {missing:?}"),
            "intersects_viewport" => assert_eq!(missing, vec!["intersects".to_string()], "{name}"),
            n if n.starts_with("err_") => assert!(exp.get("error").is_some(), "{name}"),
            _ => assert!(missing.is_empty(), "{name}: a plain browser workflow must be fully supported, missing {missing:?}"),
        }
    }
}

#[test]
fn the_capability_lists_agree() {
    use geo_ir::OpKind;
    let all = [
        OpKind::Source, OpKind::Filter, OpKind::BboxFilter, OpKind::Intersects, OpKind::Buffer, OpKind::Transform, OpKind::SpatialSemiJoin, OpKind::SpatialJoin,
        OpKind::Aggregate, OpKind::Project, OpKind::Limit, OpKind::Sort,
    ];
    let listed: Vec<&str> = all.iter().map(|o| o.name()).filter(|n| geo_lowering::browser::SUPPORTED_OPS.contains(n)).collect();
    assert_eq!(listed.len(), geo_lowering::browser::SUPPORTED_OPS.len(), "SUPPORTED_OPS names an operation the IR does not have");
}
