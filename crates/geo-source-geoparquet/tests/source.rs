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
    let d = std::env::temp_dir().join("geofabric-tests");
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
fn prune_requires_matching_known_crs() {
    let p = tmp("fx_crs.parquet");
    write_fixture(&p, r#""crs":{"id":{"authority":"EPSG","code":4326}},"#);
    let s = GeoParquetSource::open(&p).unwrap();
    let q = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(3857)).unwrap();
    assert!(matches!(s.prune(&q), Err(SourceError::CrsMismatch { .. })));

    let p2 = tmp("fx_nocrs.parquet");
    write_fixture(&p2, "");
    let s2 = GeoParquetSource::open(&p2).unwrap();
    assert!(!s2.info().crs.is_known(), "absent CRS must not default to 4326");
    let q = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap();
    assert!(matches!(s2.prune(&q), Err(SourceError::UnknownCrs(_))));
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
