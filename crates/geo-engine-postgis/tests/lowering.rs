//! PlanGraph -> PostgreSQL lowering, tested without a database: shape, parameterization, injection, refusals, and a real PostgreSQL
//! SQL parser (`sqlparser`, PostgreSQL dialect) over every statement. The parser proves SYNTAX only; semantics against a live
//! server were NOT verified in this phase (no PostgreSQL was available) - see REPORT-GEOFABRIC-POSTGIS-ENGINE.md.

use geo_engine_postgis::catalog::{PgColType, PostGisCatalog, PostGisSource};
use geo_engine_postgis::lower::{lower, LowerError};
use geo_engine_postgis::{Param, PostGisEngine};
use geo_executor::{EngineDescriptor, EngineCalibration};
use geo_ir::{AggExpr, AggFunc, CompareOp, Expr, GeometryLiteral, Literal, Node, Plan, SortKey, SourceFormat, SpatialPredicate};
use geo_types::{BoundingBox, Crs, Distance, DistanceUnit, Field, FieldType, GeometryColumn, GeometryType, SpatialSchema};

fn field(n: &str, t: FieldType) -> Field {
    Field { name: n.into(), data_type: t }
}

fn schema(crs: Crs) -> SpatialSchema {
    SpatialSchema {
        fields: vec![
            field("id", FieldType::Int64),
            field("class", FieldType::Utf8),
            field("height", FieldType::Float64),
            field("geometry", FieldType::Other("geometry".into())),
            field("bbox", FieldType::Other("struct".into())), // the GeoParquet covering column: the relation does not store it
        ],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Polygon, crs }],
    }
}

fn source(uri: &str, crs: Crs) -> Node {
    Node::Source { uri: uri.into(), format: SourceFormat::GeoParquet, schema: schema(crs), statistics: Default::default() }
}

fn rel(uri: &str, relation: &str) -> PostGisSource {
    PostGisSource {
        logical_uri: uri.into(),
        schema: "geofabric".into(),
        relation: relation.into(),
        geometry_column: "geometry".into(),
        crs: "OGC:CRS84".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("class", PgColType::Text), ("height", PgColType::Float)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(1000),
        spatially_indexed: true,
    }
}

fn catalog() -> PostGisCatalog {
    PostGisCatalog::new(vec![rel("buildings.parquet", "buildings"), rel("roads.parquet", "roads")]).unwrap()
}

fn bbox() -> BoundingBox {
    BoundingBox::new(106.0, 10.7, 106.1, 10.8, Crs::OgcCrs84).unwrap()
}

fn windowed(uri: &str) -> Node {
    Node::BboxFilter { input: Box::new(source(uri, Crs::OgcCrs84)), geometry_column: "geometry".into(), bbox: bbox() }
}

/// Every statement the tests produce must be valid PostgreSQL syntax.
fn assert_postgres_syntax(sql: &str) {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;
    Parser::parse_sql(&PostgreSqlDialect {}, sql).unwrap_or_else(|e| panic!("not valid PostgreSQL syntax: {e}\n{sql}"));
}

fn ok(node: Node) -> geo_engine_postgis::Lowered {
    let l = lower(&Plan::new(node), &catalog(), 1000).unwrap_or_else(|e| panic!("{e}"));
    assert_postgres_syntax(&l.sql);
    l
}

#[test]
fn a_viewport_query_is_one_parameterized_statement_with_no_value_in_the_text() {
    let n = Node::Limit {
        input: Box::new(Node::Project {
            input: Box::new(Node::Filter {
                input: Box::new(windowed("buildings.parquet")),
                predicate: Expr::And {
                    exprs: vec![
                        Expr::In { column: "class".into(), values: vec![Literal::Str("primary".into()), Literal::Str("secondary".into())] },
                        Expr::Compare { column: "height".into(), op: CompareOp::Gt, value: Literal::Float(3.5) },
                    ],
                },
            }),
            columns: vec!["id".into(), "class".into(), "geometry".into()],
        }),
        n: 100,
    };
    let l = ok(n);
    assert!(l.sql.contains("\"geometry\" && ST_MakeEnvelope($1::double precision, $2::double precision, $3::double precision, $4::double precision, 4326)"), "{}", l.sql);
    assert!(l.sql.contains("\"class\" IN ($5::text, $6::text)") && l.sql.contains("\"height\" > $7::double precision"), "{}", l.sql);
    assert!(l.sql.contains("LIMIT $8::bigint") && l.sql.ends_with("LIMIT $9::bigint"), "{}", l.sql);
    assert!(l.sql.contains("ST_AsBinary(\"geometry\", 'NDR') AS \"geometry\""), "{}", l.sql);
    for v in ["primary", "secondary", "3.5", "106", "10.7"] {
        assert!(!l.sql.contains(v), "value {v} leaked into the SQL text: {}", l.sql);
    }
    assert_eq!(
        l.params,
        vec![
            Param::Float(106.0), Param::Float(10.7), Param::Float(106.1), Param::Float(10.8),
            Param::Text("primary".into()), Param::Text("secondary".into()), Param::Float(3.5), Param::Int(100), Param::Int(1001),
        ]
    );
    assert_eq!(l.dropped_columns, vec!["bbox".to_string()], "the covering struct is not stored in the relation");
    assert_eq!(l.geometry_outputs, vec![("geometry".to_string(), Crs::OgcCrs84)]);
    assert_eq!(l.sources, vec!["buildings.parquet".to_string()]);
}

#[test]
fn count_by_orders_ties_by_the_group_key_with_duckdbs_null_and_text_order() {
    let agg = Node::Aggregate {
        input: Box::new(windowed("buildings.parquet")),
        group_by: vec!["class".into()],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
    };
    let n = Node::Limit {
        input: Box::new(Node::Sort {
            input: Box::new(agg),
            keys: vec![SortKey { column: "count".into(), descending: true }, SortKey { column: "class".into(), descending: false }],
        }),
        n: 50,
    };
    let l = ok(n);
    assert!(l.sql.contains("ORDER BY \"count\" DESC NULLS LAST, \"class\" COLLATE \"C\" ASC NULLS LAST"), "{}", l.sql);
    assert!(l.sql.contains("COUNT(*) AS \"count\"") && l.sql.contains("GROUP BY \"class\""), "{}", l.sql);
    assert!(l.geometry_outputs.is_empty());
}

fn metric(uri: &str) -> Node {
    Node::Transform {
        input: Box::new(windowed(uri)),
        geometry_column: "geometry".into(),
        input_crs: Crs::OgcCrs84,
        output_crs: Crs::Epsg(32648),
        output_column: "geometry_metric".into(),
    }
}

fn near(distance: Distance, left_crs_metric: bool) -> Node {
    let left = if left_crs_metric { metric("buildings.parquet") } else { windowed("buildings.parquet") };
    let right = if left_crs_metric { metric("roads.parquet") } else { windowed("roads.parquet") };
    let col = if left_crs_metric { "geometry_metric" } else { "geometry" };
    Node::SpatialSemiJoin {
        left: Box::new(left),
        right: Box::new(right),
        left_column: col.into(),
        right_column: col.into(),
        predicate: SpatialPredicate::WithinDistance { distance },
    }
}

#[test]
fn demo_b_is_a_metric_transform_plus_an_exists_semi_join() {
    let l = ok(Node::Project {
        input: Box::new(near(Distance::new(200.0, DistanceUnit::Meter).unwrap(), true)),
        columns: vec!["id".into(), "geometry".into()],
    });
    assert!(l.sql.contains("ST_Transform(ST_SetSRID(\"geometry\", 4326), 32648) AS \"geometry_metric\""), "{}", l.sql);
    assert!(l.sql.contains("WHERE EXISTS (SELECT 1 FROM") && l.sql.contains("ST_DWithin(l.\"geometry_metric\", r.\"geometry_metric\", $"), "{}", l.sql);
    assert!(!l.sql.contains("geography") && !l.sql.contains("DISTINCT") && !l.sql.contains(" JOIN "), "no geography shortcut, no join+distinct: {}", l.sql);
    assert!(l.params.contains(&Param::Float(200.0)), "the distance is a bound parameter in metres: {:?}", l.params);
    assert_eq!(l.sources, vec!["buildings.parquet".to_string(), "roads.parquet".to_string()]);
    // the result is drawn in the ORIGINAL CRS: only "geometry" leaves, as WKB
    assert_eq!(l.geometry_outputs, vec![("geometry".to_string(), Crs::OgcCrs84)]);
}

#[test]
fn metres_are_never_degrees() {
    // a metric predicate on a geographic CRS column is refused (the IR validator refuses it too; the adapter does not rely on that)
    let e = lower(&Plan::new(near(Distance::new(200.0, DistanceUnit::Meter).unwrap(), false)), &catalog(), 10).unwrap_err();
    assert!(matches!(&e, LowerError::Invalid(m) if m.contains("not a local metric CRS")), "{e}");
    let e = lower(&Plan::new(near(Distance::new(0.002, DistanceUnit::Degree).unwrap(), true)), &catalog(), 10).unwrap_err();
    assert!(matches!(&e, LowerError::Invalid(m) if m.contains("angular")), "{e}");
}

#[test]
fn intersects_binds_the_wkt_and_uses_the_srid_of_the_crs() {
    let n = Node::Intersects {
        input: Box::new(source("buildings.parquet", Crs::OgcCrs84)),
        geometry_column: "geometry".into(),
        with: GeometryLiteral { wkt: "POLYGON((106 10.7, 106.1 10.7, 106.1 10.8, 106 10.7))".into(), crs: Crs::OgcCrs84 },
    };
    let l = ok(n);
    assert!(l.sql.contains("ST_Intersects(\"geometry\", ST_GeomFromText($1::text, 4326))"), "{}", l.sql);
    assert!(!l.sql.contains("POLYGON"), "{}", l.sql);
}

#[test]
fn nothing_a_plan_can_say_becomes_sql_text_or_an_identifier() {
    // a hostile string value stays a parameter
    let evil = "x'); DROP TABLE geofabric.buildings; --";
    let l = ok(Node::Filter {
        input: Box::new(source("buildings.parquet", Crs::OgcCrs84)),
        predicate: Expr::Compare { column: "class".into(), op: CompareOp::Eq, value: Literal::Str(evil.into()) },
    });
    assert!(!l.sql.contains("DROP") && !l.sql.contains(evil), "{}", l.sql);
    assert!(l.params.contains(&Param::Text(evil.into())));
    // a hostile identifier is not a column of the catalog: refused, never quoted into the statement
    for node in [
        Node::Aggregate {
            input: Box::new(source("buildings.parquet", Crs::OgcCrs84)),
            group_by: vec!["class\"; DROP TABLE x; --".into()],
            aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
        },
        Node::Project { input: Box::new(source("buildings.parquet", Crs::OgcCrs84)), columns: vec!["id\" FROM pg_shadow --".into()] },
        Node::Sort { input: Box::new(source("buildings.parquet", Crs::OgcCrs84)), keys: vec![SortKey { column: "1; select pg_sleep(9)".into(), descending: false }] },
        Node::Filter {
            input: Box::new(source("buildings.parquet", Crs::OgcCrs84)),
            predicate: Expr::IsNull { column: "a) OR (1=1".into() },
        },
    ] {
        let e = lower(&Plan::new(node), &catalog(), 10).unwrap_err();
        assert!(matches!(e, LowerError::Unsupported(_) | LowerError::Invalid(_)), "{e}");
    }
    // a hostile logical uri is simply not resident
    let e = lower(&Plan::new(source("x\"; drop table y; --.parquet", Crs::OgcCrs84)), &catalog(), 10).unwrap_err();
    assert!(matches!(e, LowerError::NotResident(_)));
}

#[test]
fn what_cannot_be_lowered_faithfully_is_refused_with_a_reason() {
    let buffer = Node::Buffer {
        input: Box::new(source("buildings.parquet", Crs::OgcCrs84)),
        geometry_column: "geometry".into(),
        distance: Distance::new(1.0, DistanceUnit::Degree).unwrap(),
        input_crs: Crs::OgcCrs84,
        output_crs: Crs::OgcCrs84,
        error_tolerance: None,
        output_column: "geometry".into(),
    };
    assert!(matches!(lower(&Plan::new(buffer), &catalog(), 10), Err(LowerError::Unsupported(m)) if m.contains("buffer")));
    let sample = Node::Filter {
        input: Box::new(source("buildings.parquet", Crs::OgcCrs84)),
        predicate: Expr::HashBucket { column: "id".into(), modulus: 4, remainder: 0 },
    };
    assert!(matches!(lower(&Plan::new(sample), &catalog(), 10), Err(LowerError::Unsupported(m)) if m.contains("hash_bucket")));
    // no silent type coercion: a string literal against a numeric column, an integer against text
    for pred in [
        Expr::Compare { column: "height".into(), op: CompareOp::Eq, value: Literal::Str("3".into()) },
        Expr::Compare { column: "class".into(), op: CompareOp::Eq, value: Literal::Int(3) },
    ] {
        let n = Node::Filter { input: Box::new(source("buildings.parquet", Crs::OgcCrs84)), predicate: pred };
        assert!(matches!(lower(&Plan::new(n), &catalog(), 10), Err(LowerError::Unsupported(m)) if m.contains("coercion")));
    }
}

#[test]
fn a_source_must_be_resident_and_in_the_same_crs() {
    let e = lower(&Plan::new(source("elsewhere.parquet", Crs::OgcCrs84)), &catalog(), 10).unwrap_err();
    assert!(matches!(&e, LowerError::NotResident(u) if u == "elsewhere.parquet"));
    // OGC:CRS84 and EPSG:4326 are different CRSs at the semantic layer even though they share SRID 4326
    let e = lower(&Plan::new(source("buildings.parquet", Crs::Epsg(4326))), &catalog(), 10).unwrap_err();
    assert!(matches!(&e, LowerError::Unsupported(m) if m.contains("different CRSs")), "{e}");
}

#[test]
fn the_capability_matrix_matches_what_the_lowering_implements() {
    use geo_ir::OpKind;
    let engine = PostGisEngine::new(catalog(), std::sync::Arc::new(NoConnector));
    let caps = engine.capabilities();
    for implemented in [
        OpKind::Source, OpKind::Filter, OpKind::BboxFilter, OpKind::Intersects, OpKind::Transform,
        OpKind::SpatialSemiJoin, OpKind::Aggregate, OpKind::Project, OpKind::Limit, OpKind::Sort,
    ] {
        assert!(caps.ops.contains(&implemented), "{implemented:?} is lowered, so it must be declared");
    }
    for refused in [OpKind::Buffer, OpKind::SpatialJoin] {
        assert!(!caps.ops.contains(&refused), "{refused:?} is not lowered, so it must not be declared (the planner excludes the engine before execution)");
    }
    assert_eq!(engine.calibration(), EngineCalibration::Unmeasured);
}

struct NoConnector;
impl geo_engine_postgis::PgConnector for NoConnector {
    fn query(&self, _: &geo_engine_postgis::PgQuery, _: &geo_executor::CancelToken) -> Result<Vec<arrow_array::RecordBatch>, geo_engine_postgis::PgError> {
        panic!("no connector call expected")
    }
    fn probe(&self) -> Result<geo_engine_postgis::ProbeInputs, geo_engine_postgis::PgError> {
        panic!("no connector call expected")
    }
}
