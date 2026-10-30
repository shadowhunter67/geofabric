//! Vertical slice: GeoParquet -> IR -> validate -> plan -> DuckDB -> Arrow.
//! Needs benchmark data: `python benchmarks/gen_synthetic.py --size 10mb --out benchmarks/data`
//! (tests skip with a message if absent).

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type};
use geo_executor::{EngineError, ExecutionContext};
use geo_ir::*;
use geo_runtime::{Runtime, RuntimeError};
use geo_source_geoparquet::GeoParquetSource;
use geo_types::*;
use std::path::PathBuf;
use std::sync::OnceLock;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data")
}

fn file(name: &str) -> Option<PathBuf> {
    let p = data_dir().join(format!("{name}.parquet"));
    if p.exists() {
        Some(p)
    } else {
        eprintln!("SKIP: {} missing (run benchmarks/gen_synthetic.py)", p.display());
        None
    }
}

fn rt() -> &'static Runtime {
    static R: OnceLock<Runtime> = OnceLock::new();
    R.get_or_init(Runtime::with_duckdb)
}

fn ctx() -> ExecutionContext {
    ExecutionContext::new(vec![data_dir()])
}

fn bbox(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
    BoundingBox::new(x0, y0, x1, y1, Crs::Epsg(4326)).unwrap()
}

fn count_agg(input: Node) -> Node {
    Node::Aggregate {
        input: Box::new(input),
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
    }
}

fn scalar_count(plan: &Plan) -> (i64, geo_executor::ExecutionMetrics) {
    let out = rt().run(plan, &ctx()).expect("run");
    let b = &out.result.batches[0];
    (b.column(0).as_primitive::<Int64Type>().value(0), out.result.metrics)
}

/// Independent oracle: read covering + attribute columns through the Arrow-native
/// source reader and evaluate in plain Rust (no DuckDB).
fn oracle(path: &PathBuf, q: (f64, f64, f64, f64), pred: impl Fn(&str, f64) -> bool) -> i64 {
    let src = GeoParquetSource::open(path).unwrap();
    let cols = vec!["bbox".to_string(), "category".to_string(), "height".to_string()];
    let mut n = 0;
    for b in src.read(Some(&cols), None, 8192).unwrap() {
        let b = b.unwrap();
        let st = b.column_by_name("bbox").unwrap().as_struct();
        let f = |name: &str| st.column_by_name(name).unwrap().as_primitive::<Float64Type>().clone();
        let (xmin, ymin, xmax, ymax) = (f("xmin"), f("ymin"), f("xmax"), f("ymax"));
        let cat = b.column_by_name("category").unwrap().as_string::<i32>().clone();
        let h = b.column_by_name("height").unwrap().as_primitive::<Float64Type>().clone();
        for i in 0..b.num_rows() {
            let hit = xmin.value(i) <= q.2 && xmax.value(i) >= q.0 && ymin.value(i) <= q.3 && ymax.value(i) >= q.1;
            if hit && pred(cat.value(i), h.value(i)) {
                n += 1;
            }
        }
    }
    n
}

fn bbox_plan(src: Node, q: (f64, f64, f64, f64)) -> Node {
    Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: bbox(q.0, q.1, q.2, q.3) }
}

const Q: (f64, f64, f64, f64) = (104.0, 12.0, 105.0, 13.0);

#[test]
fn bbox_count_matches_oracle_and_prunes() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let plan = Plan::new(count_agg(bbox_plan(src, Q)));
    let (n, m) = scalar_count(&plan);
    assert_eq!(n, oracle(&p, Q, |_, _| true));
    assert!(n > 0);
    assert!(m.row_groups_skipped.value.unwrap() > 0, "{m:?}");
    eprintln!("bbox: n={n} metrics={m:?}");
}

#[test]
fn attribute_filter_matches_oracle() {
    let Some(p) = file("uniform_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let f = Node::Filter {
        input: Box::new(bbox_plan(src, Q)),
        predicate: Expr::And {
            exprs: vec![
                Expr::Compare { column: "category".into(), op: CompareOp::Eq, value: Literal::Str("school".into()) },
                Expr::Compare { column: "height".into(), op: CompareOp::Gt, value: Literal::Float(10.0) },
            ],
        },
    };
    let (n, _) = scalar_count(&Plan::new(count_agg(f)));
    assert_eq!(n, oracle(&p, Q, |c, h| c == "school" && h > 10.0));
}

#[test]
fn intersects_with_matching_rectangle_equals_bbox_count() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let plan = Plan::new(count_agg(Node::Intersects {
        input: Box::new(src),
        geometry_column: "geometry".into(),
        with: GeometryLiteral { wkt: "POLYGON((104 12, 105 12, 105 13, 104 13, 104 12))".into(), crs: Crs::Epsg(4326) },
    }));
    let phys = rt().plan(&plan, &ctx()).unwrap();
    assert!(phys.scans[0].prune.skipped > 0, "intersects literal should prune row groups");
    let (n, m) = scalar_count(&plan);
    assert_eq!(n, oracle(&p, Q, |_, _| true));
    assert!(m.row_groups_skipped.value.unwrap() > 0);
}

#[test]
fn aggregate_group_by_matches_oracle_totals() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let agg = Node::Sort {
        input: Box::new(Node::Aggregate {
            input: Box::new(bbox_plan(src, Q)),
            group_by: vec!["category".into()],
            aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "building_count".into() }],
        }),
        keys: vec![SortKey { column: "building_count".into(), descending: true }],
    };
    let out = rt().run(&Plan::new(agg), &ctx()).unwrap();
    let mut total = 0;
    let mut prev = i64::MAX;
    for b in &out.result.batches {
        let c = b.column_by_name("building_count").unwrap().as_primitive::<Int64Type>();
        for i in 0..c.len() {
            assert!(c.value(i) <= prev, "sorted desc");
            prev = c.value(i);
            total += c.value(i);
        }
    }
    assert_eq!(total, oracle(&p, Q, |_, _| true));
    assert!(out.physical.decisions.iter().any(|d| d.rule == "select_engine"));
}

#[test]
fn row_order_does_not_change_results_but_changes_pruning() {
    let (Some(a), Some(b)) = (file("clustered_10mb_spatial"), file("clustered_10mb_shuffled")) else { return };
    let run = |p: &PathBuf| {
        let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
        scalar_count(&Plan::new(count_agg(bbox_plan(src, Q))))
    };
    let (na, ma) = run(&a);
    let (nb, mb) = run(&b);
    assert_eq!(na, nb, "same content, different order => same answer");
    assert!(
        ma.row_groups_skipped.value.unwrap() > mb.row_groups_skipped.value.unwrap(),
        "spatial {:?} vs shuffled {:?}",
        ma.row_groups_skipped.value,
        mb.row_groups_skipped.value
    );
}

// spatial join correctness: see tests/join.rs (exact pair sets on a tiny deterministic dataset)

#[test]
fn result_geometry_carries_crs_metadata() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let plan = Plan::new(Node::Limit {
        input: Box::new(Node::Project { input: Box::new(bbox_plan(src, Q)), columns: vec!["id".into(), "geometry".into()] }),
        n: 5,
    });
    let out = rt().run(&plan, &ctx()).unwrap();
    let schema = out.result.batches[0].schema();
    let g = schema.field_with_name("geometry").unwrap();
    eprintln!("GEOMETRY FIELD: {g:?}");
    assert_eq!(g.metadata().get("ARROW:extension:name").map(String::as_str), Some("geoarrow.wkb"));
    assert!(g.metadata().get("ARROW:extension:metadata").unwrap().contains("4326"));
}

#[test]
fn metric_buffer_on_geographic_data_is_rejected_by_planner() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let plan = Plan::new(Node::Buffer {
        input: Box::new(src),
        geometry_column: "geometry".into(),
        distance: Distance::new(100.0, DistanceUnit::Meter).unwrap(),
        input_crs: Crs::Epsg(4326),
        output_crs: Crs::Epsg(4326),
        error_tolerance: None,
        output_column: "geometry".into(),
    });
    match rt().plan(&plan, &ctx()) {
        Err(RuntimeError::Plan(geo_planner::PlanError::Invalid(r))) => assert!(r.has_code("metric_on_geographic")),
        other => panic!("expected validation failure, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn security_path_and_injection() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    // 1. source outside allowed roots
    let outside = ExecutionContext::new(vec![std::fs::canonicalize(std::env::temp_dir()).unwrap().join("geofabric-nothing-here")]);
    std::fs::create_dir_all(std::fs::canonicalize(std::env::temp_dir()).unwrap().join("geofabric-nothing-here")).unwrap();
    match rt().run(&Plan::new(count_agg(src.clone())), &outside) {
        Err(RuntimeError::AccessDenied) => {}
        other => panic!("expected access denied, got {:?}", other.map(|_| ())),
    }
    // 2. quote in a string literal is data, not SQL
    let f = Node::Filter {
        input: Box::new(src),
        predicate: Expr::Compare { column: "category".into(), op: CompareOp::Eq, value: Literal::Str("x' OR '1'='1".into()) },
    };
    let (n, _) = scalar_count(&Plan::new(count_agg(f)));
    assert_eq!(n, 0);
}

#[test]
fn max_rows_limit_enforced() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    let mut c = ctx();
    c.max_rows = 10;
    match rt().run(&Plan::new(Node::Project { input: Box::new(src), columns: vec!["id".into()] }), &c) {
        // result-policy-v1 refuses a full result above the cap at PLAN time (before any work); an engine that is still reached enforces it after execution
        Err(RuntimeError::Plan(e)) => assert!(e.to_string().contains("result-delivery policy"), "{e}"),
        Err(RuntimeError::Execution(EngineError::LimitExceeded(_))) => {}
        other => panic!("expected limit exceeded, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn unknown_crs_source_cannot_be_bbox_filtered() {
    let Some(p) = file("clustered_10mb_spatial") else { return };
    let mut src = rt().open_source(p.to_str().unwrap(), &ctx()).unwrap();
    if let Node::Source { schema, .. } = &mut src {
        schema.geometry_columns[0].crs = Crs::Unknown("mystery".into());
    }
    let plan = Plan::new(bbox_plan(src, Q));
    assert!(matches!(rt().plan(&plan, &ctx()), Err(RuntimeError::Plan(geo_planner::PlanError::Invalid(_)))));
}
