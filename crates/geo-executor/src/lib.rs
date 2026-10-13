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

/// Where an engine runs relative to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// Runs next to the data (server / local process): data stays put, results travel.
    Server,
    /// Runs in the user's browser: the data it needs travels to the client.
    Browser,
}

/// Hard limits an engine declares; the planner must respect them (a browser tab must
/// not be crashed by an optimistic plan).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EngineLimits {
    pub max_input_bytes: Option<u64>,
    pub max_result_rows: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub kind: EngineKind,
    pub ops: BTreeSet<OpKind>,
    pub sources: BTreeSet<SourceFormat>,
    pub limits: EngineLimits,
    /// `BboxFilter` can only run through the source's bbox covering column (no
    /// spatial extension available to this engine).
    pub bbox_filter_requires_covering: bool,
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

/// What the planner needs to know about an engine. Engines that run elsewhere
/// (the browser) are described to the planner here and executed by their own
/// host; the planner never executes anything.
pub trait EngineDescriptor: Send + Sync {
    fn id(&self) -> EngineId;
    fn capabilities(&self) -> Capabilities;
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Estimate, EngineError>;
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError>;
}

/// An engine that can execute plans inside this process.
pub trait ExecutionEngine: EngineDescriptor {
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError>;
}

/// Canonical result digest shared by the test-suite, benchmarks, gateway and the browser
/// executor: one string per row (cells joined by `|`; Int64 decimal, Utf8 raw, Binary
/// lowercase hex, null `NULL`), rows sorted, SHA-256 over them joined by a newline.
/// `count` is the value of a scalar `count(*)`-style result (1 row x 1 Int64 column),
/// otherwise the number of rows.
pub fn result_digest(batches: &[RecordBatch]) -> (u64, String) {
    use arrow_array::cast::AsArray;
    use arrow_array::Array;
    use arrow_schema::DataType;
    use sha2::{Digest, Sha256};
    let mut rows: Vec<String> = Vec::new();
    for b in batches {
        for r in 0..b.num_rows() {
            let cells: Vec<String> = b
                .columns()
                .iter()
                .map(|c| {
                    if c.is_null(r) {
                        "NULL".to_string()
                    } else {
                        match c.data_type() {
                            DataType::Int64 => c.as_primitive::<arrow_array::types::Int64Type>().value(r).to_string(),
                            DataType::Utf8 => c.as_string::<i32>().value(r).to_string(),
                            DataType::Binary => c.as_binary::<i32>().value(r).iter().map(|x| format!("{x:02x}")).collect(),
                            other => format!("<unsupported {other}>"),
                        }
                    }
                })
                .collect();
            rows.push(cells.join("|"));
        }
    }
    let n_rows = rows.len() as u64;
    let scalar = n_rows == 1
        && batches.iter().any(|b| b.num_rows() == 1 && b.num_columns() == 1 && b.column(0).data_type() == &arrow_schema::DataType::Int64);
    let count = if scalar { rows[0].parse::<u64>().unwrap_or(n_rows) } else { n_rows };
    rows.sort();
    let mut h = Sha256::new();
    h.update(rows.join("\n").as_bytes());
    (count, h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
