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
        metric_indexes: vec![],
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

/// A resident source WITHOUT a spatial index: the model's access-path assumption (an index) does not hold, so no class can be validated for it.
fn resident_without_index(uri: &str) -> PostGisCatalog {
    let mut s = resident_sources(uri);
    s[0].spatially_indexed = false;
    PostGisCatalog::new(s).unwrap()
}

fn resident_sources(uri: &str) -> Vec<PostGisSource> {
    vec![PostGisSource {
        logical_uri: uri.into(),
        schema: "geofabric".into(),
        relation: "places".into(),
        geometry_column: "geometry".into(),
        crs: "OGC:CRS84".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("class", PgColType::Text)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(200),
        spatially_indexed: true,
        metric_indexes: vec![],
        coverage: None,
        covering: None,
    }]
}

#[test]
fn the_postgis_cost_model_reaches_the_planner_with_plan_time_features_and_catalog_index_state() {
    let (dir, uri) = fixture("compare");
    let rt = runtime(resident(&uri), Arc::new(Mock::default()));
    let ctx = ExecutionContext::new(vec![dir]);
    let p = rt.plan(&plan_for(&rt, &ctx, &uri, None), &ctx).unwrap();
    let pg = candidate(&p, "postgis");
    assert!(pg.feasible, "{:?}", pg.rejected_because);
    // the plan-time features the cost model saw: a windowed, limited feature query over an indexed source
    assert_eq!(p.workload.shape, "features");
    assert!(p.workload.has_bbox_filter && !p.workload.has_aggregate && p.workload.semi_join.is_none());
    assert_eq!(p.workload.limit, Some(50));
    // SharedPostGis forwards `cost_estimate` (it once did not, which hid the model from the planner): class from the features, index from the catalog.
    // Which validation level the shipped model has is DATA (cost_validation_v1.json), so it is not asserted here.
    let cm = pg.cost_model.as_ref().expect("the PostGIS cost model reached the planner");
    assert_eq!((cm.workload_class.as_str(), cm.index_state.as_str()), ("indexed_bbox", "available"), "{cm:?}");
    assert!(cm.model_version.starts_with("postgis-cost-v") && !cm.risk.blocked && cm.time_ms >= 1.0);
}

/// A PostGIS stand-in whose model verdict is INJECTED: the planner's policy (class-scoped R5d) is tested independently of whatever validation data ships.
struct Injected {
    inner: Arc<PostGisEngine>,
    validation: geo_executor::ModelValidation,
    time_ms: f64,
}

impl geo_executor::EngineDescriptor for Injected {
    fn id(&self) -> geo_executor::EngineId {
        geo_executor::EngineId::new("postgis")
    }
    fn capabilities(&self) -> geo_executor::Capabilities {
        self.inner.capabilities()
    }
    fn source_residency(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<(), String> {
        self.inner.source_residency(plan, ctx)
    }
    fn calibration(&self) -> EngineCalibration {
        EngineCalibration::Unmeasured
    }
    fn cost_estimate(&self, _: &Plan, _: &geo_executor::WorkloadFeatures) -> Option<geo_executor::EngineCostEstimate> {
        Some(geo_executor::EngineCostEstimate {
            model_version: "test-model".into(),
            workload_class: "indexed_bbox".into(),
            validation: self.validation,
            time_ms: self.time_ms,
            terms: vec![geo_executor::CostTerm { name: "startup".into(), ms: self.time_ms }],
            risk: geo_executor::RiskVerdict { blocked: false, level: "low".into(), reason: String::new() },
            index_state: "available".into(),
            notes: vec![],
        })
    }
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<geo_executor::Estimate, geo_executor::EngineError> {
        self.inner.estimate(plan, ctx)
    }
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<geo_executor::ExplainStep>, geo_executor::EngineError> {
        self.inner.explain(plan, ctx)
    }
}

fn plan_with_injected(validation: geo_executor::ModelValidation, tag: &str) -> geo_planner::PhysicalPlan {
    let (dir, uri) = fixture(tag);
    let engine = Arc::new(PostGisEngine::new(resident(&uri), Arc::new(Mock::default())));
    let mut rt = Runtime::empty();
    rt.register_client(Box::new(Injected { inner: engine, validation, time_ms: 2.0 }));
    rt.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
    let ctx = ExecutionContext::new(vec![dir]);
    rt.plan(&plan_for(&rt, &ctx, &uri, None), &ctx).unwrap()
}

#[test]
fn postgis_is_chosen_on_predicted_cost_only_inside_a_validated_workload_class() {
    use geo_executor::ModelValidation;
    let p = plan_with_injected(ModelValidation::Validated, "validated");
    let pg = candidate(&p, "postgis");
    assert_eq!(pg.calibration, EngineCalibration::Measured, "R5d is scoped by validated class, not blanket");
    assert_eq!(p.engine.0, "postgis", "decisions: {:?}", p.decisions);
    assert!(!p.assessment.policy.iter().any(|m| m.starts_with("R5d")), "{:?}", p.assessment.policy);
    assert!(pg.score_ms.unwrap() < candidate(&p, "duckdb").score_ms.unwrap());
    // the same prediction in a class that is merely Limited or Unvalidated is explained but never chosen on cost
    for level in [ModelValidation::Limited, ModelValidation::Unvalidated] {
        let p = plan_with_injected(level, "notvalidated");
        assert_eq!(candidate(&p, "postgis").calibration, EngineCalibration::Unmeasured, "{level:?}");
        assert_eq!(p.engine.0, "duckdb", "{level:?}: {:?}", p.decisions);
        let msg = p.assessment.policy.iter().find(|m| m.starts_with("R5d")).expect("R5d fired");
        assert!(msg.contains("not validated for automatic selection"), "{msg}");
    }
}

#[test]
fn without_the_index_the_model_cannot_be_validated_and_r5d_keeps_the_measured_engine() {
    let (dir, uri) = fixture("noindex");
    let rt = runtime(resident_without_index(&uri), Arc::new(Mock::default()));
    let ctx = ExecutionContext::new(vec![dir]);
    let p = rt.plan(&plan_for(&rt, &ctx, &uri, None), &ctx).unwrap();
    let pg = candidate(&p, "postgis");
    let cm = pg.cost_model.as_ref().unwrap();
    assert_eq!((cm.index_state.as_str(), cm.validation), ("unavailable", geo_executor::ModelValidation::Unvalidated), "{cm:?}");
    assert_eq!(cm.risk.level, "elevated");
    assert_eq!(pg.calibration, EngineCalibration::Unmeasured);
    assert_eq!(p.engine.0, "duckdb", "even though the stand-in prediction is lower: decisions {:?}", p.decisions);
    let msg = p.assessment.policy.iter().find(|m| m.starts_with("R5d")).expect("R5d fired");
    assert!(msg.contains("not validated for automatic selection") && msg.contains("indexed_bbox"), "{msg}");
    // an explicit request still reaches PostGIS: R5d is about automatic choice, never a prohibition
    let hinted = rt.plan(&plan_for(&rt, &ctx, &uri, Some("postgis")), &ctx).unwrap();
    assert_eq!(hinted.engine.0, "postgis");
}

/// An engine whose cost model reports a HARD risk refusal (the real PostGIS one needs > 50,000 probes, which a 200-row fixture cannot make).
struct RiskRefusing(Arc<PostGisEngine>);

impl geo_executor::EngineDescriptor for RiskRefusing {
    fn id(&self) -> geo_executor::EngineId {
        geo_executor::EngineId::new("pg-risky")
    }
    fn capabilities(&self) -> geo_executor::Capabilities {
        self.0.capabilities()
    }
    fn source_residency(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<(), String> {
        self.0.source_residency(plan, ctx)
    }
    fn calibration(&self) -> EngineCalibration {
        EngineCalibration::Unmeasured
    }
    fn cost_estimate(&self, _: &Plan, _: &geo_executor::WorkloadFeatures) -> Option<geo_executor::EngineCostEstimate> {
        Some(geo_executor::EngineCostEstimate {
            model_version: "test".into(),
            workload_class: "semijoin_metric_large".into(),
            validation: geo_executor::ModelValidation::Validated,
            time_ms: 1.0,
            terms: vec![],
            risk: geo_executor::RiskVerdict { blocked: true, level: "high".into(), reason: "repeated-probe risk (test)".into() },
            index_state: "available".into(),
            notes: vec![],
        })
    }
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<geo_executor::Estimate, geo_executor::EngineError> {
        self.0.estimate(plan, ctx)
    }
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<geo_executor::ExplainStep>, geo_executor::EngineError> {
        self.0.explain(plan, ctx)
    }
}

#[test]
fn a_hard_risk_verdict_refuses_automatic_selection_but_an_explicit_request_overrides_it() {
    let (dir, uri) = fixture("risk");
    let engine = Arc::new(PostGisEngine::new(resident(&uri), Arc::new(Mock::default())));
    let mut rt = Runtime::empty();
    rt.register_client(Box::new(RiskRefusing(engine)));
    rt.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
    let ctx = ExecutionContext::new(vec![dir]);
    let p = rt.plan(&plan_for(&rt, &ctx, &uri, None), &ctx).unwrap();
    let risky = candidate(&p, "pg-risky");
    assert!(!risky.feasible, "a blocked workload is refused, not merely expensive");
    assert!(risky.rejected_because.iter().any(|r| r.contains("risk policy") && r.contains("repeated-probe risk")), "{:?}", risky.rejected_because);
    assert_eq!(p.engine.0, "duckdb");
    // the refusal is about AUTOMATIC selection: asked for explicitly, the engine is allowed and says why
    let h = rt.plan(&plan_for(&rt, &ctx, &uri, Some("pg-risky")), &ctx).unwrap();
    let risky = candidate(&h, "pg-risky");
    assert!(risky.feasible && risky.reasons.iter().any(|r| r.contains("allowed because the engine was requested explicitly")), "{:?} / {:?}", risky.rejected_because, risky.reasons);
    assert_eq!(h.engine.to_string(), "pg-risky");
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
