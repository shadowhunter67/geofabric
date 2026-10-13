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
use geo_executor::{EngineDescriptor, ExecutionContext, ExecutionEngine, ExecutionResult};
use geo_ir::{Node, Plan, PlanNode};
use geo_planner::{PhysicalPlan, PlanError, Planner, PlannerConfig};
use geo_source_geoparquet::{GeoParquetSource, SourceError};

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
        Runtime { engines: Vec::new(), clients: Vec::new(), planner: Planner::default() }
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
        self.engines.push(e);
    }

    pub fn register_client(&mut self, d: Box<dyn EngineDescriptor>) {
        self.clients.push(d);
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

    /// Plan across **all** registered engines (executors first, then client engines).
    pub fn plan(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<PhysicalPlan, RuntimeError> {
        self.plan_with(&self.planner.config, plan, ctx)
    }

    /// Like [`Self::plan`] with an explicit planner configuration (per-request overrides).
    pub fn plan_with(&self, cfg: &PlannerConfig, plan: &Plan, ctx: &ExecutionContext) -> Result<PhysicalPlan, RuntimeError> {
        authorize_all(plan, ctx)?;
        let mut refs: Vec<&dyn EngineDescriptor> = Vec::new();
        for e in &self.engines {
            refs.push(e.as_ref() as &dyn EngineDescriptor);
        }
        for c in &self.clients {
            refs.push(c.as_ref());
        }
        Ok(Planner { config: cfg.clone() }.plan(plan, &refs, ctx)?)
    }

    /// Plan, then execute here if an in-process engine was selected; otherwise tell the
    /// caller to run the plan on the selected client engine.
    pub fn dispatch(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Dispatch, RuntimeError> {
        self.dispatch_with(&self.planner.config, plan, ctx)
    }

    pub fn dispatch_with(&self, cfg: &PlannerConfig, plan: &Plan, ctx: &ExecutionContext) -> Result<Dispatch, RuntimeError> {
        let physical = self.plan_with(cfg, plan, ctx)?;
        match self.engines.iter().find(|e| e.id() == physical.engine) {
            Some(engine) => {
                let result = engine.execute(plan, ctx)?;
                Ok(Dispatch::Executed(RunOutput { physical, result }))
            }
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
