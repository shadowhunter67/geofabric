//! Policy `metric-semijoin-policy-v1` (R5e): "the other engine is not validated" must never be read as "this engine is safe".
//!
//! Engines are STUBS whose model verdicts are injected, so these tests pin the planner's rules independently of whatever coefficients ship. The stubs borrow
//! capabilities and source residency from a real PostGisEngine over a tiny fixture file, exactly like the other planner tests do.

use arrow_array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use geo_engine_postgis::catalog::{PgColType, PostGisCatalog, PostGisSource};
use geo_engine_postgis::{PgConnector, PgError, PgErrorKind, PgQuery, PostGisEngine, ProbeInputs};
use geo_executor::{
    Capabilities, CancelToken, CostTerm, EngineCalibration, EngineCostEstimate, EngineDescriptor, EngineId, ExecutionContext, ModelValidation, RiskVerdict, WorkloadFeatures,
};
use geo_ir::*;
use geo_planner::METRIC_SEMIJOIN_POLICY_VERSION;
use geo_runtime::Runtime;
use geo_types::*;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use std::path::PathBuf;
use std::sync::Arc;

struct NoDb;
impl PgConnector for NoDb {
    fn query(&self, _: &PgQuery, _: &CancelToken) -> Result<Vec<RecordBatch>, PgError> {
        Err(PgError::new(PgErrorKind::Connection, "no server"))
    }
    fn probe(&self) -> Result<ProbeInputs, PgError> {
        Err(PgError::new(PgErrorKind::Connection, "no server"))
    }
}

fn point(x: f64, y: f64) -> Vec<u8> {
    let mut o = vec![1u8, 1, 0, 0, 0];
    o.extend(x.to_le_bytes());
    o.extend(y.to_le_bytes());
    o
}

fn fixture(tag: &str) -> (PathBuf, String) {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("geo-uncertainty-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path = dir.join("places.parquet");
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false), Field::new("class", DataType::Utf8, true), Field::new("geometry", DataType::Binary, false)]));
    let n = 50i64;
    let ids: ArrayRef = Arc::new(Int64Array::from((0..n).collect::<Vec<_>>()));
    let cls: ArrayRef = Arc::new(StringArray::from((0..n).map(|_| Some("school")).collect::<Vec<_>>()));
    let geoms: Vec<Vec<u8>> = (0..n).map(|i| point(106.0 + (i % 10) as f64 * 0.001, 10.7 + (i / 10) as f64 * 0.001)).collect();
    let refs: Vec<&[u8]> = geoms.iter().map(Vec::as_slice).collect();
    let g: ArrayRef = Arc::new(BinaryArray::from(refs));
    let geo = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"]}}}"#;
    let props = WriterProperties::builder().set_key_value_metadata(Some(vec![KeyValue::new("geo".into(), geo.to_string())])).build();
    let mut w = ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), schema.clone(), Some(props)).unwrap();
    w.write(&RecordBatch::try_new(schema, vec![ids, cls, g]).unwrap()).unwrap();
    w.close().unwrap();
    (dir, path.to_string_lossy().to_string())
}

fn catalog(uri: &str) -> PostGisCatalog {
    PostGisCatalog::new(vec![PostGisSource {
        logical_uri: uri.into(),
        schema: "geofabric".into(),
        relation: "places".into(),
        geometry_column: "geometry".into(),
        crs: "OGC:CRS84".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("class", PgColType::Text)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(50),
        spatially_indexed: true,
        metric_indexes: vec![],
        coverage: None,
        covering: None,
    }])
    .unwrap()
}

/// A stand-in engine with an injected model verdict.
struct Stub {
    id: &'static str,
    inner: Arc<PostGisEngine>,
    calibration: EngineCalibration,
    time_ms: f64,
    risk: RiskVerdict,
}

impl EngineDescriptor for Stub {
    fn id(&self) -> EngineId {
        EngineId::new(self.id)
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn source_residency(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<(), String> {
        self.inner.source_residency(plan, ctx)
    }
    fn calibration(&self) -> EngineCalibration {
        self.calibration
    }
    fn cost_estimate(&self, _: &Plan, _: &WorkloadFeatures) -> Option<EngineCostEstimate> {
        Some(EngineCostEstimate {
            model_version: format!("stub-{}", self.id),
            workload_class: "semijoin_metric_large".into(),
            validation: ModelValidation::Limited,
            time_ms: self.time_ms,
            terms: vec![CostTerm { name: "startup".into(), ms: self.time_ms }],
            risk: self.risk.clone(),
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

fn risk(level: &str, p: Option<f64>) -> RiskVerdict {
    RiskVerdict { timeout_probability: p, ..RiskVerdict::new(level, format!("test: {level}")) }
}

fn blocked_overridable(level: &str, p: Option<f64>) -> RiskVerdict {
    RiskVerdict { blocked: true, overridable: true, ..risk(level, p) }
}

/// Plan with `duckdb` (measured) and `postgis` (unmeasured: the R5d situation) carrying the given verdicts.
fn plan(duck: RiskVerdict, pg: RiskVerdict, hint: Option<&str>, tag: &str) -> geo_planner::PhysicalPlan {
    let (dir, uri) = fixture(tag);
    let engine = Arc::new(PostGisEngine::new(catalog(&uri), Arc::new(NoDb)));
    let mut rt = Runtime::empty();
    // registered first: on equal cost the tie would go to PostGIS, so only a POLICY can keep or move the choice
    rt.register_client(Box::new(Stub { id: "postgis", inner: engine.clone(), calibration: EngineCalibration::Unmeasured, time_ms: 100.0, risk: pg }));
    rt.register_client(Box::new(Stub { id: "duckdb", inner: engine, calibration: EngineCalibration::Measured, time_ms: 100.0, risk: duck }));
    let ctx = ExecutionContext::new(vec![dir]);
    let src = rt.open_source(&uri, &ctx).unwrap();
    let f = Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: BoundingBox::new(105.99, 10.69, 106.03, 10.72, Crs::OgcCrs84).unwrap() };
    let mut p = Plan::new(Node::Limit { input: Box::new(f), n: 10 });
    p.hints.preferred_engine = hint.map(str::to_string);
    rt.plan(&p, &ctx).unwrap()
}

fn cand<'a>(p: &'a geo_planner::PhysicalPlan, id: &str) -> &'a geo_planner::Candidate {
    p.candidates.iter().find(|c| c.engine.0 == id).unwrap()
}

#[test]
fn r5d_still_prefers_the_measured_engine_when_it_is_not_at_risk() {
    let p = plan(risk("low", Some(0.01)), risk("low", Some(0.01)), None, "r5d");
    assert_eq!(p.engine.0, "duckdb", "{:?}", p.assessment.policy);
    assert!(p.assessment.policy.iter().any(|m| m.starts_with("R5d")) && !p.assessment.policy.iter().any(|m| m.starts_with("R5e")));
    assert!(p.assessment.uncertainty.is_none());
}

#[test]
fn an_unvalidated_postgis_does_not_make_a_timeout_risk_duckdb_safe() {
    // the two prior held-out cases: R5d kept DuckDB, DuckDB then timed out while PostGIS finished
    let p = plan(risk("timeout_risk", Some(0.8)), risk("low", Some(0.02)), None, "r5e");
    assert_eq!(p.engine.0, "postgis", "{:?}", p.assessment.policy);
    let msg = p.assessment.policy.iter().find(|m| m.starts_with("R5e")).expect("R5e fired");
    assert!(msg.contains("duckdb is at timeout risk") && msg.contains("postgis is not"), "{msg}");
    assert!(p.assessment.uncertainty.is_none(), "a safe alternative exists: the planner is not uncertain");
}

#[test]
fn a_cost_based_block_is_lifted_only_when_the_alternative_is_at_timeout_risk() {
    // PostGIS blocked (the >50k-probe rule) but not at timeout risk; DuckDB at timeout risk -> the block is lifted for this reason only
    let p = plan(risk("timeout_risk", Some(0.7)), blocked_overridable("high", Some(0.03)), None, "lift");
    assert_eq!(p.engine.0, "postgis", "{:?}", p.assessment.policy);
    let pg = cand(&p, "postgis");
    assert!(pg.feasible && !pg.deferred_by_risk && pg.rejected_because.iter().all(|r| !r.starts_with("risk policy")), "{:?}", pg.rejected_because);
    assert!(p.assessment.policy.iter().any(|m| m.contains("cost-based block was lifted")), "{:?}", p.assessment.policy);
    // ...and stays in force when DuckDB is fine
    let q = plan(risk("low", Some(0.01)), blocked_overridable("high", Some(0.03)), None, "kept");
    assert_eq!(q.engine.0, "duckdb");
    let pgq = cand(&q, "postgis");
    assert!(!pgq.feasible && pgq.deferred_by_risk, "the candidate is deferred, not erased: {:?}", pgq.rejected_because);
    assert!(!q.assessment.policy.iter().any(|m| m.starts_with("R5e")));
}

#[test]
fn a_hard_block_that_is_not_overridable_is_never_lifted() {
    let hard = RiskVerdict { blocked: true, overridable: false, ..risk("high", None) };
    let p = plan(risk("timeout_risk", Some(0.9)), hard, None, "hard");
    // PostGIS is infeasible for good; DuckDB is the only engine, at timeout risk: chosen, and the planner says it is uncertain
    assert_eq!(p.engine.0, "duckdb");
    assert!(!cand(&p, "postgis").feasible && !cand(&p, "postgis").deferred_by_risk);
    let u = p.assessment.uncertainty.as_ref().expect("uncertain");
    assert_eq!(u.policy_version, METRIC_SEMIJOIN_POLICY_VERSION);
}

#[test]
fn when_every_option_is_at_timeout_risk_the_planner_says_so_and_takes_the_least_likely_to_time_out() {
    let p = plan(risk("timeout_risk", Some(0.9)), risk("timeout_risk", Some(0.4)), None, "both");
    assert_eq!(p.engine.0, "postgis", "{:?}", p.assessment.policy);
    let u = p.assessment.uncertainty.as_ref().expect("uncertain");
    assert!(u.reasons.iter().any(|r| r.contains("no engine is known to be safe")), "{:?}", u.reasons);
    // and the other way round
    let q = plan(risk("timeout_risk", Some(0.3)), risk("timeout_risk", Some(0.8)), None, "both2");
    assert_eq!(q.engine.0, "duckdb");
    assert!(q.assessment.uncertainty.is_some());
}

#[test]
fn an_unknown_risk_is_reported_as_uncertainty_and_never_read_as_low() {
    let p = plan(risk("unknown", None), risk("low", Some(0.01)), None, "unk");
    assert_eq!(p.engine.0, "duckdb", "unknown is not timeout risk: R5d still applies");
    let u = p.assessment.uncertainty.as_ref().expect("the planner admits it does not know");
    assert!(u.reasons.iter().any(|r| r.contains("UNKNOWN") && r.contains("unknown is not low")), "{:?}", u.reasons);
}

#[test]
fn an_explicit_engine_request_bypasses_the_policy_and_is_never_second_guessed() {
    let p = plan(risk("timeout_risk", Some(0.99)), risk("low", Some(0.01)), Some("duckdb"), "hint");
    assert_eq!(p.engine.0, "duckdb");
    assert!(!p.assessment.policy.iter().any(|m| m.starts_with("R5e")));
}
