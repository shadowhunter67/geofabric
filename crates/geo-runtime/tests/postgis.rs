//! PostGIS as a peer engine under the SAME planner and runtime as DuckDB and DuckDB-Wasm - with a mock database connection (no
//! PostgreSQL was available in this phase). What this proves: the abstraction (descriptor, capability matrix, source locality,
//! calibration policy, authorization, dispatch, error taxonomy) holds for a second engine FAMILY. What it does not prove: that a
//! real server answers the SQL identically - see REPORT-GEOFABRIC-POSTGIS-ENGINE.md.

use arrow_array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use geo_engine_duckdb::{DuckDbConfig, DuckDbEngine};
use geo_engine_postgis::catalog::{PgColType, PostGisCatalog, PostGisSource};
use geo_engine_postgis::{PgConnector, PgError, PgErrorKind, PgQuery, PostGisEngine, ProbeInputs};
use geo_executor::{CancelToken, EngineCalibration, ExecutionContext};
use geo_ir::*;
use geo_planner::FailureClass;
use geo_runtime::{Dispatch, Runtime, RuntimeError};
use geo_types::*;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn point(x: f64, y: f64) -> Vec<u8> {
    let mut o = vec![1u8, 1, 0, 0, 0];
    o.extend(x.to_le_bytes());
    o.extend(y.to_le_bytes());
    o
}

/// A small GeoParquet file (the LOGICAL source) in its own directory; returns (directory, absolute uri).
fn fixture(tag: &str) -> (PathBuf, String) {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("geo-runtime-postgis-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path = dir.join("places.parquet");
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false), Field::new("class", DataType::Utf8, true), Field::new("geometry", DataType::Binary, false)]));
    let n = 200i64;
    let ids: ArrayRef = Arc::new(Int64Array::from((0..n).collect::<Vec<_>>()));
    let cls: ArrayRef = Arc::new(StringArray::from((0..n).map(|i| Some(if i % 2 == 0 { "school" } else { "clinic" })).collect::<Vec<_>>()));
    let geoms: Vec<Vec<u8>> = (0..n).map(|i| point(106.0 + (i % 20) as f64 * 0.001, 10.7 + (i / 20) as f64 * 0.001)).collect();
    let refs: Vec<&[u8]> = geoms.iter().map(Vec::as_slice).collect();
    let g: ArrayRef = Arc::new(BinaryArray::from(refs));
    let geo = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"]}}}"#;
    let props = WriterProperties::builder().set_key_value_metadata(Some(vec![KeyValue::new("geo".into(), geo.to_string())])).build();
    let mut w = ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), schema.clone(), Some(props)).unwrap();
    w.write(&RecordBatch::try_new(schema, vec![ids, cls, g]).unwrap()).unwrap();
    w.close().unwrap();
    let uri = path.to_string_lossy().to_string();
    (dir, uri)
}

fn resident(uri: &str) -> PostGisCatalog {
    PostGisCatalog::new(vec![PostGisSource {
        logical_uri: uri.into(),
        schema: "geofabric".into(),
        relation: "places".into(),
        geometry_column: "geometry".into(),
        crs: "OGC:CRS84".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("class", PgColType::Text)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(200),
        spatially_indexed: true,
        coverage: None,
        covering: None,
    }])
    .unwrap()
}

#[derive(Default)]
struct Mock {
    seen: Mutex<Vec<String>>,
    fail: Mutex<Option<PgError>>,
}

impl PgConnector for Mock {
    fn query(&self, q: &PgQuery, _: &CancelToken) -> Result<Vec<RecordBatch>, PgError> {
        self.seen.lock().unwrap().push(q.sql.clone());
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
        Ok(vec![RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef]).unwrap()])
    }
    fn probe(&self) -> Result<ProbeInputs, PgError> {
        Err(PgError::new(PgErrorKind::Connection, "no server"))
    }
}

/// PostGIS registered FIRST, so that when two engines have an equal score the tie-break (registration order) favours it:
/// the only way a policy, not luck, can keep DuckDB.
fn runtime(catalog: PostGisCatalog, mock: Arc<Mock>) -> Runtime {
    let mut r = Runtime::empty();
    r.register_postgis(PostGisEngine::new(catalog, mock));
    r.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
    r
}

fn plan_for(rt: &Runtime, ctx: &ExecutionContext, uri: &str, hint: Option<&str>) -> Plan {
    let src = rt.open_source(uri, ctx).unwrap();
    let f = Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: BoundingBox::new(105.99, 10.69, 106.03, 10.72, Crs::OgcCrs84).unwrap() };
    let mut p = Plan::new(Node::Limit { input: Box::new(f), n: 50 });
    p.hints.preferred_engine = hint.map(str::to_string);
    p
}

fn candidate<'a>(p: &'a geo_planner::PhysicalPlan, id: &str) -> &'a geo_planner::Candidate {
    p.candidates.iter().find(|c| c.engine.0 == id).unwrap_or_else(|| panic!("no candidate {id}: {:?}", p.candidates.iter().map(|c| c.engine.to_string()).collect::<Vec<_>>()))
}

#[test]
fn the_planner_compares_duckdb_and_postgis_and_never_prefers_the_unmeasured_one_on_cost() {
    let (dir, uri) = fixture("compare");
    let rt = runtime(resident(&uri), Arc::new(Mock::default()));
    let ctx = ExecutionContext::new(vec![dir]);
    let p = rt.plan(&plan_for(&rt, &ctx, &uri, None), &ctx).unwrap();
    let pg = candidate(&p, "postgis");
    assert!(pg.feasible, "{:?}", pg.rejected_because);
    assert_eq!(pg.calibration, EngineCalibration::Unmeasured);
    assert!(pg.reasons.iter().any(|r| r.contains("stand-in")), "{:?}", pg.reasons);
    assert_eq!(candidate(&p, "duckdb").calibration, EngineCalibration::Measured);
    // equal stand-in cost + PostGIS registered first => the cost tie goes to PostGIS; policy R5d hands the choice to the measured engine
    assert_eq!(p.engine.0, "duckdb", "decisions: {:?}", p.decisions);
    assert!(p.assessment.policy.iter().any(|m| m.starts_with("R5d")), "{:?}", p.assessment.policy);
    // the unmeasured engine also carries lower confidence than the measured one
    let (cp, cd) = (pg.confidence.as_ref().unwrap().score, candidate(&p, "duckdb").confidence.as_ref().unwrap().score);
    assert!(cp < cd, "postgis confidence {cp} should be below duckdb {cd}");
}

#[test]
fn an_explicit_choice_runs_through_the_same_dispatch_path_and_the_engine_contract() {
    let (dir, uri) = fixture("dispatch");
    let mock = Arc::new(Mock::default());
    let rt = runtime(resident(&uri), mock.clone());
    let ctx = ExecutionContext::new(vec![dir]);
    let plan = plan_for(&rt, &ctx, &uri, Some("postgis"));
    match rt.dispatch(&plan, &ctx).unwrap() {
        Dispatch::Executed(out) => {
            assert_eq!(out.physical.engine.0, "postgis");
            assert_eq!(out.result.batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
            assert_eq!(out.result.metrics.rows_returned.value, Some(3));
        }
        Dispatch::Client { .. } => panic!("postgis is an in-process executor"),
    }
    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("\"geofabric\".\"places\"") && seen[0].contains("&& ST_MakeEnvelope"), "{}", seen[0]);
}

#[test]
fn data_not_resident_in_the_database_excludes_the_engine_before_execution() {
    let (dir, uri) = fixture("absent");
    let other = fixture("other").1; // a different logical source is the one resident
    let mock = Arc::new(Mock::default());
    let rt = runtime(resident(&other), mock.clone());
    let ctx = ExecutionContext::new(vec![dir]);
    let p = rt.plan(&plan_for(&rt, &ctx, &uri, Some("postgis")), &ctx).unwrap();
    let pg = candidate(&p, "postgis");
    assert!(!pg.feasible && pg.rejected_because.join(" ").contains("not resident"), "{:?}", pg.rejected_because);
    assert_eq!(p.engine.0, "duckdb", "the hint is ignored because the engine is infeasible, never forced");
    assert_eq!(mock.seen.lock().unwrap().len(), 0, "the database was never asked");
}

#[test]
fn an_operation_the_engine_does_not_declare_excludes_it_without_asking_the_server() {
    let (dir, uri) = fixture("ops");
    let mock = Arc::new(Mock::default());
    let rt = runtime(resident(&uri), mock.clone());
    let ctx = ExecutionContext::new(vec![dir]);
    let left = rt.open_source(&uri, &ctx).unwrap();
    let right = rt.open_source(&uri, &ctx).unwrap();
    let join = Node::SpatialJoin {
        left: Box::new(left),
        right: Box::new(right),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "r_".into(),
    };
    let p = rt.plan(&Plan::new(Node::Limit { input: Box::new(join), n: 10 }), &ctx);
    let p = p.unwrap();
    let pg = candidate(&p, "postgis");
    assert!(!pg.feasible && pg.rejected_because.join(" ").contains("unsupported operations"), "{:?}", pg.rejected_because);
    assert_eq!(mock.seen.lock().unwrap().len(), 0);
}

#[test]
fn postgres_failures_land_in_the_existing_failure_taxonomy() {
    // a fresh runtime per case: failures feed health, and after a few the planner (rightly) stops asking the engine
    let class_of = |kind: PgErrorKind, msg: &str| {
        let (dir, uri) = fixture("fail");
        let mock = Arc::new(Mock::default());
        *mock.fail.lock().unwrap() = Some(PgError::new(kind, msg));
        let rt = runtime(resident(&uri), mock);
        let ctx = ExecutionContext::new(vec![dir]);
        let plan = plan_for(&rt, &ctx, &uri, Some("postgis"));
        match rt.dispatch(&plan, &ctx) {
            Err(RuntimeError::Execution(e)) => FailureClass::classify(&e.to_string()),
            Err(e) => panic!("unexpected error {e}"),
            Ok(_) => panic!("expected a failure"),
        }
    };
    assert_eq!(class_of(PgErrorKind::Timeout, "canceling statement due to statement timeout"), FailureClass::Timeout);
    assert_eq!(class_of(PgErrorKind::Cancelled, "canceling statement due to user request"), FailureClass::Cancelled);
    assert_eq!(class_of(PgErrorKind::Connection, "connection refused"), FailureClass::Transient);
    assert_eq!(class_of(PgErrorKind::Capability, "function st_dwithin does not exist"), FailureClass::Capability);
    assert_eq!(class_of(PgErrorKind::Relation, "relation does not exist"), FailureClass::Capability);
}

#[test]
fn geofabric_runs_normally_without_postgis() {
    let rt = Runtime::with_duckdb_and_browser();
    assert!(rt.postgis().is_none());
    let (dir, uri) = fixture("none");
    let ctx = ExecutionContext::new(vec![dir]);
    let src = rt.open_source(&uri, &ctx).unwrap();
    let p = rt.plan(&Plan::new(Node::Limit { input: Box::new(src), n: 5 }), &ctx).unwrap();
    assert!(p.candidates.iter().all(|c| c.engine.0 != "postgis"));
    // and an unconfigured environment yields no engine at all
    assert!(geo_engine_postgis::engine_from_config(&geo_engine_postgis::PostGisConfig::default()).unwrap().is_none());
}

#[test]
fn a_configured_but_driverless_build_is_visible_and_explained_not_silent() {
    #[cfg(not(feature = "postgis-live"))]
    {
        let (dir, uri) = fixture("driverless");
        let cfg = geo_engine_postgis::PostGisConfig {
            enabled: true,
            url: Some("postgres://dev:dev@127.0.0.1:5433/geofabric".into()),
            sources: resident(&uri).iter().cloned().collect(),
            ..Default::default()
        };
        let engine = geo_engine_postgis::engine_from_config(&cfg).unwrap().expect("configured");
        let probe = engine.probe();
        assert!(!probe.reachable && probe.failed_checks[0].contains("no PostgreSQL driver"), "{probe:?}");
        let _ = dir;
    }
}

#[test]
fn failures_are_blamed_on_the_engine_that_failed_and_take_it_out_of_planning_with_one_bounded_alternative() {
    use geo_planner::HealthState;
    let (dir, uri) = fixture("health");
    let mock = Arc::new(Mock::default());
    *mock.fail.lock().unwrap() = Some(PgError::new(PgErrorKind::Connection, "connection refused"));
    let rt = runtime(resident(&uri), mock.clone());
    let ctx = ExecutionContext::new(vec![dir]);
    let plan = plan_for(&rt, &ctx, &uri, Some("postgis"));
    for _ in 0..3 {
        assert!(matches!(rt.dispatch(&plan, &ctx), Err(RuntimeError::Execution(_))));
    }
    let snap = rt.state().snapshot();
    assert_eq!(snap.health.get("postgis").map(|h| h.state), Some(HealthState::Unhealthy), "{:?}", snap.health.get("postgis"));
    assert_ne!(snap.health.get("duckdb").map(|h| h.state), Some(HealthState::Unhealthy), "DuckDB must not be blamed for PostGIS");
    // the next plan routes around it: the hint is ignored because the engine is unhealthy, DuckDB runs, and the database is not asked again
    let calls_before = mock.seen.lock().unwrap().len();
    match rt.dispatch(&plan, &ctx).unwrap() {
        Dispatch::Executed(out) => assert_eq!(out.physical.engine.0, "duckdb"),
        Dispatch::Client { .. } => panic!("duckdb is an in-process executor"),
    }
    assert_eq!(mock.seen.lock().unwrap().len(), calls_before);
}
