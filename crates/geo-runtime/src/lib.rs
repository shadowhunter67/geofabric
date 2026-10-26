//! Composition root. Applications talk to this crate (or the HTTP layer on
//! top of it), never to an engine directly.
//!
//! Two kinds of engines are registered:
//! * **executors** ([`ExecutionEngine`]) run plans inside this process (DuckDB);
//! * **client descriptors** ([`EngineDescriptor`] only) describe engines that run
//!   in the user's browser (DuckDB-Wasm). The planner can select them; execution is
//!   then delegated to the client ([`Dispatch::Client`]).

use geo_engine_browser::{BrowserEngineConfig, DuckDbWasmDescriptor};
use geo_engine_duckdb::{DuckDbConfig, DuckDbEngine};
use geo_engine_postgis::PostGisEngine;
use geo_executor::{EngineDescriptor, ExecutionContext, ExecutionEngine, ExecutionResult};
use geo_ir::{Node, Plan, PlanNode};
use geo_planner::{PhysicalPlan, PlanError, PlanInputs, Planner, PlannerConfig, PlannerState};
use geo_source_geoparquet::{GeoParquetSource, SourceError};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Source(SourceError),
    #[error("execution failed: {0}")]
    Execution(#[from] geo_executor::EngineError),
    /// Opaque on purpose (no path, no reason).
    #[error("source not permitted")]
    AccessDenied,
    #[error("the planner selected a client-side engine ({0}); use dispatch()")]
    ClientEngineSelected(String),
}

/// Authorization happens first, for every source of the plan, before any
/// metadata I/O (planning reads Parquet footers).
fn authorize_all(plan: &Plan, ctx: &ExecutionContext) -> Result<(), RuntimeError> {
    for n in plan.nodes.values() {
        if let PlanNode::Source { uri, .. } = n {
            ctx.guard.authorize(uri).map_err(|_| RuntimeError::AccessDenied)?;
        }
    }
    Ok(())
}

pub struct RunOutput {
    pub physical: PhysicalPlan,
    pub result: ExecutionResult,
}

/// Outcome of [`Runtime::dispatch`].
pub enum Dispatch {
    /// Executed here.
    Executed(RunOutput),
    /// The planner chose a client-side engine: the caller must hand the plan to it.
    Client { physical: PhysicalPlan },
}

pub struct Runtime {
    engines: Vec<Box<dyn ExecutionEngine>>,
    clients: Vec<Box<dyn EngineDescriptor>>,
    planner: Planner,
    /// The PostGIS engine, when configured (optional: absent in the normal case). Shared with the registered executor.
    postgis: Option<Arc<PostGisEngine>>,
    /// What the planner knows about the world: probes, health, telemetry, network (ADR-009/010).
    state: Arc<PlannerState>,
}

impl From<SourceError> for RuntimeError {
    fn from(e: SourceError) -> Self {
        match e {
            SourceError::AccessDenied => RuntimeError::AccessDenied,
            other => RuntimeError::Source(other),
        }
    }
}

impl Runtime {
    pub fn empty() -> Self {
        Runtime { engines: Vec::new(), clients: Vec::new(), planner: Planner::default(), postgis: None, state: Arc::new(PlannerState::new()) }
    }

    /// Server-only runtime: DuckDB.
    pub fn with_duckdb() -> Self {
        let mut r = Runtime::empty();
        r.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
        r
    }

    /// DuckDB (server) + DuckDB-Wasm descriptor (browser): the planner chooses where to run.
    pub fn with_duckdb_and_browser() -> Self {
        let mut r = Runtime::with_duckdb();
        r.register_client(Box::new(DuckDbWasmDescriptor { config: BrowserEngineConfig::default() }));
        r
    }

    pub fn register(&mut self, e: Box<dyn ExecutionEngine>) {
        self.state.register_engine(&e.id().to_string());
        self.engines.push(e);
    }

    /// Register the optional PostGIS engine: a peer of DuckDB under the same contract (no special path anywhere).
    pub fn register_postgis(&mut self, engine: PostGisEngine) {
        let shared = Arc::new(engine);
        self.postgis = Some(shared.clone());
        self.register(Box::new(SharedPostGis(shared)));
    }

    /// Register PostGIS when the environment configures it (`GEOFABRIC_POSTGIS_*`, docs/postgis.md). `Ok(false)` = not configured:
    /// the normal case, nothing happens. `Err` = configured wrongly (reported by the caller; PostGIS stays off, GeoFabric still runs).
    pub fn register_postgis_from_env(&mut self) -> Result<bool, String> {
        let cfg = geo_engine_postgis::PostGisConfig::from_env()?;
        match geo_engine_postgis::engine_from_config(&cfg)? {
            Some(engine) => {
                self.register_postgis(engine);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Ask the configured server whether it can run our SQL (system catalogs only) and remember the answer. No-op without PostGIS.
    pub fn probe_postgis(&self) {
        if let Some(e) = &self.postgis {
            let _ = e.probe();
        }
    }

    /// For `/api/state`: `{configured: false}` or the sources/probe of the configured engine (never the URL or credentials).
    pub fn postgis_status(&self) -> serde_json::Value {
        match &self.postgis {
            None => serde_json::json!({"configured": false}),
            Some(e) => serde_json::json!({
                "configured": true,
                "sources": e.catalog().len(),
                "probe": e.last_probe(),
            }),
        }
    }

    /// `None` when PostGIS is not configured.
    pub fn postgis(&self) -> Option<&Arc<PostGisEngine>> {
        self.postgis.as_ref()
    }

    pub fn register_client(&mut self, d: Box<dyn EngineDescriptor>) {
        self.state.register_engine(&d.id().to_string());
        self.clients.push(d);
    }

    /// Probes, health, telemetry and network profile the planner plans with.
    pub fn state(&self) -> &Arc<PlannerState> {
        &self.state
    }

    pub fn engines(&self) -> &[Box<dyn ExecutionEngine>] {
        &self.engines
    }

    pub fn set_planner_config(&mut self, cfg: PlannerConfig) {
        self.planner = Planner { config: cfg };
    }

    pub fn planner_config(&self) -> &PlannerConfig {
        &self.planner.config
    }

    /// IR `Source` node for a GeoParquet file (schema + CRS + stats from the footer).
    /// The URI is authorized through the context's guard **before** the file is opened.
    pub fn open_source(&self, uri: &str, ctx: &ExecutionContext) -> Result<Node, RuntimeError> {
        let a = ctx.guard.authorize(uri).map_err(|_| RuntimeError::AccessDenied)?;
        Ok(GeoParquetSource::open_authorized(&a)?.to_ir_source())
    }

    /// Make the spatial histogram (and column statistics) of one GeoParquet file available to the planner: load `.gf-cache/<stem>.hist.json` next to it
    /// when it still describes the file, otherwise build it and write the sidecar. The gateway does this in the background for its data directories; a
    /// benchmark harness calls it up front so that it measures the SAME planner the product runs (estimator v3), not the footer-only fallback.
    /// Returns a note (`cached` / `built in N ms`).
    pub fn ensure_histogram(&self, path: &std::path::Path, guard: &dyn geo_types::SourceAccessGuard) -> Result<String, String> {
        use geo_source_geoparquet::SpatialHistogram;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let a = guard.authorize(&path.to_string_lossy()).map_err(|_| "not authorized".to_string())?;
        let src = GeoParquetSource::open_authorized(&a).map_err(|e| e.to_string())?;
        let side = path.parent().unwrap_or_else(|| std::path::Path::new(".")).join(".gf-cache").join(format!("{stem}.hist.json"));
        if let Ok(text) = std::fs::read_to_string(&side) {
            if let Ok(h) = serde_json::from_str::<SpatialHistogram>(&text) {
                if h.matches(&src) {
                    self.state.histograms().put(src.path(), h);
                    return Ok("cached".into());
                }
            }
        }
        let h = SpatialHistogram::build(&src, geo_source_geoparquet::DEFAULT_GRID, geo_source_geoparquet::DEFAULT_GRID).map_err(|e| e.to_string())?;
        let ms = h.build_ms;
        if let Some(cache) = side.parent() {
            let _ = std::fs::create_dir_all(cache);
        }
        let _ = std::fs::write(&side, serde_json::to_string(&h).unwrap_or_default());
        self.state.histograms().put(src.path(), h);
        Ok(format!("built in {ms} ms"))
    }

    /// A failed execution is a statement about the engine that ran it: health must blame THAT engine (not a hard-coded one), with the
    /// planner's own failure classification. User errors, limits, authorization and cancellation say nothing about the engine.
    fn record_engine_failure(&self, engine: &str, plan: &Plan, e: &geo_executor::EngineError) {
        use geo_executor::EngineError;
        let classified = match e {
            EngineError::Timeout { limit_ms } => Some((geo_planner::FailureClass::Timeout, format!("timeout after {limit_ms} ms"))),
            EngineError::Engine(m) => Some((geo_planner::FailureClass::classify(m), m.clone())),
            _ => None,
        };
        if let Some(outcome) = classified {
            let source = plan.nodes.values().find_map(|n| if let PlanNode::Source { uri, .. } = n { Some(geo_planner::source_key(uri)) } else { None }).unwrap_or_default();
            self.state.record(geo_planner::ExecutionReport { engine: engine.to_string(), source, outcome: Err(outcome), observation: None });
        }
    }

    /// Plan across **all** registered engines (executors first, then client engines).
    pub fn plan(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<PhysicalPlan, RuntimeError> {
        self.plan_with(&self.planner.config, plan, ctx)
    }

    /// Like [`Self::plan`] with an explicit planner configuration (per-request overrides).
    pub fn plan_with(&self, cfg: &PlannerConfig, plan: &Plan, ctx: &ExecutionContext) -> Result<PhysicalPlan, RuntimeError> {
        self.plan_with_inputs(cfg, plan, ctx, &self.state.snapshot())
    }

    /// Plan with an explicit snapshot of the world (per-request overrides: network profile, browser warm/cold, ...).
    pub fn plan_with_inputs(&self, cfg: &PlannerConfig, plan: &Plan, ctx: &ExecutionContext, inputs: &PlanInputs) -> Result<PhysicalPlan, RuntimeError> {
        authorize_all(plan, ctx)?;
        let mut refs: Vec<&dyn EngineDescriptor> = Vec::new();
        for e in &self.engines {
            refs.push(e.as_ref() as &dyn EngineDescriptor);
        }
        for c in &self.clients {
            refs.push(c.as_ref());
        }
        Ok(Planner { config: cfg.clone() }.plan_with_inputs(plan, &refs, ctx, inputs)?)
    }

    /// Plan, then execute here if an in-process engine was selected; otherwise tell the
    /// caller to run the plan on the selected client engine.
    pub fn dispatch(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Dispatch, RuntimeError> {
        self.dispatch_with(&self.planner.config, plan, ctx)
    }

    pub fn dispatch_with(&self, cfg: &PlannerConfig, plan: &Plan, ctx: &ExecutionContext) -> Result<Dispatch, RuntimeError> {
        self.dispatch_with_inputs(cfg, plan, ctx, &self.state.snapshot())
    }

    pub fn dispatch_with_inputs(&self, cfg: &PlannerConfig, plan: &Plan, ctx: &ExecutionContext, inputs: &PlanInputs) -> Result<Dispatch, RuntimeError> {
        let physical = self.plan_with_inputs(cfg, plan, ctx, inputs)?;
        match self.engines.iter().find(|e| e.id() == physical.engine) {
            Some(engine) => match engine.execute(plan, ctx) {
                Ok(result) => Ok(Dispatch::Executed(RunOutput { physical, result })),
                Err(e) => {
                    self.record_engine_failure(&physical.engine.to_string(), plan, &e);
                    Err(e.into())
                }
            },
            None => Ok(Dispatch::Client { physical }),
        }
    }

    /// Execute in-process (errors with `ClientEngineSelected` if the planner picked a client engine).
    pub fn run(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<RunOutput, RuntimeError> {
        match self.dispatch(plan, ctx)? {
            Dispatch::Executed(o) => Ok(o),
            Dispatch::Client { physical } => Err(RuntimeError::ClientEngineSelected(physical.engine.to_string())),
        }
    }
}

/// The registered executor delegating to the shared engine (so the runtime can also read its probe/status).
struct SharedPostGis(Arc<PostGisEngine>);

impl EngineDescriptor for SharedPostGis {
    fn id(&self) -> geo_executor::EngineId {
        self.0.id()
    }
    fn capabilities(&self) -> geo_executor::Capabilities {
        self.0.capabilities()
    }
    fn source_residency(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<(), String> {
        self.0.source_residency(plan, ctx)
    }
    fn calibration(&self) -> geo_executor::EngineCalibration {
        self.0.calibration()
    }
    fn cost_estimate(&self, plan: &Plan, features: &geo_executor::WorkloadFeatures) -> Option<geo_executor::EngineCostEstimate> {
        self.0.cost_estimate(plan, features)
    }
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<geo_executor::Estimate, geo_executor::EngineError> {
        self.0.estimate(plan, ctx)
    }
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<geo_executor::ExplainStep>, geo_executor::EngineError> {
        self.0.explain(plan, ctx)
    }
}

impl ExecutionEngine for SharedPostGis {
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, geo_executor::EngineError> {
        self.0.execute(plan, ctx)
    }
}
