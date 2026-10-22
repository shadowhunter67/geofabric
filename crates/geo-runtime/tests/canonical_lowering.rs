//! RFC-004 conformance: the SAME PlanGraph rendered by the three lowerings (DuckDB server, PostGIS, DuckDB-Wasm browser), checked against the
//! canonical semantics that `geo-lowering` defines once. This is the permanent guard against the drift a live PostGIS run once exposed
//! (`BboxFilter` as geometry-envelope overlap instead of covering-box overlap).
//!
//! Needs the small synthetic file (`python benchmarks/gen_synthetic.py --size 10mb --out benchmarks/data`; CI generates it). No server is
//! started and nothing is executed: this compares what each engine WOULD run.

use geo_engine_browser::lower::{lower_browser, BrowserSource};
use geo_engine_duckdb::DuckDbEngine;
use geo_engine_postgis::catalog::{CoveringCols, PgColType, PostGisCatalog, PostGisSource};
use geo_engine_postgis::{lower, Param};
use geo_executor::ExecutionContext;
use geo_ir::*;
use geo_runtime::Runtime;
use geo_types::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data")
}

struct Fx {
    uri: String,
    src: Node,
    ctx: ExecutionContext,
    duck: DuckDbEngine,
    pg: PostGisCatalog,
    browser: BTreeMap<String, BrowserSource>,
}

fn fx() -> Option<Fx> {
    let p = data_dir().join("clustered_10mb_spatial.parquet");
    if !p.exists() {
        eprintln!("SKIP: {} missing (run benchmarks/gen_synthetic.py)", p.display());
        return None;
    }
    let ctx = ExecutionContext::new(vec![data_dir()]);
    let src = Runtime::with_duckdb().open_source(&p.to_string_lossy(), &ctx).unwrap();
    // every engine keys its sources by the URI the Source node carries (canonicalised by the source reader)
    let uri = match &src {
        Node::Source { uri, .. } => uri.clone(),
        _ => unreachable!("open_source returns a Source node"),
    };
    let pg_src = PostGisSource {
        logical_uri: uri.clone(),
        schema: "geofabric".into(),
        relation: "synthetic".into(),
        geometry_column: "geometry".into(),
        crs: "EPSG:4326".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("category", PgColType::Text), ("height", PgColType::Float)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(1000),
        spatially_indexed: true,
        coverage: None,
        covering: Some(CoveringCols { xmin: "bbox_xmin".into(), ymin: "bbox_ymin".into(), xmax: "bbox_xmax".into(), ymax: "bbox_ymax".into() }),
    };
    let mut browser = BTreeMap::new();
    browser.insert(uri.clone(), BrowserSource { covering: Some(["bbox.xmin".into(), "bbox.ymin".into(), "bbox.xmax".into(), "bbox.ymax".into()]) });
    Some(Fx { uri, src, ctx, duck: DuckDbEngine::new(Default::default()), pg: PostGisCatalog::new(vec![pg_src]).unwrap(), browser })
}

/// `$12::double precision` -> `$::double precision`, so statements with different parameter numbering compare by shape.
fn shape(sql: &str) -> String {
    let mut out = String::new();
    let mut it = sql.chars().peekable();
    while let Some(c) = it.next() {
        out.push(c);
        if c == '$' {
            while it.peek().is_some_and(|d| d.is_ascii_digit()) {
                it.next();
            }
        }
    }
    out
}

struct Rendered {
    duck: Result<String, String>,
    pg: Result<(String, Vec<Param>), String>,
    browser: Result<String, String>,
}

fn render(f: &Fx, node: Node) -> Rendered {
    let plan = Plan::new(node);
    Rendered {
        duck: f.duck.compile_sql(&plan, &f.ctx).map_err(|e| e.to_string()),
        pg: lower(&plan, &f.pg, 1000).map(|l| (shape(&l.sql), l.params)).map_err(|e| e.to_string()),
        browser: lower_browser(&plan, &f.browser, 1000).map(|(sql, _)| sql).map_err(|e| e.to_string()),
    }
}

fn window(f: &Fx) -> Node {
    Node::BboxFilter {
        input: Box::new(f.src.clone()),
        geometry_column: "geometry".into(),
        bbox: BoundingBox::new(106.0, 10.5, 106.25, 10.75, Crs::Epsg(4326)).unwrap(),
    }
}

#[test]
fn bbox_filter_is_the_covering_box_overlap_in_every_engine() {
    let Some(f) = fx() else { return };
    let r = render(&f, window(&f));
    // DuckDB and the browser read the file's covering columns; the canonical text is identical
    let canonical = "\"bbox\".\"xmin\" <= 106.25 AND \"bbox\".\"xmax\" >= 106.0 AND \"bbox\".\"ymin\" <= 10.75 AND \"bbox\".\"ymax\" >= 10.5";
    assert!(r.duck.as_ref().unwrap().contains(canonical), "DuckDB: {:?}", r.duck);
    assert!(r.browser.as_ref().unwrap().contains(canonical), "browser: {:?}", r.browser);
    // PostGIS stores the covering columns (hidden `__cov_*`) and decides with the same four comparisons, bound as parameters
    let (sql, params) = r.pg.as_ref().unwrap();
    assert!(
        sql.contains("\"__cov_xmin\" <= $::double precision AND \"__cov_xmax\" >= $::double precision AND \"__cov_ymin\" <= $::double precision AND \"__cov_ymax\" >= $::double precision"),
        "PostGIS: {sql}"
    );
    for v in [106.0, 10.5, 106.25, 10.75] {
        assert!(params.contains(&Param::Float(v)), "window value {v} must be a bound parameter: {params:?}");
    }
    // and the geometry envelope is NOT what decides (the drift this test exists for)
    assert!(!sql.contains("ST_Envelope(\"geometry\")"), "PostGIS must not decide on the geometry envelope when a covering box exists: {sql}");
}

#[test]
fn sort_is_canonical_nulls_last_both_ways_and_text_is_byte_ordered() {
    let Some(f) = fx() else { return };
    let node = Node::Sort {
        input: Box::new(Node::Project { input: Box::new(window(&f)), columns: vec!["id".into(), "category".into()] }),
        keys: vec![SortKey { column: "height".into(), descending: true }, SortKey { column: "category".into(), descending: false }],
    };
    // `height` is not projected: use the columns the projection keeps
    let node = match node {
        Node::Sort { input, .. } => Node::Sort { input, keys: vec![SortKey { column: "id".into(), descending: true }, SortKey { column: "category".into(), descending: false }] },
        n => n,
    };
    let r = render(&f, node);
    let want_duck = "ORDER BY \"id\" DESC NULLS LAST, \"category\" ASC NULLS LAST";
    assert!(r.duck.as_ref().unwrap().contains(want_duck), "{:?}", r.duck);
    assert!(r.browser.as_ref().unwrap().contains(want_duck), "{:?}", r.browser);
    // PostgreSQL's database collation must not decide: byte order is requested explicitly for the text key only
    let (sql, _) = r.pg.as_ref().unwrap();
    assert!(sql.contains("ORDER BY \"id\" DESC NULLS LAST, \"category\" COLLATE \"C\" ASC NULLS LAST"), "{sql}");
}

#[test]
fn aggregate_limit_and_row_cap_agree() {
    let Some(f) = fx() else { return };
    let node = Node::Limit {
        input: Box::new(Node::Aggregate {
            input: Box::new(window(&f)),
            group_by: vec!["category".into()],
            aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }, AggExpr { func: AggFunc::Max, column: Some("height".into()), alias: "h".into() }],
        }),
        n: 50,
    };
    let r = render(&f, node);
    for sql in [r.duck.as_ref().unwrap(), r.browser.as_ref().unwrap(), &r.pg.as_ref().unwrap().0] {
        assert!(sql.contains("COUNT(*) AS \"n\""), "{sql}");
        assert!(sql.contains("MAX(\"height\") AS \"h\""), "{sql}");
        assert!(sql.contains("GROUP BY \"category\""), "{sql}");
    }
    // one extra row is requested everywhere, so "exactly the cap" is distinguishable from "over the cap"
    assert!(r.duck.as_ref().unwrap().ends_with(&format!("LIMIT {}", f.ctx.max_rows + 1)), "{:?}", r.duck);
    assert!(r.browser.as_ref().unwrap().ends_with("LIMIT 1001"), "{:?}", r.browser);
    assert!(r.pg.as_ref().unwrap().1.contains(&Param::Int(1001)), "{:?}", r.pg);
}

#[test]
fn the_predicate_tree_is_one_tree() {
    let Some(f) = fx() else { return };
    let pred = Expr::And {
        exprs: vec![
            Expr::Compare { column: "height".into(), op: CompareOp::Ge, value: Literal::Float(3.5) },
            Expr::Not { expr: Box::new(Expr::IsNull { column: "category".into() }) },
            Expr::Or { exprs: vec![Expr::In { column: "category".into(), values: vec![Literal::Str("a".into()), Literal::Str("b".into())] }] },
        ],
    };
    let r = render(&f, Node::Filter { input: Box::new(window(&f)), predicate: pred });
    // DuckDB and the browser spell the leaves identically (literals inline)
    let tree = "(\"height\" >= 3.5) AND (NOT (\"category\" IS NULL)) AND ((\"category\" IN ('a', 'b')))";
    assert!(r.duck.as_ref().unwrap().contains(tree), "{:?}", r.duck);
    assert!(r.browser.as_ref().unwrap().contains(tree), "{:?}", r.browser);
    // PostGIS has the same tree with bound, typed leaves
    let (sql, params) = r.pg.as_ref().unwrap();
    assert!(
        sql.contains("(\"height\" >= $::double precision) AND (NOT (\"category\" IS NULL)) AND ((\"category\" IN ($::text, $::text)))"),
        "{sql}"
    );
    assert!(params.contains(&Param::Text("a".into())) && params.contains(&Param::Text("b".into())) && params.contains(&Param::Float(3.5)));
}

#[test]
fn what_every_engine_must_refuse_it_refuses() {
    let Some(f) = fx() else { return };
    let bad: Vec<(&str, Node)> = vec![
        ("empty IN", Node::Filter { input: Box::new(window(&f)), predicate: Expr::In { column: "category".into(), values: vec![] } }),
        ("empty AND", Node::Filter { input: Box::new(window(&f)), predicate: Expr::And { exprs: vec![] } }),
        ("sort without keys", Node::Sort { input: Box::new(window(&f)), keys: vec![] }),
        ("aggregate needing a column", Node::Aggregate { input: Box::new(window(&f)), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Sum, column: None, alias: "s".into() }] }),
    ];
    for (what, node) in bad {
        let r = render(&f, node);
        assert!(r.duck.is_err(), "DuckDB accepted {what}");
        assert!(r.browser.is_err(), "browser accepted {what}");
        assert!(r.pg.is_err(), "PostGIS accepted {what}");
    }
}

fn metric_pair(f: &Fx, distance: Distance) -> Node {
    let m = Crs::Epsg(32648);
    let left = Node::Transform { input: Box::new(window(f)), geometry_column: "geometry".into(), input_crs: Crs::Epsg(4326), output_crs: m.clone(), output_column: "geometry_metric".into() };
    let right = Node::Transform {
        input: Box::new(Node::Project { input: Box::new(window(f)), columns: vec!["geometry".into()] }),
        geometry_column: "geometry".into(),
        input_crs: Crs::Epsg(4326),
        output_crs: m,
        output_column: "geometry".into(),
    };
    Node::SpatialSemiJoin {
        left: Box::new(left),
        right: Box::new(right),
        left_column: "geometry_metric".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::WithinDistance { distance },
    }
}

#[test]
fn semi_join_is_exists_in_duckdb_and_postgis_and_refused_in_the_browser() {
    let Some(f) = fx() else { return };
    let r = render(&f, metric_pair(&f, Distance::new(200.0, DistanceUnit::Meter).unwrap()));
    let exists = "WHERE EXISTS (SELECT 1 FROM";
    assert!(r.duck.as_ref().unwrap().contains(exists), "{:?}", r.duck);
    assert!(r.duck.as_ref().unwrap().contains("ST_DWithin(l.\"geometry_metric\", r.\"geometry\", 200.0)"), "{:?}", r.duck);
    let (sql, params) = r.pg.as_ref().unwrap();
    assert!(sql.contains(exists) && sql.contains("ST_DWithin(l.\"geometry_metric\", r.\"geometry\", $::double precision)"), "{sql}");
    assert!(params.contains(&Param::Float(200.0)), "200 m is a bound 200.0: {params:?}");
    // the same transform in both: explicit input and output CRS, never an implicit cast
    assert!(r.duck.as_ref().unwrap().contains("ST_Transform(\"geometry\", 'EPSG:4326', 'EPSG:32648', always_xy := true)"), "{:?}", r.duck);
    assert!(sql.contains("ST_Transform(ST_SetSRID(\"geometry\", 4326), 32648)"), "{sql}");
    // the browser has no spatial extension: refused with a reason, not approximated
    assert!(r.browser.as_ref().unwrap_err().contains("not supported by the browser engine"), "{:?}", r.browser);
}

#[test]
fn a_distance_in_degrees_is_never_a_distance_in_metres() {
    let Some(f) = fx() else { return };
    let r = render(&f, metric_pair(&f, Distance::new(0.002, DistanceUnit::Degree).unwrap()));
    assert!(r.duck.as_ref().unwrap_err().contains("angular distance"), "{:?}", r.duck);
    assert!(r.pg.as_ref().unwrap_err().contains("angular distance"), "{:?}", r.pg);
}

#[test]
fn render_thinning_is_shared_by_the_duckdb_family_and_refused_by_postgis() {
    let Some(f) = fx() else { return };
    let node = Node::Filter { input: Box::new(window(&f)), predicate: Expr::HashBucket { column: "id".into(), modulus: 8, remainder: 0 } };
    let r = render(&f, node);
    let want = "(md5_number_lower(CAST(\"id\" AS VARCHAR)) % 8) = 0";
    assert!(r.duck.as_ref().unwrap().contains(want), "{:?}", r.duck);
    assert!(r.browser.as_ref().unwrap().contains(want), "{:?}", r.browser);
    assert!(r.pg.as_ref().unwrap_err().contains("hash_bucket"), "{:?}", r.pg);
    // the uri is only used to bind sources
    assert!(!f.uri.is_empty());
}
