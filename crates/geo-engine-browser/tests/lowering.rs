//! Golden fixtures of the browser-dialect lowering (RFC-004). `lower_browser` is the reference; the same fixtures are read
//! by `web/test/lowering.test.ts`, which requires `sqlgen.ts` to produce byte-identical SQL.
//! Regenerate after an intended change:  GEN_LOWERING=1 cargo test -p geo-engine-browser --test lowering

use geo_engine_browser::lower::{lower_browser, BrowserSource};
use geo_ir::*;
use geo_types::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn schema() -> SpatialSchema {
    SpatialSchema {
        fields: vec![
            Field { name: "id".into(), data_type: FieldType::Int64 },
            Field { name: "category".into(), data_type: FieldType::Utf8 },
            Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) },
        ],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Polygon, crs: Crs::Epsg(4326) }],
    }
}

fn src(uri: &str) -> Node {
    Node::Source { uri: uri.into(), format: SourceFormat::GeoParquet, schema: schema(), statistics: Default::default() }
}

fn bbox(input: Node) -> Node {
    Node::BboxFilter { input: Box::new(input), geometry_column: "geometry".into(), bbox: BoundingBox::new(104.0, 12.0, 105.5, 13.25, Crs::Epsg(4326)).unwrap() }
}

fn count() -> Vec<AggExpr> {
    vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }]
}

struct Case {
    name: &'static str,
    plan: Plan,
    covering: Option<[&'static str; 4]>,
    max_rows: u64,
}

const COV: Option<[&str; 4]> = Some(["bbox.xmin", "bbox.ymin", "bbox.xmax", "bbox.ymax"]);

fn cases() -> Vec<Case> {
    let cmp = |op, v| Expr::Compare { column: "category".into(), op, value: v };
    vec![
        Case { name: "count_in_window", plan: Plan::new(Node::Aggregate { input: Box::new(bbox(src("a.parquet"))), group_by: vec![], aggregates: count() }), covering: COV, max_rows: 1000 },
        Case {
            name: "features_limit",
            plan: Plan::new(Node::Limit {
                input: Box::new(Node::Project { input: Box::new(bbox(src("a.parquet"))), columns: vec!["id".into(), "category".into(), "geometry".into(), "bbox".into()] }),
                n: 2000,
            }),
            covering: COV,
            max_rows: 100_000,
        },
        Case {
            name: "count_by_sorted",
            plan: Plan::new(Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate { input: Box::new(bbox(src("a.parquet"))), group_by: vec!["category".into()], aggregates: count() }),
                    keys: vec![SortKey { column: "n".into(), descending: true }, SortKey { column: "category".into(), descending: false }],
                }),
                n: 50,
            }),
            covering: COV,
            max_rows: 1000,
        },
        Case {
            name: "filter_string_with_quote",
            plan: Plan::new(Node::Filter { input: Box::new(src("a.parquet")), predicate: cmp(CompareOp::Eq, Literal::Str("it's \"x\"".into())) }),
            covering: COV,
            max_rows: 10,
        },
        Case {
            name: "filter_boolean_expression",
            plan: Plan::new(Node::Filter {
                input: Box::new(src("a.parquet")),
                predicate: Expr::And {
                    exprs: vec![
                        Expr::Or { exprs: vec![cmp(CompareOp::Ne, Literal::Str("farm".into())), Expr::IsNull { column: "category".into() }] },
                        Expr::Not { expr: Box::new(Expr::In { column: "id".into(), values: vec![Literal::Int(1), Literal::Int(2), Literal::Int(3)] }) },
                        Expr::Compare { column: "id".into(), op: CompareOp::Ge, value: Literal::Float(2.5) },
                        Expr::Compare { column: "id".into(), op: CompareOp::Lt, value: Literal::Float(10.0) },
                    ],
                },
            }),
            covering: COV,
            max_rows: 10,
        },
        Case {
            name: "numeric_aggregates",
            plan: Plan::new(Node::Aggregate {
                input: Box::new(src("a.parquet")),
                group_by: vec!["category".into()],
                aggregates: vec![
                    AggExpr { func: AggFunc::Sum, column: Some("id".into()), alias: "s".into() },
                    AggExpr { func: AggFunc::Min, column: Some("id".into()), alias: "lo".into() },
                    AggExpr { func: AggFunc::Max, column: Some("id".into()), alias: "hi".into() },
                    AggExpr { func: AggFunc::Avg, column: Some("id".into()), alias: "mean".into() },
                    AggExpr { func: AggFunc::Count, column: Some("id".into()), alias: "n".into() },
                ],
            }),
            covering: None,
            max_rows: 10,
        },
        Case {
            name: "features_sampled_for_render",
            plan: Plan::new(Node::Limit {
                input: Box::new(Node::Project {
                    input: Box::new(Node::Filter {
                        input: Box::new(bbox(src("a.parquet"))),
                        predicate: Expr::HashBucket { column: "id".into(), modulus: 4, remainder: 0 },
                    }),
                    columns: vec!["id".into(), "category".into(), "geometry".into(), "bbox".into()],
                }),
                n: 20000,
            }),
            covering: COV,
            max_rows: 100_000,
        },
        Case {
            name: "transform_is_unsupported_in_the_browser",
            plan: Plan::new(Node::Transform {
                input: Box::new(src("a.parquet")),
                geometry_column: "geometry".into(),
                input_crs: Crs::Epsg(4326),
                output_crs: Crs::Epsg(32648),
                output_column: "geometry_metric".into(),
            }),
            covering: COV,
            max_rows: 10,
        },
        Case {
            name: "semi_join_is_unsupported_in_the_browser",
            plan: Plan::new(Node::SpatialSemiJoin {
                left: Box::new(src("a.parquet")),
                right: Box::new(src("a.parquet")),
                left_column: "geometry".into(),
                right_column: "geometry".into(),
                predicate: SpatialPredicate::Intersects,
            }),
            covering: COV,
            max_rows: 10,
        },
        Case { name: "bbox_without_covering_is_unsupported", plan: Plan::new(bbox(src("a.parquet"))), covering: None, max_rows: 10 },
        Case {
            name: "intersects_is_unsupported",
            plan: Plan::new(Node::Intersects {
                input: Box::new(src("a.parquet")),
                geometry_column: "geometry".into(),
                with: GeometryLiteral { wkt: "POINT(104.5 12.5)".into(), crs: Crs::Epsg(4326) },
            }),
            covering: COV,
            max_rows: 10,
        },
    ]
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lowering")
}

#[test]
fn fixtures_match_the_reference_lowering() {
    let generate = std::env::var("GEN_LOWERING").is_ok();
    for c in cases() {
        let mut sources = BTreeMap::new();
        sources.insert("a.parquet".to_string(), BrowserSource { covering: c.covering.map(|a| a.map(String::from)) });
        let result = lower_browser(&c.plan, &sources, c.max_rows);
        let expected = match &result {
            Ok((sql, files)) => serde_json::json!({"sql": sql, "files": files}),
            Err(e) => serde_json::json!({"error_kind": match e { geo_executor::EngineError::Unsupported(_) => "unsupported", _ => "invalid" }}),
        };
        let fixture = serde_json::json!({
            "name": c.name,
            "plan": serde_json::to_value(&c.plan).unwrap(),
            "sources": {"a.parquet": {"covering": c.covering}},
            "max_rows": c.max_rows,
            "expected": expected,
        });
        let path = dir().join(format!("{}.json", c.name));
        if generate {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&fixture).unwrap() + "\n").unwrap();
        } else {
            let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing fixture {} (GEN_LOWERING=1 to create)", path.display()))).unwrap();
            assert_eq!(on_disk, fixture, "fixture {} is out of date with the reference lowering", c.name);
        }
    }
}

#[test]
fn lowering_is_safe_by_construction() {
    // identifiers and strings are quoted; nothing user-controlled is concatenated raw
    let mut sources = BTreeMap::new();
    sources.insert("a.parquet".to_string(), BrowserSource { covering: None });
    let evil = Plan::new(Node::Filter {
        input: Box::new(src("a.parquet")),
        predicate: Expr::Compare { column: "x\"; DROP TABLE t; --".into(), op: CompareOp::Eq, value: Literal::Str("'; DROP TABLE t; --".into()) },
    });
    let (sql, _) = lower_browser(&evil, &sources, 10).unwrap();
    assert!(sql.contains("\"x\"\"; DROP TABLE t; --\" = '''; DROP TABLE t; --'"), "{sql}");
    let nan = Plan::new(Node::Filter { input: Box::new(src("a.parquet")), predicate: Expr::Compare { column: "id".into(), op: CompareOp::Eq, value: Literal::Float(f64::NAN) } });
    assert!(lower_browser(&nan, &sources, 10).is_err());
    assert!(lower_browser(&Plan::new(src("other.parquet")), &sources, 10).is_err(), "an unbound source is refused");
}
