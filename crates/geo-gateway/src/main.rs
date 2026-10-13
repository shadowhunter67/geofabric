//! HTTP gateway for the web UI.
//!
//! Trust model (v0): binds to 127.0.0.1 by default, no auth. Clients can only
//! submit *logical IR* or structured query templates — never SQL. Source
//! files must resolve under the configured data directory; results are capped
//! (rows, request size, concurrency). GeoJSON is generated here, at the
//! boundary, purely for map display.

mod wkb;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{Array, RecordBatch};
use arrow_schema::DataType;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use geo_executor::{EngineError, ExecutionContext};
use geo_ir::*;
use geo_runtime::{Runtime, RuntimeError};
use geo_source_geoparquet::GeoParquetSource;
use geo_types::*;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};

const MAX_FEATURES: u64 = 100_000;
const MAX_TABLE_ROWS: usize = 1_000;

struct AppState {
    rt: Runtime,
    data_dir: PathBuf,
    slots: Semaphore,
}

type Shared = Arc<AppState>;

// ---------- errors ----------

struct ApiError(StatusCode, Value);

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
            RuntimeError::Plan(geo_planner::PlanError::Invalid(r)) => ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error": "invalid_plan", "message": "plan failed validation", "issues": r.issues}),
            ),
            RuntimeError::Plan(geo_planner::PlanError::TooExpensive(m)) => err(StatusCode::PAYLOAD_TOO_LARGE, "too_expensive", m),
            RuntimeError::Plan(p) => err(StatusCode::UNPROCESSABLE_ENTITY, "planning_failed", p.to_string()),
            RuntimeError::Execution(EngineError::AccessDenied(m)) => err(StatusCode::FORBIDDEN, "access_denied", m),
            RuntimeError::Execution(EngineError::LimitExceeded(m)) => err(StatusCode::PAYLOAD_TOO_LARGE, "limit_exceeded", m),
            RuntimeError::Execution(EngineError::Unsupported(m)) => err(StatusCode::UNPROCESSABLE_ENTITY, "unsupported", m),
            RuntimeError::Execution(EngineError::InvalidPlan(m)) => err(StatusCode::UNPROCESSABLE_ENTITY, "invalid_plan", m),
            RuntimeError::Execution(EngineError::Engine(m)) => err(StatusCode::INTERNAL_SERVER_ERROR, "engine_error", m),
            RuntimeError::Source(s) => err(StatusCode::BAD_REQUEST, "source_error", s.to_string()),
        }
    }
}

// ---------- sources ----------

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() < 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) && !id.contains("..")
}

fn source_path(st: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_id(id) {
        return Err(err(StatusCode::BAD_REQUEST, "bad_id", "invalid source id"));
    }
    let p = st.data_dir.join(format!("{id}.parquet"));
    if !p.is_file() {
        return Err(err(StatusCode::NOT_FOUND, "no_such_source", id));
    }
    Ok(p)
}

fn list_sources(st: &AppState) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&st.data_dir) else { return out };
    let mut files: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "parquet")).collect();
    files.sort();
    for p in files {
        let id = p.file_stem().unwrap().to_string_lossy().to_string();
        match GeoParquetSource::open(&p) {
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
    let p = source_path(&st, &id)?;
    let s = GeoParquetSource::open(&p).map_err(|e| err(StatusCode::BAD_REQUEST, "source_error", e.to_string()))?;
    let i = s.info();
    Ok(Json(json!({"crs": i.crs.to_string(), "row_groups": i.row_groups.iter().map(|r| json!({
        "index": r.index, "rows": r.rows, "bytes": r.compressed_bytes, "bbox": r.bbox})).collect::<Vec<_>>()})))
}

// ---------- query templates -> IR ----------

#[derive(Deserialize)]
struct FilterReq {
    column: String,
    op: String,
    value: Value,
}

#[derive(Deserialize)]
struct QueryReq {
    source: String,
    /// features | count_by | intersects_features
    kind: String,
    /// viewport [xmin, ymin, xmax, ymax] in `crs`
    bbox: [f64; 4],
    crs: String,
    filter: Option<FilterReq>,
    group_by: Option<String>,
    limit: Option<u64>,
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

fn build_ir(rt: &Runtime, path: &std::path::Path, q: &QueryReq) -> Result<Plan, ApiError> {
    let bad = |m: &str| err(StatusCode::BAD_REQUEST, "bad_request", m.to_string());
    let src = rt.open_source(path)?;
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
                .filter(|f| !matches!(&f.data_type, FieldType::Other(t) if t == "struct"))
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

fn to_response(plan: &Plan, run: geo_runtime::RunOutput, total_ms: f64, plan_ms: f64) -> Value {
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
    let lineage: Vec<Value> = plan.root.lineage().into_iter().map(|(id, k)| json!({"id": &id[..12], "op": k.name()})).collect();
    json!({
        "plan_id": plan.plan_id(),
        "ir": plan,
        "physical": run.physical,
        "lineage": lineage,
        "columns": columns,
        "rows": rows,
        "rows_total": run.result.metrics.rows_returned,
        "geojson": if geom_idx.is_some() { json!({"type": "FeatureCollection", "features": features}) } else { Value::Null },
        "metrics": run.result.metrics,
        "timing": {"plan_and_overhead_ms": plan_ms, "total_ms": total_ms, "boundary_conversion_ms": t0.elapsed().as_secs_f64() * 1000.0},
    })
}

async fn run_plan(st: Shared, plan: Plan) -> Result<Json<Value>, ApiError> {
    let _permit = st
        .slots
        .try_acquire()
        .map_err(|_| err(StatusCode::TOO_MANY_REQUESTS, "busy", "too many concurrent queries"))?;
    let st2 = st.clone();
    let res = tokio::task::spawn_blocking(move || {
        let t0 = Instant::now();
        let mut ctx = ExecutionContext::new(vec![st2.data_dir.clone()]);
        ctx.max_rows = MAX_FEATURES + 1;
        let run = st2.rt.run(&plan, &ctx)?;
        let total = t0.elapsed().as_secs_f64() * 1000.0;
        // planning + overhead = everything that was not engine execution
        let plan_ms = (total - run.result.metrics.elapsed_ms).max(0.0);
        Ok::<_, RuntimeError>(to_response(&plan, run, total, plan_ms))
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    Ok(Json(res))
}

async fn query(State(st): State<Shared>, Json(q): Json<QueryReq>) -> Result<Json<Value>, ApiError> {
    let p = source_path(&st, &q.source)?;
    let plan = build_ir(&st.rt, &p, &q)?;
    run_plan(st, plan).await
}

#[derive(Deserialize)]
struct IrReq {
    ir: Value,
}

async fn run_ir(State(st): State<Shared>, Json(r): Json<IrReq>) -> Result<Json<Value>, ApiError> {
    let plan = Plan::from_json(&r.ir.to_string()).map_err(|e| err(StatusCode::BAD_REQUEST, "bad_ir", e.to_string()))?;
    run_plan(st, plan).await
}

async fn explain_ir(State(st): State<Shared>, Json(r): Json<IrReq>) -> Result<Json<Value>, ApiError> {
    let plan = Plan::from_json(&r.ir.to_string()).map_err(|e| err(StatusCode::BAD_REQUEST, "bad_ir", e.to_string()))?;
    let st2 = st.clone();
    let phys = tokio::task::spawn_blocking(move || {
        let ctx = ExecutionContext::new(vec![st2.data_dir.clone()]);
        st2.rt.plan(&plan, &ctx)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "join", e.to_string()))??;
    Ok(Json(json!({"physical": phys})))
}

fn arg(args: &[String], name: &str, default: &str) -> String {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = arg(&args, "--data", "benchmarks/data");
    let port = arg(&args, "--port", "8787");
    let host = arg(&args, "--host", "127.0.0.1");
    let web = arg(&args, "--web", "web/dist");
    let data_dir = std::fs::canonicalize(&data).unwrap_or_else(|e| panic!("data dir '{data}': {e}"));
    let state = Arc::new(AppState {
        rt: tokio::task::spawn_blocking(Runtime::with_duckdb).await.unwrap(),
        data_dir,
        slots: Semaphore::new(2),
    });
    let api = Router::new()
        .route("/api/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/sources", get(sources))
        .route("/api/sources/{id}/row_groups", get(row_groups))
        .route("/api/query", post(query))
        .route("/api/ir/run", post(run_ir))
        .route("/api/ir/explain", post(explain_ir))
        .layer(RequestBodyLimitLayer::new(1 << 20))
        .with_state(state);
    let index = ServeFile::new(format!("{web}/index.html"));
    let app = api.fallback_service(ServeDir::new(&web).fallback(index));
    let addr = format!("{host}:{port}");
    let l = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("geo-gateway listening on http://{addr} (data: {data}, web: {web})");
    axum::serve(l, app).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await.unwrap();
}
