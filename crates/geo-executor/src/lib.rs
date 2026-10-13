//! Execution engine contract (RFC-002).
//!
//! An engine receives a *validated* logical plan ([`geo_ir::Plan`]) and
//! returns Arrow record batches plus metrics. How it gets there (SQL, API
//! calls, WASM...) is private to the adapter.

use arrow_array::RecordBatch;
use geo_ir::{OpKind, Plan, SourceFormat};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct EngineId(pub String);

impl EngineId {
    pub fn new(s: impl Into<String>) -> Self {
        EngineId(s.into())
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub ops: BTreeSet<OpKind>,
    pub sources: BTreeSet<SourceFormat>,
    /// Where this engine runs, for planner explanations (e.g. "local process").
    pub location: String,
    pub notes: Vec<String>,
}

impl Capabilities {
    /// Operations of `plan` the engine cannot run.
    pub fn missing_ops(&self, plan: &Plan) -> Vec<OpKind> {
        plan.required_ops().into_iter().filter(|o| !self.ops.contains(o)).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// From file metadata (e.g. row-group statistics).
    Metadata,
    /// Heuristic guess.
    Heuristic,
}

#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub rows_upper_bound: Option<u64>,
    pub bytes_to_read: Option<u64>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplainStep {
    pub label: String,
    pub detail: String,
}

/// Limits and sandbox for one execution. Defaults are deliberately finite.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Source files must live under one of these roots (arbitrary file access
    /// is refused). Empty = no file source allowed.
    pub allowed_roots: Vec<PathBuf>,
    pub max_rows: u64,
    pub memory_limit_mb: u64,
    pub threads: usize,
}

impl ExecutionContext {
    pub fn new(allowed_roots: Vec<PathBuf>) -> Self {
        ExecutionContext { allowed_roots, max_rows: 1_000_000, memory_limit_mb: 2048, threads: 4 }
    }
}

/// Metrics recorded for every execution. `*_source` fields say whether a
/// number is measured by the engine or derived from file statistics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExecutionMetrics {
    pub elapsed_ms: f64,
    pub rows_returned: u64,
    pub rows_scanned: Option<u64>,
    pub bytes_read: Option<u64>,
    pub row_groups_total: Option<u64>,
    pub row_groups_skipped: Option<u64>,
    pub peak_memory_bytes: Option<u64>,
    /// How the scan numbers were obtained, e.g. "geoparquet-statistics-analysis".
    pub scan_metrics_source: String,
    pub notes: Vec<String>,
}

pub struct ExecutionResult {
    pub batches: Vec<RecordBatch>,
    pub metrics: ExecutionMetrics,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("plan failed validation: {0}")]
    InvalidPlan(String),
    #[error("unsupported by engine: {0}")]
    Unsupported(String),
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("engine error: {0}")]
    Engine(String),
}

pub trait ExecutionEngine: Send + Sync {
    fn id(&self) -> EngineId;
    fn capabilities(&self) -> Capabilities;
    fn estimate(&self, plan: &Plan) -> Result<Estimate, EngineError>;
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError>;
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError>;
}
