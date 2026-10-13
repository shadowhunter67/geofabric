//! Exact-result correctness for spatial join / intersects / bbox without a
//! covering column, on tiny deterministic datasets (no benchmark data needed).
//!
//! Left L (id, polygon):          Right R (id, geometry):
//!   1 [0,0]-[2,2]                   1 POINT(1 1)             -> inside L1
//!   2 [2,0]-[4,2]  (touches L1)     2 POINT(2 1)             -> on the L1/L2 shared edge
//!   3 [10,10]-[12,12]               3 POLYGON [3,1]-[11,11]  -> overlaps L2 and L3
//!   4 [20,20]-[22,22]               4 POINT(50 50)           -> nothing
//!                                   5 POLYGON [4,0]-[5,1]    -> touches L2 at x=4
//! Expected inner pairs (l,r): (1,1) (1,2) (2,2) (2,3) (2,5) (3,3)
//! Expected left join adds:    (4,NULL)

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

fn write(path: &PathBuf, ids: Vec<i64>, geoms: Vec<Vec<u8>>) {
    let geo = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":[],"crs":{"id":{"authority":"EPSG","code":4326}}}}}"#;
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("geometry", DataType::Binary, false),
    ]));
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo.to_string())]))
        .build();
    let mut w = ArrowWriter::try_new(std::fs::File::create(path).unwrap(), schema.clone(), Some(props)).unwrap();
    let refs: Vec<&[u8]> = geoms.iter().map(|g| g.as_slice()).collect();
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
        let d = std::env::temp_dir().join(format!("geofabric-join-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        write(
            &d.join("left.parquet"),
            vec![1, 2, 3, 4],
            vec![rect(0., 0., 2., 2.), rect(2., 0., 4., 2.), rect(10., 10., 12., 12.), rect(20., 20., 22., 22.)],
        );
        write(
            &d.join("right.parquet"),
            vec![1, 2, 3, 4, 5],
            vec![point(1., 1.), point(2., 1.), rect(3., 1., 11., 11.), point(50., 50.), rect(4., 0., 5., 1.)],
        );
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

fn rows(plan: Node, cols: &[&str]) -> Vec<Vec<Option<i64>>> {
    let out = rt().run(&Plan::new(plan), &ctx()).expect("run");
    let mut v = Vec::new();
    for b in &out.result.batches {
        let arrays: Vec<_> = cols.iter().map(|c| b.column_by_name(c).unwrap().as_primitive::<Int64Type>().clone()).collect();
        for i in 0..b.num_rows() {
            v.push(arrays.iter().map(|a| if a.is_null(i) { None } else { Some(a.value(i)) }).collect());
        }
    }
    v
}

fn join(jt: JoinType) -> Node {
    Node::Sort {
        input: Box::new(Node::Project {
            input: Box::new(Node::SpatialJoin {
                left: Box::new(src("left.parquet")),
                right: Box::new(src("right.parquet")),
                left_column: "geometry".into(),
                right_column: "geometry".into(),
                predicate: SpatialPredicate::Intersects,
                join_type: jt,
                right_prefix: "r_".into(),
            }),
            columns: vec!["id".into(), "r_id".into()],
        }),
        keys: vec![SortKey { column: "id".into(), descending: false }, SortKey { column: "r_id".into(), descending: false }],
    }
}

#[test]
fn inner_join_returns_exactly_the_known_pairs() {
    let got = rows(join(JoinType::Inner), &["id", "r_id"]);
    let expect: Vec<Vec<Option<i64>>> = [(1, 1), (1, 2), (2, 2), (2, 3), (2, 5), (3, 3)]
        .iter()
        .map(|(l, r)| vec![Some(*l), Some(*r)])
        .collect();
    assert_eq!(got, expect, "exact pair set (not just a count)");
}

#[test]
fn left_join_keeps_unmatched_left_rows_with_null() {
    let got = rows(join(JoinType::Left), &["id", "r_id"]);
    let expect: Vec<Vec<Option<i64>>> =
        vec![vec![Some(1), Some(1)], vec![Some(1), Some(2)], vec![Some(2), Some(2)], vec![Some(2), Some(3)], vec![Some(2), Some(5)], vec![Some(3), Some(3)], vec![Some(4), None]];
    assert_eq!(got, expect);
}

#[test]
fn join_result_is_deterministic_across_runs() {
    let a = rows(join(JoinType::Inner), &["id", "r_id"]);
    let b = rows(join(JoinType::Inner), &["id", "r_id"]);
    assert_eq!(a, b);
}

#[test]
fn intersects_literal_and_bbox_without_covering_column_are_exact() {
    // POINT(2 1) lies on the shared edge of L1 and L2
    let inter = Node::Sort {
        input: Box::new(Node::Project {
            input: Box::new(Node::Intersects {
                input: Box::new(src("left.parquet")),
                geometry_column: "geometry".into(),
                with: GeometryLiteral { wkt: "POINT(2 1)".into(), crs: Crs::Epsg(4326) },
            }),
            columns: vec!["id".into()],
        }),
        keys: vec![SortKey { column: "id".into(), descending: false }],
    };
    assert_eq!(rows(inter, &["id"]), vec![vec![Some(1)], vec![Some(2)]]);

    // bbox [0,0]-[2.5,2.5] on the right dataset (no bbox covering column => ST_Envelope path)
    let bb = Node::Sort {
        input: Box::new(Node::Project {
            input: Box::new(Node::BboxFilter {
                input: Box::new(src("right.parquet")),
                geometry_column: "geometry".into(),
                bbox: BoundingBox::new(0.0, 0.0, 2.5, 2.5, Crs::Epsg(4326)).unwrap(),
            }),
            columns: vec!["id".into()],
        }),
        keys: vec![SortKey { column: "id".into(), descending: false }],
    };
    assert_eq!(rows(bb, &["id"]), vec![vec![Some(1)], vec![Some(2)]]);
}

// ---------- DAG: one shared source feeding both sides of a join ----------

fn dag_self_join() -> Plan {
    use geo_ir::{GraphBuilder, PlanNode};
    let mut b = GraphBuilder::new();
    let source_node = match src("left.parquet") {
        Node::Source { uri, format, schema, statistics } => PlanNode::Source { uri, format, schema, statistics },
        _ => unreachable!(),
    };
    let s = b.add(source_node); // ONE node, two consumers
    let a = b.add(PlanNode::Filter {
        input: s.clone(),
        predicate: Expr::Compare { column: "id".into(), op: CompareOp::Le, value: Literal::Int(2) },
    });
    let c = b.add(PlanNode::Filter {
        input: s,
        predicate: Expr::Compare { column: "id".into(), op: CompareOp::Ge, value: Literal::Int(2) },
    });
    let j = b.add(PlanNode::SpatialJoin {
        left: a,
        right: c,
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "r_".into(),
    });
    let p = b.add(PlanNode::Project { input: j, columns: vec!["id".into(), "r_id".into()] });
    let root = b.add(PlanNode::Sort {
        input: p,
        keys: vec![SortKey { column: "id".into(), descending: false }, SortKey { column: "r_id".into(), descending: false }],
    });
    b.finish(root)
}

#[test]
fn shared_source_dag_runs_with_exact_result_and_one_cte_for_the_source() {
    let plan = dag_self_join();
    assert_eq!(plan.nodes.values().filter(|n| n.kind() == OpKind::Source).count(), 1);
    assert!(validate(&plan).is_ok());
    // left side {1,2}, right side {2,3,4}: intersecting pairs (1,2) [touch] and (2,2) [self]
    let out = rt().run(&plan, &ctx()).expect("run");
    let mut got = Vec::new();
    for b in &out.result.batches {
        let a = b.column_by_name("id").unwrap().as_primitive::<Int64Type>().clone();
        let r = b.column_by_name("r_id").unwrap().as_primitive::<Int64Type>().clone();
        for i in 0..b.num_rows() {
            got.push((a.value(i), r.value(i)));
        }
    }
    assert_eq!(got, vec![(1, 2), (2, 2)]);
    // the shared source is compiled once and referenced twice
    let sql = geo_engine_duckdb::DuckDbEngine::new(Default::default()).compile_sql(&plan, &ctx()).unwrap();
    assert_eq!(sql.matches("read_parquet(").count(), 1, "{sql}");
    // planner: one scan for the shared source
    let phys = rt().plan(&plan, &ctx()).unwrap();
    assert_eq!(phys.scans.len(), 1);
    assert_eq!(phys.scans[0].access_paths, 1, "both filters are above the join's inputs without spatial constraints => one unconstrained path");
}

#[test]
fn tree_and_dag_forms_of_the_same_plan_give_identical_results() {
    let dag = dag_self_join();
    let tree_plan = Plan::new(dag.to_tree().unwrap());
    assert_eq!(tree_plan.plan_id(), dag.plan_id(), "same logical plan");
    let digest = |p: &Plan| geo_executor::result_digest(&rt().run(p, &ctx()).unwrap().result.batches);
    assert_eq!(digest(&dag), digest(&tree_plan));
}
