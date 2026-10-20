//! HTTP gateway for the web UI.
//!
//! Trust model (v0): binds to 127.0.0.1 by default, no auth. Clients can only
//! submit *logical IR* or structured query templates — never SQL. Every source
//! goes through the `SourceAccessGuard` **before** any file is opened (listing,
//! planning, execution). Results are capped (rows, request size, concurrency)
//! and every query has an execution timeout enforced inside the engine; a
//! client disconnect cancels the running query. GeoJSON is generated here, at
//! the boundary, purely for map display.

pub mod catalog;
pub mod templates;
pub mod wkb;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{Array, RecordBatch};
use arrow_schema::DataType;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use geo_executor::{limits, result_digest, CancelToken, EngineError, ExecutionContext};
use geo_ir::*;
use geo_planner::{
    plan_shape, source_key, ExecutionReport, FailureClass, NetworkProfile, Observation, PhysicalPlan, PlanInputs, ProbeReport,
};
use geo_runtime::{Dispatch, Runtime, RuntimeError};
use geo_source_geoparquet::GeoParquetSource;
use geo_types::*;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use tower_http::limit::RequestBodyLimitLayer;

pub const MAX_FEATURES: u64 = 100_000;
pub const MAX_TABLE_ROWS: usize = 1_000;

/// Gateway limits, configured in one place.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub query_timeout: Duration,
    pub max_concurrent_queries: usize,
    pub max_body_bytes: usize,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        GatewayConfig {
            query_timeout: limits::DEFAULT_QUERY_TIMEOUT,
            max_concurrent_queries: 2,
            max_body_bytes: 1 << 20,
        }
    }
}

/// Development/validation aid: delays every `/api` response as a link with this profile would
/// (latency + shared-bandwidth serialization). It models a network; it is not one.
pub struct SimNet {
    pub profile: NetworkProfile,
    free_at: AsyncMutex<tokio::time::Instant>,
}

impl SimNet {
    pub fn new(profile: NetworkProfile) -> Self {
        SimNet { profile, free_at: AsyncMutex::new(tokio::time::Instant::now()) }
    }
}

pub struct AppState {
    pub rt: Runtime,
    pub data_dirs: Vec<PathBuf>,
    pub manifests_dir: PathBuf,
    pub repo_root: PathBuf,
    pub config: GatewayConfig,
    pub slots: Arc<Semaphore>,
    pub sim: Option<Arc<SimNet>>,
    /// Parquet range requests served to the browser engine (cumulative): lets a measurement run count how many
    /// requests and bytes a browser query really needed (the planner's network model depends on both).
    pub file_requests: std::sync::atomic::AtomicU64,
    pub file_bytes: std::sync::atomic::AtomicU64,
    /// Spatial histograms: sources still waiting to be built, and per-source notes (built / cached / why not).
    pub hist_pending: std::sync::atomic::AtomicUsize,
    pub hist_notes: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    /// Where `/api/devicecheck` saves device-check reports; None = the endpoint does not exist (opt-in: `--devicecheck-dir`).
    pub devicecheck_dir: Option<PathBuf>,
}

impl AppState {
    pub fn new(rt: Runtime, data_dir: PathBuf, config: GatewayConfig) -> Self {
        Self::new_with_dirs(rt, vec![data_dir], config)
    }

    pub fn new_with_dirs(rt: Runtime, data_dirs: Vec<PathBuf>, config: GatewayConfig) -> Self {
        let slots = Arc::new(Semaphore::new(config.max_concurrent_queries));
        AppState {
            rt,
            data_dirs,
            manifests_dir: PathBuf::from("data/manifests"),
            repo_root: PathBuf::from("."),
            config,
            slots,
            sim: None,
            file_requests: Default::default(),
            file_bytes: Default::default(),
            hist_pending: Default::default(),
            hist_notes: Default::default(),
            devicecheck_dir: None,
        }
    }

    pub fn with_catalog_paths(mut self, manifests_dir: PathBuf, repo_root: PathBuf) -> Self {
        self.manifests_dir = manifests_dir;
        self.repo_root = repo_root;
        self
    }

    pub fn primary_dir(&self) -> Option<&PathBuf> {
        self.data_dirs.first()
    }

    pub fn source_path(&self, id: &str) -> Option<PathBuf> {
        self.data_dirs
            .iter()
            .map(|dir| dir.join(format!("{id}.parquet")))
            .find(|path| path.is_file())
    }

    pub fn with_simulated_network(mut self, profile: NetworkProfile) -> Self {
        self.rt.state().set_network(profile.clone());
        self.sim = Some(Arc::new(SimNet::new(profile)));
        self
    }

    fn ctx(&self) -> ExecutionContext {
        let mut ctx = ExecutionContext::new(self.data_dirs.clone());
        ctx.max_rows = MAX_FEATURES + 1;
        ctx.timeout = self.config.query_timeout;
        ctx
    }
}

pub type Shared = Arc<AppState>;

// ---------- errors ----------

pub struct ApiError(StatusCode, Value);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

pub(crate) fn err(status: StatusCode, code: &str, msg: impl Into<String>) -> ApiError {
    ApiError(status, json!({"error": code, "message": msg.into()}))
}

impl From<RuntimeError> for ApiError {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::AccessDenied => err(StatusCode::FORBIDDEN, "access_denied", "source not permitted"),
            RuntimeError::Plan(geo_planner::PlanError::Invalid(r)) => ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error": "invalid_plan", "message": "plan failed validation", "issues": r.issues}),
            ),
            RuntimeError::Plan(geo_planner::PlanError::AccessDenied) => err(StatusCode::FORBIDDEN, "access_denied", "source not permitted"),
            RuntimeError::Plan(geo_planner::PlanError::TooExpensive(m)) => err(StatusCode::PAYLOAD_TOO_LARGE, "too_expensive", m),
            RuntimeError::Plan(p) => err(StatusCode::UNPROCESSABLE_ENTITY, "planning_failed", p.to_string()),
            RuntimeError::Execution(EngineError::AccessDenied) => err(StatusCode::FORBIDDEN, "access_denied", "source not permitted"),
            RuntimeError::Execution(EngineError::LimitExceeded(m)) => err(StatusCode::PAYLOAD_TOO_LARGE, "limit_exceeded", m),
            RuntimeError::Execution(EngineError::Unsupported(m)) => err(StatusCode::UNPROCESSABLE_ENTITY, "unsupported", m),
            RuntimeError::Execution(EngineError::InvalidPlan(m)) => err(StatusCode::UNPROCESSABLE_ENTITY, "invalid_plan", m),
            RuntimeError::Execution(EngineError::Timeout { limit_ms }) => ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                json!({"error": "execution_timeout", "message": format!("query interrupted after {limit_ms} ms"), "limit_ms": limit_ms}),
            ),
            RuntimeError::Execution(EngineError::Cancelled) => err(StatusCode::REQUEST_TIMEOUT, "cancelled", "query cancelled"),
            RuntimeError::Execution(EngineError::Engine(m)) => err(StatusCode::INTERNAL_SERVER_ERROR, "engine_error", m),
            RuntimeError::Source(s) => err(StatusCode::BAD_REQUEST, "source_error", s.to_string()),
            RuntimeError::ClientEngineSelected(m) => err(StatusCode::INTERNAL_SERVER_ERROR, "client_engine", m),
        }
    }
}

// ---------- sources ----------

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() < 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) && !id.contains("..")
}

/// Resolve a source id to an *authorized* source (guard first, then any I/O).
fn authorized_source(st: &AppState, id: &str) -> Result<AuthorizedSource, ApiError> {
    if !valid_id(id) {
        return Err(err(StatusCode::BAD_REQUEST, "bad_id", "invalid source id"));
    }
    let p = st.source_path(id).ok_or_else(|| err(StatusCode::NOT_FOUND, "no_such_source", id))?;
    st.ctx()
        .guard
        .authorize(&p.to_string_lossy())
        .map_err(|_| err(StatusCode::NOT_FOUND, "no_such_source", id))
}

/// UTC timestamp `YYYY-MM-DDTHH:MM:SSZ` of a file's last modification (civil-from-days, no date crate).
fn modified_iso(p: &std::path::Path) -> Option<String> {
    let secs = std::fs::metadata(p).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60))
}

fn list_sources(st: &AppState) -> Vec<Value> {
    let mut by_id = std::collections::BTreeMap::new();
    for dir in &st.data_dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for path in rd.filter_map(|entry| entry.ok()).map(|entry| entry.path()) {
            if path.extension().is_some_and(|extension| extension == "parquet") {
                if let Some(stem) = path.file_stem() {
                    by_id.entry(stem.to_string_lossy().into_owned()).or_insert(path);
                }
            }
        }
    }
    let mut out = Vec::new();
    let files: Vec<_> = by_id.into_values().collect();
    let guard = st.ctx().guard;
    for p in files {
        let id = p.file_stem().unwrap().to_string_lossy().to_string();
        let Ok(a) = guard.authorize(&p.to_string_lossy()) else { continue };
        match GeoParquetSource::open_authorized(&a) {
            Ok(s) => {
                let i = s.info();
                out.push(json!({
                    "id": id, "format": "GeoParquet", "size_bytes": i.file_size, "rows": i.row_count,
                    "row_groups": i.row_groups.len(), "crs": i.crs.to_string(), "crs_known": i.crs.is_known(),
                    "geometry_column": i.geometry_column, "geometry_type": i.geometry_type,
                    "bbox": i.file_bbox, "has_covering": i.covering.is_some(),
                    "covering_inferred": i.covering.as_ref().is_some_and(|c| !c.declared), "modified": modified_iso(&p),
                    "columns": i.schema.fields.iter().map(|f| json!({"name": f.name, "type": f.data_type})).collect::<Vec<_>>(),
                }));
            }
            Err(e) => out.push(json!({"id": id, "error": e.to_string()})),
        }
    }
    out
}

async fn sources(State(st): State<Shared>) -> Json<Value> {
    let st2 = st.clone();
    let v = tokio::task::spawn_blocking(move || list_sources(&st2)).await.unwrap_or_default();
    Json(json!({"sources": v}))
}

async fn row_groups(State(st): State<Shared>, UrlPath(id): UrlPath<String>) -> Result<Json<Value>, ApiError> {
    let a = authorized_source(&st, &id)?;
    let s = GeoParquetSource::open_authorized(&a).map_err(|e| err(StatusCode::BAD_REQUEST, "source_error", e.to_string()))?;
    let i = s.info();
    Ok(Json(json!({"crs": i.crs.to_string(), "row_groups": i.row_groups.iter().map(|r| json!({
        "index": r.index, "rows": r.rows, "bytes": r.compressed_bytes, "bbox": r.bbox})).collect::<Vec<_>>()})))
}

// ---------- query templates -> IR ----------

#[derive(Deserialize)]
pub struct FilterReq {
    column: String,
    /// eq | ne | lt | le | gt | ge (use `value`) or in (use `values`)
    op: String,
    #[serde(default)]
    value: Value,
    #[serde(default)]
    values: Vec<Value>,
}

#[derive(Deserialize)]
pub struct QueryReq {
    source: String,
    /// features | count_by | count | intersects_features | near | near_count | join_count_by
    kind: String,
    /// viewport [xmin, ymin, xmax, ymax] in `crs`
    bbox: [f64; 4],
    crs: String,
    filter: Option<FilterReq>,
    group_by: Option<String>,
    #[serde(rename = "with")]
    with_req: Option<WithReq>,
    limit: Option<u64>,
    /// RENDERING only: keep the rows whose `md5(id) mod sample_modulus = 0` (a power of two, at most 2^20): a deterministic,
    /// spatially uniform thinning that nests (the sample at modulus 2m is contained in the one at m). Valid for the
    /// row-returning kinds, never for counts or aggregates: analytics always see the full logical result (ADR-012).
    sample_modulus: Option<u32>,
    /// Engine preference ("duckdb" | "duckdb-wasm"); honored only if feasible.
    engine: Option<String>,
    /// Override `allow_browser` for this request.
    browser: Option<bool>,
    /// What-if network profile for this request only (planner evaluation), e.g. "MOBILE".
    network: Option<String>,
    /// State of the client's browser engine (cold start cost).
    client: Option<ClientState>,
    /// Measurement aid: operator memory limit (MB) for the browser engine for this request only (replaces the
    /// risk model's memory rejection, like `--browser-max-mb`). Used to run the browser engine past the planner's guard.
    browser_limit_mb: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithReq {
    source: String,
    filter: Option<FilterReq>,
    distance_m: Option<f64>,
    group_by: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
pub struct ClientState {
    /// The DuckDB-Wasm instance is already instantiated.
    warm: Option<bool>,
}

/// Per-request overrides of the planning context.
#[derive(Default, Clone)]
struct Opts {
    browser_limit_mb: Option<u64>,
    browser: Option<bool>,
    network: Option<NetworkProfile>,
    warm: Option<bool>,
}

fn opts(browser: Option<bool>, network: &Option<String>, client: &Option<ClientState>, limit_mb: Option<u64>) -> Result<Opts, ApiError> {
    let network = match network {
        None => None,
        Some(n) => Some(NetworkProfile::by_name(n).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_network", format!("unknown network profile '{n}'")))?),
    };
    Ok(Opts { browser, network, warm: client.as_ref().and_then(|c| c.warm), browser_limit_mb: limit_mb })
}

fn json_to_literal(v: &Value) -> Result<Literal, ApiError> {
    Ok(match v {
        Value::Bool(b) => Literal::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Literal::Int(i),
            None => Literal::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Literal::Str(s.clone()),
        Value::Null => Literal::Null,
        _ => return Err(err(StatusCode::BAD_REQUEST, "bad_filter", "filter value must be a scalar")),
    })
}

fn apply_filter(mut node: Node, filter: Option<&FilterReq>) -> Result<Node, ApiError> {
    let Some(f) = filter else {
        return Ok(node);
    };
    if f.op == "in" {
        if f.values.is_empty() {
            return Err(err(StatusCode::BAD_REQUEST, "bad_request", "filter op 'in' needs a non-empty values list"));
        }
        let values = f.values.iter().map(json_to_literal).collect::<Result<Vec<_>, _>>()?;
        node = Node::Filter { input: Box::new(node), predicate: Expr::In { column: f.column.clone(), values } };
    } else {
        let op = match f.op.as_str() {
            "eq" => CompareOp::Eq,
            "ne" => CompareOp::Ne,
            "lt" => CompareOp::Lt,
            "le" => CompareOp::Le,
            "gt" => CompareOp::Gt,
            "ge" => CompareOp::Ge,
            _ => return Err(err(StatusCode::BAD_REQUEST, "bad_request", "unknown filter op")),
        };
        node = Node::Filter {
            input: Box::new(node),
            predicate: Expr::Compare { column: f.column.clone(), op, value: json_to_literal(&f.value)? },
        };
    }
    Ok(node)
}

/// Add the render-thinning filter when the request asks for it (and refuse it where it would change an analysis).
pub(crate) fn apply_sample(node: Node, q: &QueryReq) -> Result<Node, ApiError> {
    let Some(m) = q.sample_modulus else { return Ok(node) };
    let bad = |msg: &str| err(StatusCode::BAD_REQUEST, "bad_request", msg.to_string());
    if !matches!(q.kind.as_str(), "features" | "intersects_features" | "near") {
        return Err(bad("sample_modulus is only valid for row-returning kinds (features, intersects_features, near): counts and aggregates always use the full result"));
    }
    if m == 0 || !m.is_power_of_two() || m > (1 << 20) {
        return Err(bad("sample_modulus must be a power of two between 1 and 1048576"));
    }
    if m == 1 {
        return Ok(node);
    }
    let schema = node_schema(&node)?;
    if !schema.fields.iter().any(|f| f.name == "id" && schema.geometry(&f.name).is_none()) {
        return Err(bad("sampling needs an `id` column in the source"));
    }
    Ok(Node::Filter { input: Box::new(node), predicate: Expr::HashBucket { column: "id".into(), modulus: m, remainder: 0 } })
}

fn build_ir(rt: &Runtime, uri: &str, with_uri: Option<&str>, ctx: &ExecutionContext, q: &QueryReq) -> Result<Plan, ApiError> {
    if matches!(q.kind.as_str(), "near" | "near_count" | "join_count_by") {
        return templates::build(rt, uri, with_uri, ctx, q);
    }
    let bad = |m: &str| err(StatusCode::BAD_REQUEST, "bad_request", m.to_string());
    let src = rt.open_source(uri, ctx)?;
    let (geom_col, crs_src) = match &src {
        Node::Source { schema, .. } => {
            let g = schema.geometry_columns.first().ok_or_else(|| bad("source has no geometry column"))?;
            (g.name.clone(), g.crs.clone())
        }
        _ => unreachable!(),
    };
    let crs: Crs = q.crs.parse().map_err(|_| bad("bad crs"))?;
    if crs != crs_src {
        // never transform silently: the validator would reject the mismatch anyway,
        // but a clear message beats an IR issue list for the common UI mistake
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "crs_mismatch",
            format!("viewport CRS {crs} differs from source CRS {crs_src}; explicit transform not available in v0"),
        ));
    }
    let bb = BoundingBox::new(q.bbox[0], q.bbox[1], q.bbox[2], q.bbox[3], crs.clone()).map_err(|e| bad(&e.to_string()))?;
    let mut node = match q.kind.as_str() {
        "intersects_features" => {
            let (a, b, c, d) = (bb.min_x(), bb.min_y(), bb.max_x(), bb.max_y());
            Node::Intersects {
                input: Box::new(src.clone()),
                geometry_column: geom_col.clone(),
                with: GeometryLiteral { wkt: format!("POLYGON(({a} {b}, {c} {b}, {c} {d}, {a} {d}, {a} {b}))"), crs },
            }
        }
        "features" | "count_by" | "count" => Node::BboxFilter { input: Box::new(src.clone()), geometry_column: geom_col.clone(), bbox: bb },
        _ => return Err(bad("unknown kind")),
    };
    node = apply_filter(node, q.filter.as_ref())?;
    node = apply_sample(node, q)?;
    let limit = q.limit.unwrap_or(20_000).min(MAX_FEATURES);
    node = match q.kind.as_str() {
        "count" => Node::Aggregate {
            input: Box::new(node),
            group_by: vec![],
            aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
        },
        "count_by" => {
            let g = q.group_by.clone().ok_or_else(|| bad("group_by required"))?;
            Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate {
                        input: Box::new(node),
                        group_by: vec![g.clone()],
                        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
                    }),
                    keys: vec![SortKey { column: "count".into(), descending: true }, SortKey { column: g.clone(), descending: false }], // the group key breaks ties: two engines must cut the top 50 at the same place
                }),
                n: 50,
            }
        }
        _ => {
            let s = node_schema(&node)?;
            let cols: Vec<String> = s
                .fields
                .iter()
                .filter(|f| !matches!(&f.data_type, FieldType::Other(t) if t != "geometry"))
                .map(|f| f.name.clone())
                .collect();
            Node::Limit { input: Box::new(Node::Project { input: Box::new(node), columns: cols }), n: limit }
        }
    };
    Ok(Plan::new(node))
}

pub(crate) fn node_schema(n: &Node) -> Result<SpatialSchema, ApiError> {
    n.output_schema().map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, "schema", e.to_string()))
}

// ---------- run ----------

fn cell(col: &dyn Array, i: usize) -> Value {
    if col.is_null(i) {
        return Value::Null;
    }
    match col.data_type() {
        DataType::Int64 => json!(col.as_primitive::<Int64Type>().value(i)),
        DataType::Float64 => {
            let v = col.as_primitive::<Float64Type>().value(i);
            if v.is_finite() { json!(v) } else { Value::Null }
        }
        DataType::Utf8 => json!(col.as_string::<i32>().value(i)),
        DataType::Boolean => json!(col.as_boolean().value(i)),
        _ => arrow_cast::display::array_value_to_string(col, i).map(Value::from).unwrap_or(Value::Null),
    }
}

fn is_geometry_field(f: &arrow_schema::Field) -> bool {
    f.metadata().get("ARROW:extension:name").is_some_and(|n| n == "geoarrow.wkb")
}

fn to_response(plan: &Plan, run: geo_runtime::RunOutput, total_ms: f64) -> Value {
    let t0 = Instant::now();
    let batches: &Vec<RecordBatch> = &run.result.batches;
    let schema = batches.first().map(|b| b.schema());
    let mut columns = Vec::new();
    let mut geom_idx: Option<usize> = None;
    if let Some(s) = &schema {
        for (i, f) in s.fields().iter().enumerate() {
            let g = is_geometry_field(f);
            if g && geom_idx.is_none() {
                geom_idx = Some(i);
            }
            columns.push(json!({"name": f.name(), "type": f.data_type().to_string(), "geometry": g,
                "crs": f.metadata().get("ARROW:extension:metadata")}));
        }
    }
    let mut rows = Vec::new();
    let mut features = Vec::new();
    for b in batches {
        let sch = b.schema();
        for r in 0..b.num_rows() {
            let mut props = Map::new();
            for (i, f) in sch.fields().iter().enumerate() {
                if is_geometry_field(f) || matches!(f.data_type(), DataType::Struct(_)) {
                    continue;
                }
                props.insert(f.name().clone(), cell(b.column(i).as_ref(), r));
            }
            if rows.len() < MAX_TABLE_ROWS {
                rows.push(Value::Object(props.clone()));
            }
            if let Some(gi) = geom_idx {
                let geom = b.column(gi).as_binary_opt::<i32>().and_then(|a| if a.is_null(r) { None } else { wkb::wkb_to_geojson(a.value(r)) });
                features.push(json!({"type": "Feature", "properties": props, "geometry": geom}));
            }
        }
    }
    let lineage: Vec<Value> = plan.lineage().into_iter().map(|(id, k)| json!({"id": &id[..12], "op": k.name()})).collect();
    let engine_ms = run.result.metrics.elapsed_ms.value.unwrap_or(0.0);
    let (dcount, dhash) = result_digest(batches);
    let mut response = json!({
        "placement": "server",
        "result_digest": {"count": dcount, "sha256": dhash},
        "plan_id": plan.plan_id(),
        "ir": plan,
        "physical": run.physical,
        "lineage": lineage,
        "columns": columns,
        "rows": rows,
        "rows_total": run.result.metrics.rows_returned.value.unwrap_or(0),
        "geojson": if geom_idx.is_some() { json!({"type": "FeatureCollection", "features": features}) } else { Value::Null },
        "metrics": run.result.metrics,
        "timing": {
            "total_ms": {"value": total_ms, "source": "engine_observed"},
            "plan_and_overhead_ms": {"value": (total_ms - engine_ms).max(0.0), "source": "derived"},
            "boundary_conversion_ms": {"value": t0.elapsed().as_secs_f64() * 1000.0, "source": "engine_observed"},
        },
    });
    // a metric predicate runs in the plane of a local metric CRS: say which one and how accurate it is (units are never implicit)
    let metric_predicate = plan.nodes.values().find_map(|node| match node {
        geo_ir::PlanNode::SpatialSemiJoin { predicate: geo_ir::SpatialPredicate::WithinDistance { distance }, .. }
        | geo_ir::PlanNode::SpatialJoin { predicate: geo_ir::SpatialPredicate::WithinDistance { distance }, .. } => distance.to_meters().ok(),
        _ => None,
    });
    if let Some(distance_m) = metric_predicate {
        let crs = plan.nodes.values().find_map(|node| match node {
            geo_ir::PlanNode::Transform { output_crs, .. } if output_crs.is_local_metric() => Some(output_crs.to_string()),
            _ => None,
        });
        response["metric_semantics"] = json!({
            "distance_m": distance_m,
            "crs": crs,
            "method": "ST_DWithin in the plane of a local UTM zone, chosen from the area of the query",
            "max_scale_error_pct": geo_types::MAX_SCALE_ERROR * 100.0,
        });
    }
    response
}

/// Cancels the running query if the request future is dropped (client
/// disconnect / client-side timeout) before the query finished.
struct CancelOnDrop(CancelToken, bool);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.1 {
            self.0.cancel();
        }
    }
}

/// Gateway-side description of a plan the planner placed in the browser: the
/// plan itself, the planner's explanation, and the authorized URLs of its sources.
fn client_response(st: &AppState, plan: &Plan, physical: geo_planner::PhysicalPlan) -> Value {
    let mut urls = Map::new();
    for n in plan.nodes.values() {
        if let geo_ir::PlanNode::Source { uri, .. } = n {
            if let Ok(a) = st.ctx().guard.authorize(uri) {
                if let Some(stem) = a.path().file_stem() {
                    urls.insert(uri.clone(), json!(format!("/api/files/{}", stem.to_string_lossy())));
                }
            }
        }
    }
    json!({
        "placement": "browser",
        "plan_id": plan.plan_id(),
        "ir": plan,
        "physical": physical,
        "source_urls": urls,
        "lineage": plan.lineage().into_iter().map(|(id, k)| json!({"id": &id[..12], "op": k.name()})).collect::<Vec<_>>(),
        "limits": {"max_rows": MAX_FEATURES, "timeout_ms": st.config.query_timeout.as_millis() as u64},
    })
}

fn planner_cfg(st: &AppState, o: &Opts) -> geo_planner::PlannerConfig {
    let mut cfg = st.rt.planner_config().clone();
    if let Some(b) = o.browser {
        cfg.allow_browser = b;
    }
    if let Some(mb) = o.browser_limit_mb {
        cfg.browser_max_input_bytes = Some(mb << 20);
    }
    cfg
}

fn plan_inputs(st: &AppState, o: &Opts) -> PlanInputs {
    let mut i = st.rt.state().snapshot();
    if let Some(n) = &o.network {
        i.network = n.clone();
    }
    i.browser_warm = o.warm;
    i
}

fn apply_engine(plan: &mut Plan, engine: &Option<String>) {
    if let Some(e) = engine {
        plan.hints.preferred_engine = Some(e.clone());
    }
}

/// Telemetry record of a finished run (estimator calibration + planner accuracy evaluation).
fn observation(physical: &PhysicalPlan, plan: &Plan, engine: &str, actual_rows: u64, actual_ms: f64, network: &str, fallback: bool) -> Observation {
    let shape = plan_shape(plan);
    let est = &physical.assessment.estimate;
    let censored = shape.limit.is_some_and(|l| actual_rows >= l);
    Observation {
        ts_ms: geo_planner::now_ms(),
        source: physical.scans.first().map(|s| source_key(&s.uri)).unwrap_or_default(),
        kind: shape.kind().into(),
        engine: engine.into(),
        raw_selected: est.raw_selected,
        total_rows: physical.scans.iter().map(|s| s.total_rows).sum(),
        est_rows: physical.result_rows_estimate,
        actual_rows,
        limit: shape.limit,
        censored,
        group_by: shape.group_by,
        est_ms: physical.candidates.iter().find(|c| c.engine.0 == engine).and_then(|c| c.score_ms),
        actual_ms,
        network: network.into(),
        fallback,
        layout: est.layout.clone(),
    }
}

async fn run_plan(st: Shared, plan: Plan, o: Opts) -> Result<Json<Value>, ApiError> {
    let permit = st
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| err(StatusCode::TOO_MANY_REQUESTS, "busy", "too many concurrent queries"))?;
    let mut ctx = st.ctx();
    let cancel = ctx.cancel.clone();
    let mut guard = CancelOnDrop(cancel, false);
    let st2 = st.clone();
    let cfg = planner_cfg(&st, &o);
    let inputs = plan_inputs(&st, &o);
    // The permit moves into the blocking task: the slot is only released when the
    // query has really finished (or was interrupted), not when the client left.
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let t0 = Instant::now();
        ctx.max_rows = MAX_FEATURES + 1;
        let net = inputs.network.name.clone();
        match st2.rt.dispatch_with_inputs(&cfg, &plan, &ctx, &inputs) {
            Ok(Dispatch::Executed(run)) => {
                let total = t0.elapsed().as_secs_f64() * 1000.0;
                let actual_rows = run.result.metrics.rows_returned.value.unwrap_or(0);
                let engine = run.physical.engine.to_string();
                let obs = observation(&run.physical, &plan, &engine, actual_rows, total, &net, false);
                let source = obs.source.clone();
                st2.rt.state().record(ExecutionReport { engine, source, outcome: Ok(()), observation: Some(obs) });
                Ok::<_, RuntimeError>(to_response(&plan, run, total))
            }
            Ok(Dispatch::Client { physical }) => Ok(client_response(&st2, &plan, physical)),
            // the runtime has already recorded the failure under the engine that ran (and failed)
            Err(e) => {
                Err(e)
            }
        }
    });
    let res = handle.await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    guard.1 = true; // finished: do not cancel
    Ok(Json(res))
}

/// Plan only (no execution): where would this query run, and why?
async fn plan_only(st: Shared, plan: Plan, o: Opts) -> Result<Json<Value>, ApiError> {
    let st2 = st.clone();
    let cfg = planner_cfg(&st, &o);
    let inputs = plan_inputs(&st, &o);
    let (plan, physical) = tokio::task::spawn_blocking(move || {
        let physical = st2.rt.plan_with_inputs(&cfg, &plan, &st2.ctx(), &inputs)?;
        Ok::<_, RuntimeError>((plan, physical))
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    let mut v = client_response(&st, &plan, physical);
    v["placement"] = v["physical"]["placement"].clone();
    Ok(Json(v))
}

async fn build_query_plan(st: &Shared, q: QueryReq) -> Result<(Plan, Opts), ApiError> {
    let with_uri = match q.kind.as_str() {
        "near" | "near_count" | "join_count_by" => {
            let with = q
                .with_req
                .as_ref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "with is required for this kind"))?;
            Some(authorized_source(st, &with.source)?.uri().to_string())
        }
        _ => {
            if q.with_req.is_some() {
                return Err(err(StatusCode::BAD_REQUEST, "bad_request", "with is only valid for near and join_count_by"));
            }
            None
        }
    };
    let a = authorized_source(st, &q.source)?;
    let st2 = st.clone();
    let uri = a.uri().to_string();
    let o = opts(q.browser, &q.network, &q.client, q.browser_limit_mb)?;
    let engine = q.engine.clone();
    let mut plan = tokio::task::spawn_blocking(move || {
        let ctx = st2.ctx();
        build_ir(&st2.rt, &uri, with_uri.as_deref(), &ctx, &q)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    apply_engine(&mut plan, &engine);
    Ok((plan, o))
}

fn query_req(payload: Result<Json<QueryReq>, axum::extract::rejection::JsonRejection>) -> Result<QueryReq, ApiError> {
    payload
        .map(|Json(q)| q)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.body_text()))
}

async fn query(
    State(st): State<Shared>,
    payload: Result<Json<QueryReq>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (plan, o) = build_query_plan(&st, query_req(payload)?).await?;
    run_plan(st, plan, o).await
}

async fn query_plan(
    State(st): State<Shared>,
    payload: Result<Json<QueryReq>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (plan, o) = build_query_plan(&st, query_req(payload)?).await?;
    plan_only(st, plan, o).await
}

#[derive(Deserialize)]
pub struct IrReq {
    ir: Value,
    engine: Option<String>,
    browser: Option<bool>,
    network: Option<String>,
    client: Option<ClientState>,
    browser_limit_mb: Option<u64>,
}

fn parse_ir(r: &IrReq) -> Result<Plan, ApiError> {
    let mut plan = Plan::from_json(&r.ir.to_string()).map_err(|e| err(StatusCode::BAD_REQUEST, "bad_ir", e.to_string()))?;
    apply_engine(&mut plan, &r.engine);
    Ok(plan)
}

async fn run_ir(State(st): State<Shared>, Json(r): Json<IrReq>) -> Result<Json<Value>, ApiError> {
    let plan = parse_ir(&r)?;
    let o = opts(r.browser, &r.network, &r.client, r.browser_limit_mb)?;
    run_plan(st, plan, o).await
}

async fn explain_ir(State(st): State<Shared>, Json(r): Json<IrReq>) -> Result<Json<Value>, ApiError> {
    let plan = parse_ir(&r)?;
    let o = opts(r.browser, &r.network, &r.client, r.browser_limit_mb)?;
    plan_only(st, plan, o).await
}

/// Parquet bytes for the browser engine, with HTTP range support. Authorized sources only.
async fn file(
    State(st): State<Shared>,
    UrlPath(id): UrlPath<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Response, ApiError> {
    use tower::ServiceExt;
    let a = authorized_source(&st, &id)?;
    let resp = tower_http::services::ServeFile::new(a.path())
        .oneshot(req)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "file", e.to_string()))?;
    let mut resp = resp.map(axum::body::Body::new).into_response();
    // the data directory can change under us and a measurement must hit the (simulated) network every time
    resp.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(resp)
}

async fn config(State(st): State<Shared>) -> Json<Value> {
    Json(json!({
        "planner": st.rt.planner_config(),
        "query_timeout_ms": st.config.query_timeout.as_millis() as u64,
        "engines": ["duckdb", "duckdb-wasm"],
        "network": st.rt.state().network(),
        "simulated_network": st.sim.as_ref().map(|s| s.profile.name.clone()),
    }))
}

// ---------- planner state: probes, health, telemetry, network (ADR-009/010) ----------

async fn put_probe(State(st): State<Shared>, Json(report): Json<ProbeReport>) -> Result<Json<Value>, ApiError> {
    if report.engine.is_empty() || report.engine.len() > 64 || report.checks.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "bad_probe", "malformed probe report"));
    }
    let engine = report.engine.clone();
    let verdict = st.rt.state().put_probe(report);
    Ok(Json(json!({"stored": true, "engine": engine, "version_changed": verdict.version_changed, "replaced_session": verdict.replaced_session})))
}

#[derive(Deserialize)]
pub struct EngineRef {
    engine: String,
}

async fn invalidate_probe(State(st): State<Shared>, Json(r): Json<EngineRef>) -> Json<Value> {
    Json(json!({"invalidated": st.rt.state().invalidate_probe(&r.engine)}))
}

#[derive(Deserialize)]
pub struct ClientReport {
    engine: String,
    source: String,
    ok: bool,
    message: Option<String>,
    kind: Option<String>,
    raw_selected: Option<f64>,
    total_rows: Option<u64>,
    est_rows: Option<u64>,
    actual_rows: Option<u64>,
    limit: Option<u64>,
    group_by: Option<String>,
    est_ms: Option<f64>,
    actual_ms: Option<f64>,
    fallback: Option<bool>,
    layout: Option<String>,
}

/// Save a device-check report (web/src/devicecheck.ts) next to the other measurement evidence. Opt-in, one file per check
/// (named after its start time, sanitised), bounded by the request body limit. Posted after every case, so a crash leaves a trace.
async fn device_check(State(st): State<Shared>, Json(report): Json<Value>) -> Result<Json<Value>, ApiError> {
    let Some(dir) = &st.devicecheck_dir else {
        return Err(err(StatusCode::NOT_FOUND, "not_enabled", "device-check reports are not enabled (start the gateway with --devicecheck-dir)"));
    };
    let started = report.get("started").and_then(|v| v.as_str()).unwrap_or("unknown");
    let name: String = started.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(40).collect();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "bad_report", "report has no usable start time"));
    }
    std::fs::create_dir_all(dir).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()))?;
    let path = dir.join(format!("device-{name}.json"));
    let text = serde_json::to_string_pretty(&report).map_err(|e| err(StatusCode::BAD_REQUEST, "bad_report", e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()))?;
    Ok(Json(json!({"saved": path.file_name().map(|n| n.to_string_lossy().to_string())})))
}

/// A client-side engine (the browser) reports how a run went: health, known incompatibilities, telemetry.
async fn client_report(State(st): State<Shared>, Json(r): Json<ClientReport>) -> Result<Json<Value>, ApiError> {
    if !valid_id(&r.source) || r.engine.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "bad_report", "malformed report"));
    }
    let state = st.rt.state();
    let (outcome, class) = if r.ok {
        (Ok(()), None)
    } else {
        let msg = r.message.clone().unwrap_or_default();
        let class = FailureClass::classify(&msg);
        (Err((class, msg)), Some(class))
    };
    let observation = (r.ok && r.actual_rows.is_some()).then(|| Observation {
        ts_ms: geo_planner::now_ms(),
        source: r.source.clone(),
        kind: r.kind.clone().unwrap_or_else(|| "features".into()),
        engine: r.engine.clone(),
        raw_selected: r.raw_selected.unwrap_or(0.0),
        total_rows: r.total_rows.unwrap_or(0),
        est_rows: r.est_rows.unwrap_or(0),
        actual_rows: r.actual_rows.unwrap_or(0),
        limit: r.limit,
        censored: r.limit.is_some_and(|l| r.actual_rows.unwrap_or(0) >= l) && r.group_by.is_none(),
        group_by: r.group_by.clone(),
        est_ms: r.est_ms,
        actual_ms: r.actual_ms.unwrap_or(0.0),
        network: state.network().name,
        fallback: r.fallback.unwrap_or(false),
        layout: r.layout.clone().unwrap_or_default(),
    });
    state.record(ExecutionReport { engine: r.engine.clone(), source: r.source.clone(), outcome, observation });
    Ok(Json(json!({"recorded": true, "class": class})))
}

async fn planner_state(State(st): State<Shared>) -> Json<Value> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut v = json!(st.rt.state().summary());
    v["histograms"] = json!({
        "ready": st.rt.state().histograms().len(),
        "pending": st.hist_pending.load(Relaxed),
        "notes": st.hist_notes.lock().map(|g| g.clone()).unwrap_or_default(),
    });
    v["postgis"] = st.rt.postgis_status();
    Json(v)
}

/// Build (or load from `.gf-cache/`) the spatial histogram of every source in the data directory, smallest first, on a
/// background thread: the planner estimates with footer statistics until a histogram is ready (estimator v3).
pub fn spawn_histogram_builder(st: Shared) {
    use geo_source_geoparquet::SpatialHistogram;
    use std::sync::atomic::Ordering::Relaxed;
    let mut by_id = std::collections::BTreeMap::new();
    for dir in &st.data_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()) {
            if path.extension().is_some_and(|extension| extension == "parquet") {
                if let Some(stem) = path.file_stem() {
                    by_id.entry(stem.to_string_lossy().into_owned()).or_insert(path);
                }
            }
        }
    }
    let mut files: Vec<PathBuf> = by_id.into_values().collect();
    files.sort_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    st.hist_pending.store(files.len(), Relaxed);
    std::thread::spawn(move || {
        let guard = st.ctx().guard;
        for p in files {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let note = (|| -> Result<String, String> {
                let a = guard.authorize(&p.to_string_lossy()).map_err(|_| "not authorized".to_string())?;
                let src = GeoParquetSource::open_authorized(&a).map_err(|e| e.to_string())?;
                let side = p.parent().unwrap_or_else(|| std::path::Path::new(".")).join(".gf-cache").join(format!("{stem}.hist.json"));
                if let Ok(text) = std::fs::read_to_string(&side) {
                    if let Ok(h) = serde_json::from_str::<SpatialHistogram>(&text) {
                        if h.matches(&src) {
                            st.rt.state().histograms().put(src.path(), h);
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
                st.rt.state().histograms().put(src.path(), h);
                Ok(format!("built in {ms} ms"))
            })();
            if let Ok(mut g) = st.hist_notes.lock() {
                g.insert(stem, note.unwrap_or_else(|e| format!("none: {e}")));
            }
            st.hist_pending.fetch_sub(1, Relaxed);
        }
    });
}

/// Development/test aid (localhost, no auth, like the rest of the v0 gateway): forget probes, health, telemetry.
async fn reset_state(State(st): State<Shared>) -> Json<Value> {
    st.rt.state().reset();
    Json(json!({"reset": true}))
}

async fn telemetry(State(st): State<Shared>) -> Json<Value> {
    Json(json!({"observations": st.rt.state().telemetry()}))
}

#[derive(Deserialize)]
pub struct NetworkReq {
    profile: String,
}

async fn set_network(State(st): State<Shared>, Json(r): Json<NetworkReq>) -> Result<Json<Value>, ApiError> {
    let p = NetworkProfile::by_name(&r.profile).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_network", "unknown network profile"))?;
    st.rt.state().set_network(p.clone());
    Ok(Json(json!({"network": p})))
}

/// Counts file-endpoint traffic and, when a network is simulated, delays every `/api` response (see [`SimNet`]).
async fn simulate_network(State(st): State<Shared>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    use axum::body::HttpBody;
    use std::sync::atomic::Ordering::Relaxed;
    let is_file = req.uri().path().starts_with("/api/files/");
    let resp = next.run(req).await;
    // streamed file bodies have no exact size hint: fall back to the Content-Length header (ServeFile sets it)
    let header_len = resp.headers().get(axum::http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let bytes = resp.body().size_hint().exact().or(header_len).unwrap_or(0);
    if is_file {
        st.file_requests.fetch_add(1, Relaxed);
        st.file_bytes.fetch_add(bytes, Relaxed);
    }
    let Some(sim) = st.sim.clone() else { return resp };
    let transfer = Duration::from_secs_f64(sim.profile.download_ms(bytes) / 1000.0);
    let latency = Duration::from_secs_f64(sim.profile.turn_ms() / 1000.0);
    let finish = {
        let mut free = sim.free_at.lock().await;
        let f = (*free).max(tokio::time::Instant::now()) + transfer;
        *free = f;
        f
    };
    tokio::time::sleep_until(finish + latency).await;
    resp
}

async fn stats(State(st): State<Shared>) -> Json<Value> {
    use std::sync::atomic::Ordering::Relaxed;
    Json(json!({"file_requests": st.file_requests.load(Relaxed), "file_bytes": st.file_bytes.load(Relaxed)}))
}

/// API routes only (static UI is mounted by the binary).
pub fn api_router(state: Shared) -> Router {
    let limit = state.config.max_body_bytes;
    Router::new()
        .route("/api/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/config", get(config))
        .route("/api/state", get(planner_state))
        .route("/api/stats", get(stats))
        .route("/api/telemetry", get(telemetry))
        .route("/api/state/reset", post(reset_state))
        .route("/api/network", post(set_network))
        .route("/api/probe", post(put_probe))
        .route("/api/probe/invalidate", post(invalidate_probe))
        .route("/api/report", post(client_report))
        .route("/api/devicecheck", post(device_check))
        .route("/api/catalog", get(catalog::catalog))
        .route("/api/rasters/{id}/{name}", get(catalog::raster))
        .route("/api/sources", get(sources))
        .route("/api/sources/{id}/row_groups", get(row_groups))
        .route("/api/files/{id}", get(file))
        .route("/api/query", post(query))
        .route("/api/query/plan", post(query_plan))
        .route("/api/ir/run", post(run_ir))
        .route("/api/ir/explain", post(explain_ir))
        .layer(axum::middleware::from_fn_with_state(state.clone(), simulate_network))
        .layer(RequestBodyLimitLayer::new(limit))
        .with_state(state)
}
