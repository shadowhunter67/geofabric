//! `PostGisEngine` behaviour with a mock connector (no database): the order of events, limits, cancellation, timeout, error
//! mapping, metrics provenance, probe. What these tests cannot show - that a real PostgreSQL accepts and answers the SQL - is
//! listed in the report as NOT verified.

use arrow_array::{ArrayRef, BinaryArray, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field as AField, Schema};
use geo_engine_postgis::catalog::{PgColType, PostGisCatalog, PostGisSource};
use geo_engine_postgis::{Param, PgConnector, PgError, PgErrorKind, PgQuery, PostGisEngine, ProbeInputs};
use geo_executor::{CancelToken, EngineDescriptor, EngineError, ExecutionContext, ExecutionEngine, MetricSource};
use geo_ir::{Expr, CompareOp, Literal, Node, Plan, SourceFormat};
use geo_types::{Crs, Field, FieldType, GeometryColumn, GeometryType, SpatialSchema};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    calls: Mutex<Vec<(String, Vec<Param>)>>,
    reply: Mutex<Option<Result<Vec<RecordBatch>, PgError>>>,
    probe: Mutex<Option<Result<ProbeInputs, PgError>>>,
}

impl Mock {
    fn replying(r: Result<Vec<RecordBatch>, PgError>) -> Arc<Mock> {
        let m = Mock::default();
        *m.reply.lock().unwrap() = Some(r);
        Arc::new(m)
    }
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl PgConnector for Mock {
    fn query(&self, q: &PgQuery, _cancel: &CancelToken) -> Result<Vec<RecordBatch>, PgError> {
        self.calls.lock().unwrap().push((q.sql.clone(), q.params.clone()));
        self.reply.lock().unwrap().clone().unwrap_or_else(|| Ok(vec![]))
    }
    fn probe(&self) -> Result<ProbeInputs, PgError> {
        self.probe.lock().unwrap().clone().unwrap_or_else(|| Err(PgError::new(PgErrorKind::Connection, "connection refused")))
    }
}

fn catalog(uri: &str) -> PostGisCatalog {
    PostGisCatalog::new(vec![PostGisSource {
        logical_uri: uri.into(),
        schema: "geofabric".into(),
        relation: "buildings".into(),
        geometry_column: "geometry".into(),
        crs: "OGC:CRS84".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("class", PgColType::Text)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(500),
        spatially_indexed: true,
        coverage: None,
        covering: None,
    }])
    .unwrap()
}

fn plan(uri: &str) -> Plan {
    let schema = SpatialSchema {
        fields: vec![
            Field { name: "id".into(), data_type: FieldType::Int64 },
            Field { name: "class".into(), data_type: FieldType::Utf8 },
            Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) },
        ],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Polygon, crs: Crs::OgcCrs84 }],
    };
    let src = Node::Source { uri: uri.into(), format: SourceFormat::GeoParquet, schema, statistics: Default::default() };
    Plan::new(Node::Filter { input: Box::new(src), predicate: Expr::Compare { column: "class".into(), op: CompareOp::Eq, value: Literal::Str("school".into()) } })
}

/// A directory the guard allows, holding the logical source file (the guard authorizes real paths).
fn allowed_dir() -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("geo-postgis-test-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("buildings.parquet"), b"x").unwrap();
    d.canonicalize().unwrap()
}

/// (logical uri = the absolute path of a real file under an allowed root, as the guard requires, context allowing it)
fn env() -> (String, ExecutionContext) {
    let d = allowed_dir();
    (d.join("buildings.parquet").to_string_lossy().to_string(), ExecutionContext::new(vec![d]))
}

fn ctx_denying() -> ExecutionContext {
    let other = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("geo-postgis-other-{}", std::process::id()));
    std::fs::create_dir_all(&other).unwrap();
    ExecutionContext::new(vec![other.canonicalize().unwrap()])
}

fn rows(n: usize) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![AField::new("id", DataType::Int64, false), AField::new("geometry", DataType::Binary, true)]));
    let ids: ArrayRef = Arc::new(Int64Array::from((0..n as i64).collect::<Vec<_>>()));
    let g: ArrayRef = Arc::new(BinaryArray::from_iter_values((0..n).map(|_| vec![1u8, 3, 0, 0, 0])));
    RecordBatch::try_new(schema, vec![ids, g]).unwrap()
}

#[test]
fn authorization_comes_first_and_the_database_is_never_touched_without_it() {
    let m = Mock::replying(Ok(vec![rows(3)]));
    let (uri, _) = env();
    let e = PostGisEngine::new(catalog(&uri), m.clone());
    let err = e.execute(&plan(&uri), &ctx_denying()).err().unwrap();
    assert!(matches!(err, EngineError::AccessDenied), "{err:?}");
    assert_eq!(m.call_count(), 0, "no query, no catalog access, before the guard said yes");
}

#[test]
fn a_successful_run_returns_tagged_geometry_and_honest_metrics() {
    let m = Mock::replying(Ok(vec![rows(3)]));
    let (uri, ctx) = env();
    let e = PostGisEngine::new(catalog(&uri), m.clone());
    let r = e.execute(&plan(&uri), &ctx).unwrap();
    assert_eq!(r.batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    // the same GeoArrow description every engine produces
    let f = r.batches[0].schema().field_with_name("geometry").unwrap().clone();
    assert_eq!(f.metadata().get("ARROW:extension:name").map(String::as_str), Some("geoarrow.wkb"));
    assert!(f.metadata().get("ARROW:extension:metadata").unwrap().contains("OGC:CRS84"));
    assert_eq!(r.metrics.rows_returned.value, Some(3));
    assert_eq!(r.metrics.elapsed_ms.source, MetricSource::EngineObserved);
    assert_eq!(r.metrics.rows_scanned_upper_bound.source, MetricSource::SourceEstimated);
    // nothing fabricated: PostgreSQL gives no trustworthy bytes-read / memory number here
    assert_eq!(r.metrics.bytes_read.source, MetricSource::Unknown);
    assert_eq!(r.metrics.bytes_read.value, None);
    assert_eq!(r.metrics.peak_memory_bytes.value, None);
    // the statement the server received: parameterized, the value is a bound parameter
    let calls = m.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].0.contains("school") && calls[0].0.contains("$1::text"), "{}", calls[0].0);
    assert_eq!(calls[0].1[0], Param::Text("school".into()));
}

#[test]
fn cancellation_before_the_query_never_reaches_the_server() {
    let m = Mock::replying(Ok(vec![rows(1)]));
    let (uri, ctx) = env();
    let e = PostGisEngine::new(catalog(&uri), m.clone());
    ctx.cancel.cancel();
    assert!(matches!(e.execute(&plan(&uri), &ctx), Err(EngineError::Cancelled)));
    assert_eq!(m.call_count(), 0);
}

#[test]
fn driver_errors_become_the_engine_error_taxonomy_the_planner_classifies() {
    let run = |kind: PgErrorKind, msg: &str| {
        let (uri, ctx) = env();
        let e = PostGisEngine::new(catalog(&uri), Mock::replying(Err(PgError::new(kind, msg))));
        e.execute(&plan(&uri), &ctx).err().unwrap()
    };
    assert!(matches!(run(PgErrorKind::Timeout, "canceling statement due to statement timeout"), EngineError::Timeout { limit_ms: 30_000 }));
    assert!(matches!(run(PgErrorKind::Cancelled, "canceling statement due to user request"), EngineError::Cancelled));
    let conn = run(PgErrorKind::Connection, "connection refused").to_string();
    assert!(conn.contains("connection failed (transient)"), "{conn}");
    let cap = run(PgErrorKind::Capability, "function st_dwithin does not exist").to_string();
    assert!(cap.contains("capability") && cap.contains("not available"), "{cap}");
    let rel = run(PgErrorKind::Relation, "relation \"geofabric.buildings\" does not exist").to_string();
    assert!(rel.contains("capability") && rel.contains("not available"), "{rel}");
    assert!(run(PgErrorKind::Other, "boom").to_string().contains("postgis error"));
}

#[test]
fn the_row_cap_is_enforced_even_if_the_server_returns_more() {
    let (uri, mut ctx) = env();
    let e = PostGisEngine::new(catalog(&uri), Mock::replying(Ok(vec![rows(5)])));
    ctx.max_rows = 4;
    assert!(matches!(e.execute(&plan(&uri), &ctx), Err(EngineError::LimitExceeded(_))));
}

#[test]
fn the_planner_facing_descriptor_refuses_non_resident_data_before_anything_runs() {
    let (uri, ctx) = env();
    let e = PostGisEngine::new(catalog(&uri), Mock::replying(Ok(vec![])));
    assert!(e.source_residency(&plan(&uri), &ctx).is_ok());
    let schema = SpatialSchema {
        fields: vec![Field { name: "id".into(), data_type: FieldType::Int64 }, Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) }],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Point, crs: Crs::OgcCrs84 }],
    };
    let other = Plan::new(Node::Source { uri: uri.replace("buildings.parquet", "only-a-file.parquet"), format: SourceFormat::GeoParquet, schema, statistics: Default::default() });
    let why = e.source_residency(&other, &ctx).unwrap_err();
    assert!(why.contains("not resident"), "{why}");
    // an engine run is refused too, as unsupported (never a SQL error from the server)
    std::fs::write(std::path::Path::new(&uri).with_file_name("only-a-file.parquet"), b"x").unwrap();
    assert!(matches!(e.execute(&other, &ctx), Err(EngineError::Unsupported(_))));
}

#[test]
fn explain_shows_the_residency_the_dropped_columns_and_the_sql_without_values() {
    let (uri, ctx) = env();
    let e = PostGisEngine::new(catalog(&uri), Mock::replying(Ok(vec![])));
    let steps = e.explain(&plan(&uri), &ctx).unwrap();
    let text = steps.iter().map(|s| format!("{}: {}", s.label, s.detail)).collect::<Vec<_>>().join("\n");
    assert!(text.contains("buildings.parquet") && text.contains("bind parameter") && !text.contains("school"), "{text}");
    let est = e.estimate(&plan(&uri), &ctx).unwrap();
    assert_eq!(est.rows_upper_bound.value, Some(500));
    assert_eq!(est.rows_upper_bound.source, MetricSource::SourceEstimated);
    assert_eq!(est.bytes_upper_bound.source, MetricSource::Unknown);
}

#[test]
fn the_probe_reports_an_unreachable_server_without_failing_the_application() {
    let m = Arc::new(Mock::default());
    let (uri, _) = env();
    let e = PostGisEngine::new(catalog(&uri), m.clone());
    let p = e.probe();
    assert!(!p.reachable && !p.ok && p.failed_checks[0].contains("connection refused"), "{p:?}");
    *m.probe.lock().unwrap() = Some(Ok(ProbeInputs {
        postgres_version: "PostgreSQL 16.9".into(),
        postgis_full_version: "POSTGIS=\"3.6.2 x\" [EXTENSION]".into(),
        functions_found: geo_engine_postgis::probe::REQUIRED_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
    }));
    let p = e.probe();
    assert!(p.ok && p.reachable);
    assert_eq!(e.last_probe().unwrap().postgis_version.as_deref(), Some("3.6"));
}
