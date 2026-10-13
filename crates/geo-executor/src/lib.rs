//! Execution engine contract (RFC-002).
//!
//! An engine receives a *validated* logical plan ([`geo_ir::Plan`]) and
//! returns Arrow record batches plus metrics. How it gets there (SQL, API
//! calls, WASM...) is private to the adapter.

use arrow_array::RecordBatch;
use geo_ir::{OpKind, Plan, SourceFormat};
use geo_types::{RootedAccessGuard, SourceAccessGuard};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

/// Where a metric value comes from. UIs must show this next to the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricSource {
    /// Measured by the engine adapter while executing.
    EngineObserved,
    /// Read from the source (e.g. Parquet footer): a fact about the data.
    SourceObserved,
    /// Computed from source statistics; an estimate or bound, not a measurement.
    SourceEstimated,
    /// Produced by the planner before execution.
    PlannerEstimated,
    /// Computed from other metrics.
    Derived,
    /// No trustworthy measurement exists. `value` is `None`.
    Unknown,
}

/// A metric with provenance. Engines are **not** required to provide every
/// metric: an unavailable one is `value: None, source: Unknown`, never a
/// made-up number.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metric<T> {
    pub value: Option<T>,
    pub source: MetricSource,
}

impl<T> Metric<T> {
    pub fn new(value: T, source: MetricSource) -> Self {
        Metric { value: Some(value), source }
    }
    pub fn engine_observed(v: T) -> Self {
        Self::new(v, MetricSource::EngineObserved)
    }
    pub fn source_observed(v: T) -> Self {
        Self::new(v, MetricSource::SourceObserved)
    }
    pub fn source_estimated(v: T) -> Self {
        Self::new(v, MetricSource::SourceEstimated)
    }
    pub fn planner_estimated(v: T) -> Self {
        Self::new(v, MetricSource::PlannerEstimated)
    }
    pub fn derived(v: T) -> Self {
        Self::new(v, MetricSource::Derived)
    }
    pub fn unknown() -> Self {
        Metric { value: None, source: MetricSource::Unknown }
    }
}

impl<T> Default for Metric<T> {
    fn default() -> Self {
        Metric::unknown()
    }
}

/// Pre-execution estimate (metadata based).
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub rows_upper_bound: Metric<u64>,
    pub bytes_upper_bound: Metric<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplainStep {
    pub label: String,
    pub detail: String,
}

/// Central default limits (single place; do not hard-code elsewhere).
pub mod limits {
    use std::time::Duration;
    pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
    pub const DEFAULT_MAX_ROWS: u64 = 1_000_000;
    pub const DEFAULT_MEMORY_LIMIT_MB: u64 = 2048;
    pub const DEFAULT_THREADS: usize = 4;
}

/// Cooperative cancellation shared between the caller and the engine
/// (e.g. the gateway cancels when the client disconnects).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Limits and sandbox for one execution. Defaults are deliberately finite.
#[derive(Clone)]
pub struct ExecutionContext {
    /// Authorization boundary: every source is authorized through this guard
    /// before any I/O on it.
    pub guard: Arc<dyn SourceAccessGuard>,
    pub max_rows: u64,
    pub memory_limit_mb: u64,
    pub threads: usize,
    /// Execution timeout enforced inside the engine (query is interrupted).
    pub timeout: Duration,
    pub cancel: CancelToken,
}

impl ExecutionContext {
    /// Context that allows local files under `roots`.
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self::with_guard(Arc::new(RootedAccessGuard::new(roots)))
    }

    pub fn with_guard(guard: Arc<dyn SourceAccessGuard>) -> Self {
        ExecutionContext {
            guard,
            max_rows: limits::DEFAULT_MAX_ROWS,
            memory_limit_mb: limits::DEFAULT_MEMORY_LIMIT_MB,
            threads: limits::DEFAULT_THREADS,
            timeout: limits::DEFAULT_QUERY_TIMEOUT,
            cancel: CancelToken::default(),
        }
    }
}

/// Metrics recorded for every execution, each with provenance.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionMetrics {
    pub elapsed_ms: Metric<f64>,
    pub rows_returned: Metric<u64>,
    pub row_groups_total: Metric<u64>,
    pub row_groups_skipped: Metric<u64>,
    pub rows_scanned_upper_bound: Metric<u64>,
    pub estimated_bytes_upper_bound: Metric<u64>,
    /// Actual I/O. `Unknown` unless the engine has a trustworthy measurement.
    pub bytes_read: Metric<u64>,
    pub peak_memory_bytes: Metric<u64>,
    pub notes: Vec<String>,
}

impl Default for ExecutionMetrics {
    fn default() -> Self {
        ExecutionMetrics {
            elapsed_ms: Metric::unknown(),
            rows_returned: Metric::unknown(),
            row_groups_total: Metric::unknown(),
            row_groups_skipped: Metric::unknown(),
            rows_scanned_upper_bound: Metric::unknown(),
            estimated_bytes_upper_bound: Metric::unknown(),
            bytes_read: Metric::unknown(),
            peak_memory_bytes: Metric::unknown(),
            notes: Vec::new(),
        }
    }
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
    /// Opaque on purpose (no path / reason).
    #[error("source not permitted")]
    AccessDenied,
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("execution timeout after {limit_ms} ms (query interrupted)")]
    Timeout { limit_ms: u64 },
    #[error("execution cancelled (query interrupted)")]
    Cancelled,
    #[error("engine error: {0}")]
    Engine(String),
}

pub trait ExecutionEngine: Send + Sync {
    fn id(&self) -> EngineId;
    fn capabilities(&self) -> Capabilities;
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Estimate, EngineError>;
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError>;
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError>;
}
