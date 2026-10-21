use arrow_array::{ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StructArray};
use arrow_schema::{DataType, Field, Fields, Schema};
use geo_source_geoparquet::*;
use geo_types::{BoundingBox, Crs};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::collections::HashMap;
use std::sync::Arc;

/// 4 row groups, each a 1x1 square cluster at x = 0,10,20,30 (y = 0..1).
fn write_fixture(path: &std::path::Path, crs_json: &str) {
    let bbox_fields: Fields = ["xmin", "ymin", "xmax", "ymax"]
        .iter()
        .map(|n| Arc::new(Field::new(*n, DataType::Float64, false)))
        .collect::<Vec<_>>()
        .into();
    let geo = format!(
            r#"{{"version":"1.1.0","primary_column":"geometry","columns":{{"geometry":{{"encoding":"WKB","geometry_types":["Polygon"],{crs_json}"covering":{{"bbox":{{"xmin":["bbox","xmin"],"ymin":["bbox","ymin"],"xmax":["bbox","xmax"],"ymax":["bbox","ymax"]}}}}}}}}}}"#
    );
    // test hook: `"__no_covering":true` in crs_json hides the covering declaration
    let geo = if crs_json.contains("__no_covering") { geo.replace("\"covering\"", "\"ignored_covering\"") } else { geo };
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("height", DataType::Float64, false),
            Field::new("geometry", DataType::Binary, false),
            Field::new("bbox", DataType::Struct(bbox_fields.clone()), false),
        ],
        HashMap::new(),
    ));
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(10))
        .set_key_value_metadata(Some(vec![parquet::file::metadata::KeyValue::new("geo".to_string(), geo)]))
        .build();
    let mut w = ArrowWriter::try_new(std::fs::File::create(path).unwrap(), schema.clone(), Some(props)).unwrap();
    for g in 0..4 {
        let x0 = (g * 10) as f64;
        let ids: Vec<i64> = (0..10).map(|i| (g * 10 + i) as i64).collect();
        let xs: Vec<f64> = (0..10).map(|i| x0 + i as f64 * 0.1).collect();
        let ys: Vec<f64> = (0..10).map(|i| i as f64 * 0.1).collect();
        let geoms: Vec<&[u8]> = vec![b"x"; 10];
        let col = |v: Vec<f64>| Arc::new(Float64Array::from(v)) as ArrayRef;
        let st = StructArray::new(
            bbox_fields.clone(),
            vec![
                col(xs.clone()),
                col(ys.clone()),
                col(xs.iter().map(|x| x + 0.05).collect()),
                col(ys.iter().map(|y| y + 0.05).collect()),
            ],
            None,
        );
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(ids)),
                col(vec![1.0; 10]),
                Arc::new(BinaryArray::from(geoms)),
                Arc::new(st),
            ],
        )
        .unwrap();
        w.write(&batch).unwrap();
        w.flush().unwrap();
    }
    w.close().unwrap();
}

fn tmp(name: &str) -> std::path::PathBuf {
    // canonical temp dir: on Windows runners the temp path is an 8.3 short name (RUNNER~1), which the guard's lexical check rightly refuses
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join("geofabric-tests");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn reads_metadata_crs_and_row_groups() {
    let p = tmp("fx_epsg.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let i = s.info();
    assert_eq!(i.crs, Crs::Epsg(4326));
    assert_eq!(i.geometry_column, "geometry");
    assert_eq!(i.row_count, 40);
    assert_eq!(i.row_groups.len(), 4);
    assert!(i.covering.is_some());
    let b = i.row_groups[2].bbox.unwrap();
    assert!(b[0] >= 20.0 && b[2] < 21.0, "{b:?}");
    assert!(matches!(s.to_ir_source(), geo_ir::Node::Source { .. }));
}

#[test]
fn bbox_prunes_row_groups_and_is_conservative() {
    let p = tmp("fx_prune.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let q = BoundingBox::new(19.0, -1.0, 21.0, 2.0, Crs::Epsg(4326)).unwrap();
    let r = s.prune(&q).unwrap();
    assert_eq!(r.kept, vec![2]);
    assert_eq!(r.skipped, 3);
    assert_eq!(r.method, PruneMethod::CoveringStats);
    assert!(r.kept_bytes < r.total_bytes);
    // everything
    let all = BoundingBox::new(-100.0, -100.0, 100.0, 100.0, Crs::Epsg(4326)).unwrap();
    assert_eq!(s.prune(&all).unwrap().kept.len(), 4);
    // nothing
    let none = BoundingBox::new(50.0, 50.0, 60.0, 60.0, Crs::Epsg(4326)).unwrap();
    assert!(s.prune(&none).unwrap().kept.is_empty());
}

#[test]
fn crs_missing_key_is_ogc_crs84_not_epsg4326() {
    let p = tmp("fx_missing_crs.parquet");
    write_fixture(&p, ""); // no "crs" key at all
    let s = GeoParquetSource::open(&p).unwrap();
    assert_eq!(s.info().crs, Crs::OgcCrs84);
    assert_ne!(s.info().crs, Crs::Epsg(4326), "must not silently become EPSG:4326");
    // the IR source carries the same CRS
    if let geo_ir::Node::Source { schema, .. } = s.to_ir_source() {
        assert_eq!(schema.geometry_columns[0].crs, Crs::OgcCrs84);
    }
    // a query in EPSG:4326 does not match CRS84 implicitly ...
    let q4326 = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap();
    assert!(matches!(s.prune(&q4326), Err(SourceError::CrsMismatch { .. })));
    // ... but a CRS84 query prunes fine
    let q84 = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::OgcCrs84).unwrap();
    assert_eq!(s.prune(&q84).unwrap().kept, vec![0]);
}

#[test]
fn crs_null_is_undefined_and_blocks_spatial_pruning() {
    let p = tmp("fx_null_crs.parquet");
    write_fixture(&p, r#""crs":null,"#);
    let s = GeoParquetSource::open(&p).unwrap();
    assert_eq!(s.info().crs, Crs::Undefined);
    assert!(!s.info().crs.is_known());
    let q = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap();
    assert!(matches!(s.prune(&q), Err(SourceError::UnknownCrs(_))));
}

#[test]
fn crs_explicit_objects_are_parsed() {
    let p = tmp("fx_explicit_epsg.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":3857}},"#);
    assert_eq!(GeoParquetSource::open(&p).unwrap().info().crs, Crs::Epsg(3857));
    let p = tmp("fx_explicit_crs84.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"OGC","code":"CRS84"}},"#);
    assert_eq!(GeoParquetSource::open(&p).unwrap().info().crs, Crs::OgcCrs84);
    let p = tmp("fx_explicit_other.parquet");
    write_fixture(&p, r#""crs":{"type":"ProjectedCRS","name":"custom"},"#);
    assert!(matches!(GeoParquetSource::open(&p).unwrap().info().crs, Crs::Unknown(_)));
    let p = tmp("fx_crs_string.parquet");
    write_fixture(&p, r#""crs":"EPSG:4326","#);
    assert_eq!(GeoParquetSource::open(&p).unwrap().info().crs, Crs::Epsg(4326));
}

#[test]
fn prune_requires_matching_crs() {
    let p = tmp("fx_crs.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let q = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(3857)).unwrap();
    assert!(matches!(s.prune(&q), Err(SourceError::CrsMismatch { .. })));
}

#[test]
fn projection_and_row_group_selection_are_arrow_native() {
    let p = tmp("fx_read.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let cols = vec!["id".to_string()];
    let batches: Vec<_> = s.read(Some(&cols), Some(&[1, 3]), 1024).unwrap().map(|b| b.unwrap()).collect();
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 20);
    assert_eq!(batches[0].num_columns(), 1);
    assert!(s.estimate_bytes(Some(&cols), &[1]) < s.estimate_bytes(None, &[1]));
    assert!(matches!(s.read(Some(&["nope".to_string()]), None, 10), Err(SourceError::UnknownColumn(_))));
}

#[test]
fn rejects_non_geoparquet() {
    // plain parquet without 'geo' metadata
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, false)]));
    let p = tmp("plain.parquet");
    let mut w = ArrowWriter::try_new(std::fs::File::create(&p).unwrap(), schema.clone(), None).unwrap();
    w.write(&RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![1]))]).unwrap()).unwrap();
    w.close().unwrap();
    assert!(matches!(GeoParquetSource::open(&p), Err(SourceError::NotGeoParquet(_))));
}

#[test]
fn benchmark_dataset_pruning_if_present() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data");
    let (sp, sh) = (base.join("clustered_10mb_spatial.parquet"), base.join("clustered_10mb_shuffled.parquet"));
    if !sp.exists() || !sh.exists() {
        eprintln!("benchmark data absent; skipping");
        return;
    }
    let q = BoundingBox::new(104.0, 12.0, 105.0, 13.0, Crs::Epsg(4326)).unwrap();
    let a = GeoParquetSource::open(&sp).unwrap().prune(&q).unwrap();
    let b = GeoParquetSource::open(&sh).unwrap().prune(&q).unwrap();
    // spatial ordering must prune strictly more than shuffled (shuffled ~ keeps nearly everything)
    assert!(a.skipped > b.skipped, "spatial skipped {} vs shuffled {}", a.skipped, b.skipped);
}

// ---------- PlanGraph scan analysis: shared sources, union over access paths ----------

fn shared_source_join(src: geo_ir::Node, a: BoundingBox, b: Option<BoundingBox>) -> geo_ir::Plan {
    use geo_ir::*;
    let left = Node::BboxFilter { input: Box::new(src.clone()), geometry_column: "geometry".into(), bbox: a };
    let right = match b {
        Some(b) => Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: b },
        None => src,
    };
    Plan::new(Node::SpatialJoin {
        left: Box::new(left),
        right: Box::new(right),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "r_".into(),
    })
}

#[test]
fn shared_source_is_analysed_once_with_union_of_access_paths() {
    let p = tmp("fx_shared.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let guard = geo_types::RootedAccessGuard::new([p.parent().unwrap().to_path_buf()]);
    let bb = |x0: f64, x1: f64| BoundingBox::new(x0, -1.0, x1, 2.0, Crs::Epsg(4326)).unwrap();

    // path A reads row group 0 (x ~ 0), path B reads row group 3 (x ~ 30): union = {0, 3}
    let plan = shared_source_join(s.to_ir_source(), bb(-1.0, 1.5), Some(bb(29.0, 31.0)));
    assert_eq!(plan.nodes.values().filter(|n| n.kind() == geo_ir::OpKind::Source).count(), 1, "source is shared");
    let scans = analyze_plan(&plan, &guard).unwrap();
    assert_eq!(scans.len(), 1, "one scan for the one shared source");
    assert_eq!(scans[0].access_paths, 2);
    assert_eq!(scans[0].prune.kept, vec![0, 3]);
    assert_eq!(scans[0].prune.skipped, 2);

    // a path with no spatial constraint needs every row group
    let plan = shared_source_join(s.to_ir_source(), bb(-1.0, 1.5), None);
    let scans = analyze_plan(&plan, &guard).unwrap();
    assert_eq!(scans[0].prune.kept, vec![0, 1, 2, 3]);
    assert!(scans[0].has_covering);
}

#[test]
fn unauthorized_shared_source_is_denied_before_any_read() {
    let p = tmp("fx_shared_denied.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let other_root = std::env::temp_dir().join(format!("gf-other-root-{}", std::process::id()));
    std::fs::create_dir_all(&other_root).unwrap();
    let guard = geo_types::RootedAccessGuard::new([other_root]);
    let plan = shared_source_join(s.to_ir_source(), BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap(), None);
    assert!(matches!(analyze_plan(&plan, &guard), Err(SourceError::AccessDenied)));
}

// ---------- covering: declared vs inferred (real-data finding: Overture subset has no `covering` key) ----------

#[test]
fn covering_is_declared_when_present_and_inferred_by_convention_when_not() {
    let p = tmp("fx_cov_declared.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let c = GeoParquetSource::open(&p).unwrap().info().covering.clone().unwrap();
    assert!(c.declared);

    let p = tmp("fx_cov_inferred.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"__no_covering":true,"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let c = s.info().covering.clone().expect("a struct column `bbox` is recognised by convention");
    assert!(!c.declared, "must be flagged as inferred");
    assert_eq!(c.xmin, "bbox.xmin");
    // pruning works through the inferred covering (statistics exist)
    let q = BoundingBox::new(19.0, -1.0, 21.0, 2.0, Crs::Epsg(4326)).unwrap();
    assert_eq!(s.prune(&q).unwrap().kept, vec![2]);
}

fn data_file(name: &str) -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data").join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("SKIP: {name} missing");
        None
    }
}

#[test]
fn estimator_inputs_reflect_layout_and_geometry_type() {
    use geo_source_geoparquet::GeoParquetSource;
    if let (Some(a), Some(b)) = (data_file("clustered_10mb_spatial.parquet"), data_file("clustered_10mb_shuffled.parquet")) {
        let sp = GeoParquetSource::open(&a).unwrap();
        let sh = GeoParquetSource::open(&b).unwrap();
        assert_eq!(sp.info().read_class, geo_source_geoparquet::ReadClass::PlainWkb, "synthetic files are plain WKB");
        assert_eq!(sp.info().geometry_null_fraction, Some(0.0));
        let to_scan = |s: &GeoParquetSource| {
            let plan = geo_ir::Plan::new(s.to_ir_source());
            geo_source_geoparquet::analyze_plan(&plan, &geo_types::RootedAccessGuard::new(vec![s.path().parent().unwrap().to_path_buf()])).unwrap().remove(0)
        };
        let (cs, ch) = (to_scan(&sp).spatial_coherence.unwrap(), to_scan(&sh).spatial_coherence.unwrap());
        assert!(cs > 0.9 && ch < 0.2, "spatial file coherent ({cs}), shuffled not ({ch})");
    }
    if let Some(o) = data_file("overture_buildings_vn_south.parquet") {
        assert_eq!(GeoParquetSource::open(&o).unwrap().info().read_class, geo_source_geoparquet::ReadClass::DuckDbTyped, "the Overture subset was written by DuckDB 1.5");
    }
}

#[test]
fn read_class_from_writer_string() {
    use geo_source_geoparquet::ReadClass::*;
    let c = geo_source_geoparquet::ReadClass::from_created_by;
    assert_eq!(c(Some("DuckDB version v1.5.6 (build 069cc9f9b5)")), DuckDbTyped);
    assert_eq!(c(Some("DuckDB version v1.4.3 (build x)")), PlainWkb);
    assert_eq!(c(Some("DuckDB version v2.0.0")), DuckDbTyped);
    assert_eq!(c(Some("parquet-cpp-arrow version 24.0.0")), PlainWkb);
    assert_eq!(c(None), PlainWkb);
}
