//! Execution engine contract (RFC-002).
//!
//! An engine receives a *validated* logical plan ([`geo_ir::Plan`]) and
//! returns Arrow record batches plus metrics. How it gets there (SQL, API
//! calls, WASM...) is private to the adapter.

use arrow_array::RecordBatch;
use geo_ir::{OpKind, Plan, SourceFormat};
use geo_types::{Crs, RootedAccessGuard, SourceAccessGuard};
use serde::{Deserialize, Serialize};
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
pub mod cost_model;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineCalibration {
    /// The planner's cost model for this engine was fitted/validated on measurements.
    Measured,
    /// No measurements yet: the planner uses a stand-in model and says so.
    Unmeasured,
}

/// Plan-time facts about a workload, computed ONCE by the planner from source metadata and the row estimator and handed to an engine's cost
/// model. Engine independent (no SQL, no index names); everything here is knowable before anything runs - nothing is a runtime observation.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkloadFeatures {
    pub sources: Vec<SourceFeatures>,
    /// Estimated rows the query returns / the pessimistic bound / the rows that match before aggregate and LIMIT.
    pub result_rows: u64,
    pub result_rows_high: u64,
    pub cardinality: u64,
    pub result_geom_bytes: f64,
    pub result_attr_bytes: f64,
    /// Selected / total rows of the first source (the spatial window's selectivity), when the plan has a window.
    pub bbox_selectivity: Option<f64>,
    /// `features | count | count_by`
    pub shape: String,
    pub has_bbox_filter: bool,
    pub has_filter: bool,
    pub has_sort: bool,
    pub has_aggregate: bool,
    pub has_intersects: bool,
    pub has_transform: bool,
    pub limit: Option<u64>,
    pub limit_certain: bool,
    pub bounded_by_limit: bool,
    pub semi_join: Option<SemiJoinFeatures>,
    /// `good | fair | poor`: how much the row estimate can be trusted.
    pub estimate_quality: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceFeatures {
    pub uri: String,
    pub total_rows: u64,
    /// Rows the spatial window is expected to select (uniform/histogram estimate, before attribute filters).
    pub selected_rows: f64,
}

/// A SpatialSemiJoin: the shape that makes the cost of repeated right-side probes matter.
///
/// The `Option` fields are the CANDIDATE-WORK features (metric-semijoin-features-v1): each is `Some` only when the planner could really estimate it
/// (window area from the plan's own `BboxFilter`s, filter selectivity from the column statistics of the histogram sidecar). `None` means UNKNOWN -
/// never zero and never one - and a model that needs a `None` feature must say so instead of guessing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SemiJoinFeatures {
    pub left_uri: String,
    pub right_uri: String,
    pub left_rows: f64,
    pub right_rows: f64,
    pub left_total_rows: u64,
    pub right_total_rows: u64,
    /// The predicate runs on projected (metric) geometries, i.e. an explicit Transform feeds it.
    pub metric: bool,
    /// `within_distance | intersects | other`
    pub predicate: String,
    pub distance_m: Option<f64>,
    /// Right-side probes the join has to make: one per left row (after the left filter when its selectivity is known).
    pub expected_probes: f64,
    /// Area (km2) of the left window and of the right window (the left window grown by the distance), from the plan's `BboxFilter`s.
    pub left_window_km2: Option<f64>,
    pub right_window_km2: Option<f64>,
    /// Selectivity of the attribute filters on each side: `Some(1.0)` = no filter, `Some(s)` = estimated from column statistics, `None` = a filter exists
    /// but the planner has no statistics for it.
    pub left_filter_selectivity: Option<f64>,
    pub right_filter_selectivity: Option<f64>,
    /// Right rows after its spatial window AND its filter (None when the filter selectivity is unknown).
    pub right_rows_filtered: Option<f64>,
    /// Filtered right features per km2 of the right window.
    pub right_density_per_km2: Option<f64>,
    /// Expected right features within `distance` of one probe: density x pi x d^2.
    pub candidates_per_probe: Option<f64>,
    /// `expected_probes x candidates_per_probe`: the candidate pairs the join has to examine.
    pub candidate_pairs: Option<f64>,
    /// Left window area / right window area (1 = the distance is negligible against the window).
    pub overlap_ratio: Option<f64>,
    /// `histogram` when both sides' row counts came from a spatial histogram, `uniform` for the footer-only fallback.
    pub estimate_basis: String,
}

/// How much evidence stands behind an engine's cost model for ONE workload class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelValidation {
    /// Enough held-out samples, acceptable prediction error and planner regret, no unsafe selection: the planner may choose on this cost.
    Validated,
    /// Measured but not (yet) validated: explained, never chosen on predicted cost over a measured engine (R5d).
    Limited,
    /// No measurements for this class.
    Unvalidated,
}

#[derive(Debug, Clone, Serialize)]
pub struct CostTerm {
    pub name: String,
    pub ms: f64,
}

/// A HARD risk policy, deliberately separate from the cost: a blocked workload is not "expensive", it is refused.
#[derive(Debug, Clone, Serialize)]
pub struct RiskVerdict {
    pub blocked: bool,
    /// `low | elevated | high`
    pub level: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineCostEstimate {
    pub model_version: String,
    pub workload_class: String,
    pub validation: ModelValidation,
    /// Estimated server-side time (everything except the network to the client), milliseconds.
    pub time_ms: f64,
    pub terms: Vec<CostTerm>,
    pub risk: RiskVerdict,
    /// `available | unavailable | unknown`: the physical access path the model assumed (index) can be relied on.
    pub index_state: String,
    pub notes: Vec<String>,
}

/// What the planner needs to know about an engine. Engines that run elsewhere
/// (the browser) are described to the planner here and executed by their own
/// host; the planner never executes anything.
pub trait EngineDescriptor: Send + Sync {
    fn id(&self) -> EngineId;
    fn capabilities(&self) -> Capabilities;
    /// Can this engine reach the data of `plan`? File engines read the logical file; a database engine can only run plans
    /// whose sources are resident in it. `Err(reason)` makes the engine infeasible for this plan BEFORE anything runs.
    /// Must not perform I/O on a source: authorization (the planner's caller) has already happened, and this is a metadata check.
    fn source_residency(&self, _plan: &Plan, _ctx: &ExecutionContext) -> Result<(), String> {
        Ok(())
    }
    /// Has this engine's cost behaviour been measured on this system? An `Unmeasured` engine is feasible and explained, but the
    /// planner never prefers it on predicted cost over a `Measured` one (policy R5d): a stand-in cost model is not evidence.
    fn calibration(&self) -> EngineCalibration {
        EngineCalibration::Measured
    }
    /// An engine-owned cost model for THIS workload. `None` (default) = the planner's shared server model applies. An engine that returns
    /// `Some` states its model version, the workload class, how well that class is validated and a hard risk verdict; the planner treats a
    /// non-`Validated` class like an `Unmeasured` engine (R5d) and refuses a blocked one. The planner never learns what is inside the model.
    fn cost_estimate(&self, _plan: &Plan, _features: &WorkloadFeatures) -> Option<EngineCostEstimate> {
        None
    }
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Estimate, EngineError>;
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError>;
}

/// An engine that can execute plans inside this process.
pub trait ExecutionEngine: EngineDescriptor {
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError>;
}

/// Canonical result digest shared by the test-suite, benchmarks, gateway and the browser
/// executor: one string per row (cells joined by `|`; integers decimal, Boolean true/false, Utf8 raw, Binary
/// lowercase hex, null `NULL`), rows sorted, SHA-256 over them joined by a newline.
/// `count` is the value of a scalar `count(*)`-style result (1 row x 1 Int64 column),
/// otherwise the number of rows.
/// Floats are rendered with Rust's shortest round-trip `Display` (`4.0` -> "4", `0.1` -> "0.1"),
/// which equals JavaScript's `Number#toString` for magnitudes in [1e-6, 1e21); NaN/inf map to the
/// JavaScript spellings. Outside that range Rust and JS differ: do not digest such floats.
fn float_cell(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else {
        format!("{v}")
    }
}

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
                            DataType::Int32 => c.as_primitive::<arrow_array::types::Int32Type>().value(r).to_string(),
                            DataType::Int16 => c.as_primitive::<arrow_array::types::Int16Type>().value(r).to_string(),
                            DataType::Int8 => c.as_primitive::<arrow_array::types::Int8Type>().value(r).to_string(),
                            DataType::UInt64 => c.as_primitive::<arrow_array::types::UInt64Type>().value(r).to_string(),
                            DataType::UInt32 => c.as_primitive::<arrow_array::types::UInt32Type>().value(r).to_string(),
                            DataType::Boolean => c.as_boolean().value(r).to_string(),
                            DataType::Float64 => float_cell(c.as_primitive::<arrow_array::types::Float64Type>().value(r)),
                            DataType::Float32 => float_cell(c.as_primitive::<arrow_array::types::Float32Type>().value(r) as f64),
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

#[cfg(test)]
mod digest_tests {
    use super::*;
    use arrow_array::{ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray};
    use arrow_schema::{Field, Schema};
    use std::sync::Arc;

    fn batch(cols: Vec<(&str, ArrayRef)>) -> RecordBatch {
        let schema = Arc::new(Schema::new(cols.iter().map(|(n, a)| Field::new(*n, a.data_type().clone(), true)).collect::<Vec<_>>()));
        RecordBatch::try_new(schema, cols.into_iter().map(|(_, a)| a).collect()).unwrap()
    }

    // expected hashes computed independently with python hashlib over the sorted row strings
    #[test]
    fn mixed_columns_with_nulls_match_the_python_digest() {
        let b = batch(vec![
            ("id", Arc::new(Int64Array::from(vec![2, 1, 10])) as ArrayRef),
            ("name", Arc::new(StringArray::from(vec!["c", "a", "b"])) as ArrayRef),
            ("flag", Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])) as ArrayRef),
        ]);
        let (count, hash) = result_digest(&[b]);
        assert_eq!(count, 3);
        assert_eq!(hash, "bd88b34dca3530c0e765d9589773fb219bd551f3c64782051de10e7f3489f455");
    }

    #[test]
    fn floats_use_the_shortest_round_trip_spelling() {
        let b = batch(vec![("h", Arc::new(Float64Array::from(vec![1.5, 4.0, 0.1])) as ArrayRef)]);
        assert_eq!(result_digest(&[b]).1, "1607ef75523fb4893a4bf4aa248381a9b62595e6a987d6bf06f517c1182811c8");
    }

    #[test]
    fn scalar_integer_result_is_a_count_and_int32_is_supported() {
        let c = batch(vec![("n", Arc::new(Int64Array::from(vec![7])) as ArrayRef)]);
        assert_eq!(result_digest(&[c]), (7, "7902699be42c8a8e46fbbb4501726517e86b22c56a189f7625a6da49081b2451".to_string()));
        let v = batch(vec![("version", Arc::new(Int32Array::from(vec![3, -5])) as ArrayRef)]);
        let (count, hash) = result_digest(&[v]);
        assert_eq!(count, 2, "two rows, not a scalar count");
        assert_eq!(hash, "51e37fe8c8a5befb8bb7865806599206d250b1e721383f11f58190104aec07c2");
    }
}

/// Attach GeoArrow extension metadata (name + CRS from the IR schema) to WKB geometry columns. Shared by every engine so a
/// geometry column is described identically whichever engine produced it.
pub fn tag_geometry(batches: Vec<RecordBatch>, geoms: &[(String, Crs)]) -> Vec<RecordBatch> {
    use arrow_schema::{Field, Schema};
    use std::collections::HashMap;
    if geoms.is_empty() {
        return batches;
    }
    batches
        .into_iter()
        .map(|b| {
            let fields: Vec<Arc<Field>> = b
                .schema()
                .fields()
                .iter()
                .map(|f| match geoms.iter().find(|(n, _)| n == f.name()) {
                    Some((_, crs)) => {
                        let mut md: HashMap<String, String> = f.metadata().clone();
                        md.insert("ARROW:extension:name".into(), "geoarrow.wkb".into());
                        let ext = match crs {
                            Crs::Epsg(c) => serde_json::json!({"crs": format!("EPSG:{c}"), "crs_type": "authority_code"}),
                            Crs::OgcCrs84 => serde_json::json!({"crs": "OGC:CRS84", "crs_type": "authority_code"}),
                            Crs::Undefined => serde_json::json!({"crs_type": "undefined"}),
                            Crs::Unknown(s) => serde_json::json!({"crs": s, "crs_type": "unknown"}),
                        };
                        md.insert("ARROW:extension:metadata".into(), ext.to_string());
                        Arc::new(f.as_ref().clone().with_metadata(md))
                    }
                    None => f.clone(),
                })
                .collect();
            RecordBatch::try_new(Arc::new(Schema::new(fields)), b.columns().to_vec()).unwrap_or(b)
        })
        .collect()
}
