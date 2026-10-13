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

    pub fn plan(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<PhysicalPlan, RuntimeError> {
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
