//! Composition root. Applications talk to this crate (or the HTTP layer on
//! top of it), never to an engine directly.

use geo_engine_duckdb::{DuckDbConfig, DuckDbEngine};
use geo_executor::{ExecutionContext, ExecutionEngine, ExecutionResult};
use geo_ir::{Node, Plan};
use geo_planner::{PhysicalPlan, PlanError, Planner};
use geo_source_geoparquet::{GeoParquetSource, SourceError};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("execution failed: {0}")]
    Execution(#[from] geo_executor::EngineError),
}

pub struct RunOutput {
    pub physical: PhysicalPlan,
    pub result: ExecutionResult,
}

pub struct Runtime {
    engines: Vec<Box<dyn ExecutionEngine>>,
    planner: Planner,
}

impl Runtime {
    pub fn empty() -> Self {
        Runtime { engines: Vec::new(), planner: Planner::default() }
    }

    /// Default runtime: DuckDB as the first (and, in the MVP, only) engine.
    pub fn with_duckdb() -> Self {
        let mut r = Runtime::empty();
        r.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
        r
    }

    pub fn register(&mut self, e: Box<dyn ExecutionEngine>) {
        self.engines.push(e);
    }

    pub fn engines(&self) -> &[Box<dyn ExecutionEngine>] {
        &self.engines
    }

    /// IR `Source` node for a GeoParquet file (schema + CRS + stats from the footer).
    pub fn open_source(&self, path: impl AsRef<Path>) -> Result<Node, RuntimeError> {
        Ok(GeoParquetSource::open(path)?.to_ir_source())
    }

    /// Every Source URI must resolve inside `ctx.allowed_roots` *before* any file
    /// is opened for planning (planning reads Parquet footers).
    pub fn check_source_access(plan: &Plan, ctx: &ExecutionContext) -> Result<(), RuntimeError> {
        fn walk(n: &Node, ctx: &ExecutionContext) -> Result<(), geo_executor::EngineError> {
            if let Node::Source { uri, .. } = n {
                if uri.contains("://") {
                    return Err(geo_executor::EngineError::AccessDenied(format!("remote source '{uri}' not enabled")));
                }
                let p = std::fs::canonicalize(uri)
                    .map_err(|_| geo_executor::EngineError::AccessDenied(format!("cannot access '{uri}'")))?;
                let ok = ctx
                    .allowed_roots
                    .iter()
                    .filter_map(|r| std::fs::canonicalize(r).ok())
                    .any(|r| p.starts_with(r));
                if !ok {
                    return Err(geo_executor::EngineError::AccessDenied(format!("'{uri}' is outside the allowed roots")));
                }
            }
            for i in n.inputs() {
                walk(i, ctx)?;
            }
            Ok(())
        }
        Ok(walk(&plan.root, ctx)?)
    }

    pub fn plan(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<PhysicalPlan, RuntimeError> {
        Self::check_source_access(plan, ctx)?;
        let refs: Vec<&dyn ExecutionEngine> = self.engines.iter().map(|e| e.as_ref()).collect();
        Ok(self.planner.plan(plan, &refs, ctx)?)
    }

    pub fn run(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<RunOutput, RuntimeError> {
        let physical = self.plan(plan, ctx)?;
        let engine = self
            .engines
            .iter()
            .find(|e| e.id() == physical.engine)
            .expect("planner returned a registered engine");
        let result = engine.execute(plan, ctx)?;
        Ok(RunOutput { physical, result })
    }
}
