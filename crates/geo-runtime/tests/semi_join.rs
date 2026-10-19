//! SpatialSemiJoin and metric distance, on tiny deterministic datasets.
//!
//! Part 1 (planar, projected CRS EPSG:32648, coordinates in metres): each LEFT row appears at most once, whatever the number of
//! matching RIGHT rows; empty right side, null and empty geometries.
//! Part 2 (longitude/latitude): "within 200 m" is a REAL metric distance. The fixture points were placed with a geodesic
//! computation (pyproj `Geod.fwd` on WGS 84, 199 m and 201 m north and east of POINT(106.0 10.8)); a degree-as-metres
//! implementation gets the east-west pair wrong by about 2 %, a metric one gets all four right.

use arrow_array::cast::AsArray;
use arrow_array::types::Int64Type;
use arrow_array::{Array, ArrayRef, BinaryArray, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use geo_executor::ExecutionContext;
use geo_ir::*;
use geo_runtime::Runtime;
use geo_types::*;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend(3u32.to_le_bytes());
    v.extend(1u32.to_le_bytes());
    v.extend(5u32.to_le_bytes());
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] {
        v.extend(x.to_le_bytes());
        v.extend(y.to_le_bytes());
    }
    v
}

fn point(x: f64, y: f64) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend(1u32.to_le_bytes());
    v.extend(x.to_le_bytes());
    v.extend(y.to_le_bytes());
    v
}

/// WKB of an empty polygon (type 3, zero rings).
fn empty_polygon() -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend(3u32.to_le_bytes());
    v.extend(0u32.to_le_bytes());
    v
}

fn write(path: &PathBuf, epsg: u32, ids: Vec<i64>, geoms: Vec<Option<Vec<u8>>>) {
    let geo = format!(
        r#"{{"version":"1.1.0","primary_column":"geometry","columns":{{"geometry":{{"encoding":"WKB","geometry_types":[],"crs":{{"id":{{"authority":"EPSG","code":{epsg}}}}}}}}}}}"#
    );
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false), Field::new("geometry", DataType::Binary, true)]));
    let props = WriterProperties::builder().set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo)])).build();
    let mut w = ArrowWriter::try_new(std::fs::File::create(path).unwrap(), schema.clone(), Some(props)).unwrap();
    let refs: Vec<Option<&[u8]>> = geoms.iter().map(|g| g.as_deref()).collect();
    let batch = RecordBatch::try_new(
        schema,
        vec![Arc::new(Int64Array::from(ids)) as ArrayRef, Arc::new(BinaryArray::from(refs)) as ArrayRef],
    )
    .unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
}

fn dir() -> &'static PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("geofabric-semijoin-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let some = |v: Vec<u8>| Some(v);
        // planar part: LEFT 1..6 / RIGHT with 0, 1 and several matches
        write(
            &d.join("left.parquet"),
            32648,
            vec![1, 2, 3, 4, 5, 6],
            vec![
                some(rect(0., 0., 10., 10.)),     // 1: matches R1 and R2 (two right rows)
                some(rect(100., 0., 110., 10.)),  // 2: matches R3 only (one right row)
                some(rect(500., 500., 510., 510.)), // 3: matches nothing
                None,                             // 4: null geometry
                some(empty_polygon()),            // 5: empty geometry
                some(rect(0., 100., 10., 110.)),  // 6: matches R4 at distance 30 m only (within 50 m, not within 20 m)
            ],
        );
        write(
            &d.join("right.parquet"),
            32648,
            vec![1, 2, 3, 4, 5],
            vec![
                some(point(5., 5.)),               // R1 inside L1
                some(rect(8., 8., 20., 20.)),      // R2 overlaps L1
                some(point(105., 5.)),             // R3 inside L2
                some(point(5., 140.)),             // R4: 30 m above L6
                None,                              // R5: null geometry
            ],
        );
        write(&d.join("right_empty.parquet"), 32648, vec![], vec![]);
        // metric part (lon/lat): center R = POINT(106.0 10.8); left points 199 / 201 m north and east of it (geodesic)
        write(
            &d.join("pts.parquet"),
            4326,
            vec![1, 2, 3, 4],
            vec![
                some(point(106.0, 10.801799060660723)),      // 1: 199 m north
                some(point(106.0, 10.801817141671332)),      // 2: 201 m north
                some(point(106.00181966864291, 10.8)),       // 3: 199 m east
                some(point(106.00183795676998, 10.8)),       // 4: 201 m east
            ],
        );
        write(&d.join("center.parquet"), 4326, vec![1], vec![some(point(106.0, 10.8))]);
        d.canonicalize().unwrap()
    })
}

fn rt() -> &'static Runtime {
    static R: OnceLock<Runtime> = OnceLock::new();
    R.get_or_init(Runtime::with_duckdb)
}

fn ctx() -> ExecutionContext {
    ExecutionContext::new(vec![dir().clone()])
}

fn src(name: &str) -> Node {
    rt().open_source(dir().join(name).to_str().unwrap(), &ctx()).unwrap()
}

fn ids(plan: Node) -> Vec<i64> {
    let out = rt().run(&Plan::new(plan), &ctx()).expect("run");
    let mut v = Vec::new();
    for b in &out.result.batches {
        let a = b.column_by_name("id").unwrap().as_primitive::<Int64Type>().clone();
        for i in 0..b.num_rows() {
            v.push(a.value(i));
        }
    }
    v.sort_unstable();
    v
}

fn semi(left: &str, right: &str, predicate: SpatialPredicate) -> Node {
    Node::Project {
        input: Box::new(Node::SpatialSemiJoin {
            left: Box::new(src(left)),
            right: Box::new(src(right)),
            left_column: "geometry".into(),
            right_column: "geometry".into(),
            predicate,
        }),
        columns: vec!["id".into()],
    }
}

#[test]
fn a_left_row_with_several_right_matches_appears_once() {
    // L1 intersects R1 and R2: a join would return it twice, a semi join once
    assert_eq!(ids(semi("left.parquet", "right.parquet", SpatialPredicate::Intersects)), vec![1, 2]);
}

#[test]
fn zero_one_and_many_matches_null_and_empty_geometries() {
    let got = ids(semi("left.parquet", "right.parquet", SpatialPredicate::Intersects));
    assert!(got.contains(&1), "many matches -> once");
    assert!(got.contains(&2), "one match -> once");
    assert!(!got.contains(&3), "no match -> absent");
    assert!(!got.contains(&4), "null geometry never matches");
    assert!(!got.contains(&5), "empty geometry never matches");
    assert_eq!(got.iter().filter(|i| **i == 1).count(), 1);
}

#[test]
fn an_empty_right_side_matches_nothing() {
    assert!(ids(semi("left.parquet", "right_empty.parquet", SpatialPredicate::Intersects)).is_empty());
}

#[test]
fn within_distance_in_a_projected_crs_uses_metres() {
    let d = |m: f64| SpatialPredicate::WithinDistance { distance: Distance::new(m, DistanceUnit::Meter).unwrap() };
    // L6 is 30 m from R4 (and 1 and 2 are touching/near): within 50 m yes, within 20 m no
    let near50 = ids(semi("left.parquet", "right.parquet", d(50.0)));
    let near20 = ids(semi("left.parquet", "right.parquet", d(20.0)));
    assert!(near50.contains(&6) && !near20.contains(&6), "{near50:?} / {near20:?}");
    // the same distance in kilometres is the same predicate
    let km = SpatialPredicate::WithinDistance { distance: Distance::new(0.05, DistanceUnit::Kilometer).unwrap() };
    assert_eq!(ids(semi("left.parquet", "right.parquet", km)), near50);
}

fn metric_near(distance_m: f64) -> Vec<i64> {
    let metric = Crs::Epsg(32648);
    let left = Node::Transform {
        input: Box::new(src("pts.parquet")),
        geometry_column: "geometry".into(),
        input_crs: Crs::Epsg(4326),
        output_crs: metric.clone(),
        output_column: "geometry_metric".into(),
    };
    let right = Node::Transform {
        input: Box::new(src("center.parquet")),
        geometry_column: "geometry".into(),
        input_crs: Crs::Epsg(4326),
        output_crs: metric,
        output_column: "geometry".into(),
    };
    ids(Node::Project {
        input: Box::new(Node::SpatialSemiJoin {
            left: Box::new(left),
            right: Box::new(right),
            left_column: "geometry_metric".into(),
            right_column: "geometry".into(),
            predicate: SpatialPredicate::WithinDistance { distance: Distance::new(distance_m, DistanceUnit::Meter).unwrap() },
        }),
        columns: vec!["id".into()],
    })
}

#[test]
fn two_hundred_metres_is_two_hundred_metres_in_every_direction() {
    // 199 m north and east are in, 201 m north and east are out (geodesic ground truth; UTM scale error here is 0.025 %)
    assert_eq!(metric_near(200.0), vec![1, 3]);
    // a degree-as-metres implementation (111,320 m per degree both ways) would put 201 m EAST inside: it is 1.9 % short there
    assert_eq!(metric_near(150.0), Vec::<i64>::new());
    assert_eq!(metric_near(250.0), vec![1, 2, 3, 4]);
}

#[test]
fn the_validator_refuses_metric_predicates_in_geographic_or_wrongly_unit_ed_plans() {
    let d = Distance::new(200.0, DistanceUnit::Meter).unwrap();
    // lon/lat columns: not a metric CRS
    let geographic = Plan::new(Node::SpatialSemiJoin {
        left: Box::new(src("pts.parquet")),
        right: Box::new(src("center.parquet")),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::WithinDistance { distance: d },
    });
    let r = validate(&geographic);
    assert!(r.issues.iter().any(|i| i.code == "not_metric_crs"), "{:?}", r.issues);
    // a degree distance is never accepted for a metric predicate, even in a projected CRS
    let angular = Plan::new(Node::SpatialSemiJoin {
        left: Box::new(src("left.parquet")),
        right: Box::new(src("right.parquet")),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::WithinDistance { distance: Distance::new(0.001, DistanceUnit::Degree).unwrap() },
    });
    let r = validate(&angular);
    assert!(r.issues.iter().any(|i| i.code == "angular_distance_predicate"), "{:?}", r.issues);
    // Web Mercator has metre units but is not a metric CRS for distances
    assert!(!Crs::Epsg(3857).is_local_metric());
    assert!(Crs::Epsg(32648).is_local_metric());
}
