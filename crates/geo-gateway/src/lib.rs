//! HTTP gateway for the web UI.
//!
//! Trust model (v0): binds to 127.0.0.1 by default, no auth. Clients can only
//! submit *logical IR* or structured query templates — never SQL. Every source
//! goes through the `SourceAccessGuard` **before** any file is opened (listing,
//! planning, execution). Results are capped (rows, request size, concurrency)
//! and every query has an execution timeout enforced inside the engine; a
//! client disconnect cancels the running query. GeoJSON is generated here, at
//! the boundary, purely for map display.

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
use geo_runtime::{Dispatch, Runtime, RuntimeError};
use geo_source_geoparquet::GeoParquetSource;
use geo_types::*;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
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

pub struct AppState {
    pub rt: Runtime,
    pub data_dir: PathBuf,
    pub config: GatewayConfig,
    pub slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(rt: Runtime, data_dir: PathBuf, config: GatewayConfig) -> Self {
        let slots = Arc::new(Semaphore::new(config.max_concurrent_queries));
        AppState { rt, data_dir, config, slots }
    }

    fn ctx(&self) -> ExecutionContext {
        let mut ctx = ExecutionContext::new(vec![self.data_dir.clone()]);
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

fn err(status: StatusCode, code: &str, msg: impl Into<String>) -> ApiError {
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

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() < 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) && !id.contains("..")
}

/// Resolve a source id to an *authorized* source (guard first, then any I/O).
fn authorized_source(st: &AppState, id: &str) -> Result<AuthorizedSource, ApiError> {
    if !valid_id(id) {
        return Err(err(StatusCode::BAD_REQUEST, "bad_id", "invalid source id"));
    }
    let p = st.data_dir.join(format!("{id}.parquet"));
    st.ctx()
        .guard
        .authorize(&p.to_string_lossy())
        .map_err(|_| err(StatusCode::NOT_FOUND, "no_such_source", id))
}

fn list_sources(st: &AppState) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&st.data_dir) else { return out };
    let mut files: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "parquet")).collect();
    files.sort();
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
    op: String,
    value: Value,
}

#[derive(Deserialize)]
pub struct QueryReq {
    source: String,
    /// features | count_by | intersects_features
    kind: String,
    /// viewport [xmin, ymin, xmax, ymax] in `crs`
    bbox: [f64; 4],
    crs: String,
    filter: Option<FilterReq>,
    group_by: Option<String>,
    limit: Option<u64>,
    /// Engine preference ("duckdb" | "duckdb-wasm"); honored only if feasible.
    engine: Option<String>,
    /// Override `allow_browser` for this request.
    browser: Option<bool>,
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

fn build_ir(rt: &Runtime, uri: &str, ctx: &ExecutionContext, q: &QueryReq) -> Result<Plan, ApiError> {
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
        "features" | "count_by" => Node::BboxFilter { input: Box::new(src.clone()), geometry_column: geom_col.clone(), bbox: bb },
        _ => return Err(bad("unknown kind")),
    };
    if let Some(f) = &q.filter {
        let op = match f.op.as_str() {
            "eq" => CompareOp::Eq,
            "ne" => CompareOp::Ne,
            "lt" => CompareOp::Lt,
            "le" => CompareOp::Le,
            "gt" => CompareOp::Gt,
            "ge" => CompareOp::Ge,
            _ => return Err(bad("unknown filter op")),
        };
        node = Node::Filter {
            input: Box::new(node),
            predicate: Expr::Compare { column: f.column.clone(), op, value: json_to_literal(&f.value)? },
        };
    }
    let limit = q.limit.unwrap_or(20_000).min(MAX_FEATURES);
    node = match q.kind.as_str() {
        "count_by" => {
            let g = q.group_by.clone().ok_or_else(|| bad("group_by required"))?;
            Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate {
                        input: Box::new(node),
                        group_by: vec![g],
                        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
                    }),
                    keys: vec![SortKey { column: "count".into(), descending: true }],
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

fn node_schema(n: &Node) -> Result<SpatialSchema, ApiError> {
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
    json!({
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
    })
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

fn planner_cfg(st: &AppState, browser: Option<bool>) -> geo_planner::PlannerConfig {
    let mut cfg = st.rt.planner_config().clone();
    if let Some(b) = browser {
        cfg.allow_browser = b;
    }
    cfg
}

fn apply_engine(plan: &mut Plan, engine: &Option<String>) {
    if let Some(e) = engine {
        plan.hints.preferred_engine = Some(e.clone());
    }
}

async fn run_plan(st: Shared, plan: Plan, browser: Option<bool>) -> Result<Json<Value>, ApiError> {
    let permit = st
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| err(StatusCode::TOO_MANY_REQUESTS, "busy", "too many concurrent queries"))?;
    let mut ctx = st.ctx();
    let cancel = ctx.cancel.clone();
    let mut guard = CancelOnDrop(cancel, false);
    let st2 = st.clone();
    let cfg = planner_cfg(&st, browser);
    // The permit moves into the blocking task: the slot is only released when the
    // query has really finished (or was interrupted), not when the client left.
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let t0 = Instant::now();
        ctx.max_rows = MAX_FEATURES + 1;
        match st2.rt.dispatch_with(&cfg, &plan, &ctx)? {
            Dispatch::Executed(run) => {
                let total = t0.elapsed().as_secs_f64() * 1000.0;
                Ok::<_, RuntimeError>(to_response(&plan, run, total))
            }
            Dispatch::Client { physical } => Ok(client_response(&st2, &plan, physical)),
        }
    });
    let res = handle.await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    guard.1 = true; // finished: do not cancel
    Ok(Json(res))
}

/// Plan only (no execution): where would this query run, and why?
async fn plan_only(st: Shared, plan: Plan, browser: Option<bool>) -> Result<Json<Value>, ApiError> {
    let st2 = st.clone();
    let cfg = planner_cfg(&st, browser);
    let (plan, physical) = tokio::task::spawn_blocking(move || {
        let physical = st2.rt.plan_with(&cfg, &plan, &st2.ctx())?;
        Ok::<_, RuntimeError>((plan, physical))
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    let mut v = client_response(&st, &plan, physical);
    v["placement"] = v["physical"]["placement"].clone();
    Ok(Json(v))
}

async fn build_query_plan(st: &Shared, q: QueryReq) -> Result<(Plan, Option<bool>), ApiError> {
    let a = authorized_source(st, &q.source)?;
    let st2 = st.clone();
    let uri = a.uri().to_string();
    let (engine, browser) = (q.engine.clone(), q.browser);
    let mut plan = tokio::task::spawn_blocking(move || {
        let ctx = st2.ctx();
        build_ir(&st2.rt, &uri, &ctx, &q)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    apply_engine(&mut plan, &engine);
    Ok((plan, browser))
}

async fn query(State(st): State<Shared>, Json(q): Json<QueryReq>) -> Result<Json<Value>, ApiError> {
    let (plan, browser) = build_query_plan(&st, q).await?;
    run_plan(st, plan, browser).await
}

async fn query_plan(State(st): State<Shared>, Json(q): Json<QueryReq>) -> Result<Json<Value>, ApiError> {
    let (plan, browser) = build_query_plan(&st, q).await?;
    plan_only(st, plan, browser).await
}

#[derive(Deserialize)]
pub struct IrReq {
    ir: Value,
    engine: Option<String>,
    browser: Option<bool>,
}

fn parse_ir(r: &IrReq) -> Result<Plan, ApiError> {
    let mut plan = Plan::from_json(&r.ir.to_string()).map_err(|e| err(StatusCode::BAD_REQUEST, "bad_ir", e.to_string()))?;
    apply_engine(&mut plan, &r.engine);
    Ok(plan)
}

async fn run_ir(State(st): State<Shared>, Json(r): Json<IrReq>) -> Result<Json<Value>, ApiError> {
    let plan = parse_ir(&r)?;
    run_plan(st, plan, r.browser).await
}

async fn explain_ir(State(st): State<Shared>, Json(r): Json<IrReq>) -> Result<Json<Value>, ApiError> {
    let plan = parse_ir(&r)?;
    plan_only(st, plan, r.browser).await
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
    Ok(resp.map(axum::body::Body::new).into_response())
}

async fn config(State(st): State<Shared>) -> Json<Value> {
    Json(json!({
        "planner": st.rt.planner_config(),
        "query_timeout_ms": st.config.query_timeout.as_millis() as u64,
        "engines": ["duckdb", "duckdb-wasm"],
    }))
}

/// API routes only (static UI is mounted by the binary).
pub fn api_router(state: Shared) -> Router {
    let limit = state.config.max_body_bytes;
    Router::new()
        .route("/api/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/config", get(config))
        .route("/api/sources", get(sources))
        .route("/api/sources/{id}/row_groups", get(row_groups))
        .route("/api/files/{id}", get(file))
        .route("/api/query", post(query))
        .route("/api/query/plan", post(query_plan))
        .route("/api/ir/run", post(run_ir))
        .route("/api/ir/explain", post(explain_ir))
        .layer(RequestBodyLimitLayer::new(limit))
        .with_state(state)
}
