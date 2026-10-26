//! PostGIS COST MODEL EVIDENCE: the benchmark corpus harness (opt-in, live; a skip is not evidence).
//!
//! Runs every case of a corpus file (benchmarks/make_postgis_cost_cases.py) on DuckDB and on PostGIS through the real runtime and records, per case:
//! the planner's own view (plan-time workload features, candidate costs, chosen engine), both engines' timings (first run + warm repetitions,
//! engine order alternated), whether the two results are identical (cell by cell, geometries coordinate by coordinate), and for PostGIS the
//! `EXPLAIN (ANALYZE, BUFFERS)` text. The harness measures; it never decides anything and no model is read from or fed back into it.
//!
//!   GEOFABRIC_POSTGIS_ENABLED=1 GEOFABRIC_POSTGIS_URL=... GEOFABRIC_POSTGIS_SOURCES=<catalog json>
//!   GF_COST_CASES=<cases.json> GF_COST_OUT=<results.jsonl>   (resumable)   [GF_COST_START GF_COST_COUNT GF_COST_TIMEOUT_S GF_COST_EXPLAIN=0]
//!
//! Cache terminology (no "cold cache" is claimed anywhere): `first_ms` of the first case after a PostgreSQL restart is process-cold; the OS file
//! cache is not controlled. `warm_ms` are repetitions after the first run.
#![cfg(feature = "postgis-live")]

use arrow_array::cast::AsArray;
use arrow_array::{Array, RecordBatch};
use arrow_schema::DataType;
use geo_engine_duckdb::{DuckDbConfig, DuckDbEngine};
use geo_executor::{result_digest, ExecutionContext, ExecutionEngine};
use geo_ir::*;
use geo_runtime::Runtime;
use geo_types::*;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    /// WKB parsed to a flat list: [type, n, coords...] (recursive for rings/parts); compared coordinate by coordinate
    Geom(Vec<f64>),
}

fn wkb(b: &[u8], pos: &mut usize, out: &mut Vec<f64>) {
    let little = b[*pos] == 1;
    *pos += 1;
    let rd32 = |p: &mut usize| {
        let v = if little { u32::from_le_bytes(b[*p..*p + 4].try_into().unwrap()) } else { u32::from_be_bytes(b[*p..*p + 4].try_into().unwrap()) };
        *p += 4;
        v
    };
    let rd64 = |p: &mut usize| {
        let v = if little { f64::from_le_bytes(b[*p..*p + 8].try_into().unwrap()) } else { f64::from_be_bytes(b[*p..*p + 8].try_into().unwrap()) };
        *p += 8;
        v
    };
    let t = rd32(pos) & 0xff; // OGC 2D types 1..7 (ISO flags are not produced by either engine for 2D data)
    out.push(t as f64);
    match t {
        1 => {
            out.push(rd64(pos));
            out.push(rd64(pos));
        }
        2 => {
            let n = rd32(pos);
            out.push(n as f64);
            for _ in 0..n * 2 {
                out.push(rd64(pos));
            }
        }
        3 => {
            let rings = rd32(pos);
            out.push(rings as f64);
            for _ in 0..rings {
                let n = rd32(pos);
                out.push(n as f64);
                for _ in 0..n * 2 {
                    out.push(rd64(pos));
                }
            }
        }
        4..=7 => {
            let parts = rd32(pos);
            out.push(parts as f64);
            for _ in 0..parts {
                wkb(b, pos, out);
            }
        }
        other => panic!("WKB geometry type {other} is not handled by the comparator"),
    }
}

fn rows_of(batches: &[RecordBatch]) -> (Vec<String>, Vec<Vec<Cell>>) {
    let mut names = Vec::new();
    let mut rows = Vec::new();
    for b in batches {
        let schema = b.schema();
        if names.is_empty() {
            names = schema.fields().iter().map(|f| f.name().clone()).collect();
        }
        for r in 0..b.num_rows() {
            let mut row = Vec::new();
            for (ci, col) in b.columns().iter().enumerate() {
                let field = schema.field(ci);
                row.push(if col.is_null(r) {
                    Cell::Null
                } else {
                    match col.data_type() {
                        DataType::Int64 => Cell::Int(col.as_primitive::<arrow_array::types::Int64Type>().value(r)),
                        DataType::Int32 => Cell::Int(col.as_primitive::<arrow_array::types::Int32Type>().value(r) as i64),
                        DataType::Float64 => Cell::Float(col.as_primitive::<arrow_array::types::Float64Type>().value(r)),
                        DataType::Float32 => Cell::Float(col.as_primitive::<arrow_array::types::Float32Type>().value(r) as f64),
                        DataType::Boolean => Cell::Bool(col.as_boolean().value(r)),
                        DataType::Utf8 => Cell::Text(col.as_string::<i32>().value(r).to_string()),
                        DataType::Binary => {
                            assert_eq!(field.metadata().get("ARROW:extension:name").map(String::as_str), Some("geoarrow.wkb"), "binary column {} is not tagged as WKB", field.name());
                            let b = col.as_binary::<i32>().value(r);
                            let (mut out, mut pos) = (Vec::new(), 0usize);
                            wkb(b, &mut pos, &mut out);
                            Cell::Geom(out)
                        }
                        other => panic!("unhandled column type {other}"),
                    }
                });
            }
            rows.push(row);
        }
    }
    (names, rows)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * a.abs().max(b.abs()).max(1.0)
}

fn cell_eq(a: &Cell, b: &Cell, geom_tol: f64) -> bool {
    match (a, b) {
        (Cell::Float(x), Cell::Float(y)) => close(*x, *y, 1e-9),
        (Cell::Geom(x), Cell::Geom(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (p - q).abs() <= geom_tol),
        _ => a == b,
    }
}


// ------------------------------------------------------------------------------------------------------------------------------ the harness

struct Env {
    rt: Runtime,
    ctx: ExecutionContext,
    uris: BTreeMap<String, String>,
}

fn env() -> Option<Env> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(_), Some(sources_path)) = (get("GF_COST_CASES"), get("GEOFABRIC_POSTGIS_SOURCES")) else {
        eprintln!("SKIP: set GF_COST_CASES=<cases.json> (and GEOFABRIC_POSTGIS_*): the cost benchmark is opt-in and proved nothing");
        return None;
    };
    let catalog: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&sources_path).unwrap()).unwrap();
    let uris: BTreeMap<String, String> = catalog.iter().filter_map(|s| Some((s["relation"].as_str()?.to_string(), s["logical_uri"].as_str()?.to_string()))).collect();
    let mut rt = Runtime::empty();
    assert!(rt.register_postgis_from_env().expect("postgis configuration"), "PostGIS is not enabled in the environment");
    rt.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
    rt.probe_postgis();
    assert_eq!(rt.postgis_status()["probe"]["ok"], true, "the live probe must pass first");
    let roots: Vec<std::path::PathBuf> = uris.values().map(|u| std::path::Path::new(u).parent().unwrap().to_path_buf()).collect();
    let mut ctx = ExecutionContext::new(roots);
    ctx.timeout = std::time::Duration::from_secs(get("GF_COST_TIMEOUT_S").and_then(|v| v.parse().ok()).unwrap_or(60));
    // The product's planner always runs with spatial histograms + column statistics (the gateway builds them in the background). The earlier
    // evidence (up to 2026-10-26) was planned WITHOUT them, i.e. with the footer-only fallback; from here on the harness measures what ships.
    // GF_COST_NO_HIST=1 reproduces the old behaviour.
    if get("GF_COST_NO_HIST").as_deref() != Some("1") {
        for uri in uris.values() {
            let t0 = std::time::Instant::now();
            match rt.ensure_histogram(std::path::Path::new(uri), ctx.guard.as_ref()) {
                Ok(note) => eprintln!("histogram {:<40} {note} ({:.1} s)", std::path::Path::new(uri).file_name().unwrap().to_string_lossy(), t0.elapsed().as_secs_f64()),
                Err(e) => eprintln!("histogram {uri}: NOT available ({e}); estimates fall back to footer statistics"),
            }
        }
    }
    Some(Env { rt, ctx, uris })
}

fn src(e: &Env, id: &str) -> Node {
    let uri = e.uris.get(id).unwrap_or_else(|| panic!("no source {id}"));
    e.rt.open_source(uri, &e.ctx).unwrap_or_else(|err| panic!("open {id}: {err}"))
}

fn bb(v: &serde_json::Value) -> BoundingBox {
    let a: Vec<f64> = v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    BoundingBox::new(a[0], a[1], a[2], a[3], Crs::OgcCrs84).unwrap()
}

fn windowed(n: Node, b: &BoundingBox) -> Node {
    Node::BboxFilter { input: Box::new(n), geometry_column: "geometry".into(), bbox: b.clone() }
}

fn sort_id(input: Node) -> Node {
    Node::Sort { input: Box::new(input), keys: vec![SortKey { column: "id".into(), descending: false }] }
}

fn project(input: Node, cols: &[&str]) -> Node {
    Node::Project { input: Box::new(input), columns: cols.iter().map(|s| s.to_string()).collect() }
}

/// (plan, geometry tolerance) for one case. Requests go through the canonical builder (`geo-plan`), exactly as the gateway does.
fn build(e: &Env, case: &serde_json::Value) -> Result<(Plan, f64), String> {
    if case["kind"] == "request" {
        let req: geo_plan::PlanRequest = serde_json::from_value(case["request"].clone()).map_err(|x| x.to_string())?;
        let mut sources = geo_plan::Sources::new();
        sources.insert(req.source.clone(), src(e, &req.source));
        if let Some(w) = &req.with_request {
            sources.insert(w.source.clone(), src(e, &w.source));
        }
        return Ok((geo_plan::build_plan(&req, &sources).map_err(|x| x.to_string())?, 1e-9));
    }
    let p = &case["params"];
    let b = bb(&p["bbox"]);
    let bld = src(e, "buildings");
    let w = windowed(bld.clone(), &b);
    let cols = ["id", "class", "height", "geometry"];
    let node = match case["name"].as_str().unwrap() {
        "semi_intersects_admin" => sort_id(project(
            Node::SpatialSemiJoin { left: Box::new(w), right: Box::new(windowed(src(e, "admin"), &b)), left_column: "geometry".into(), right_column: "geometry".into(), predicate: SpatialPredicate::Intersects },
            &["id", "class", "geometry"],
        )),
        "sort_limit" => Node::Limit {
            input: Box::new(Node::Sort {
                input: Box::new(project(w, &cols)),
                keys: vec![SortKey { column: "height".into(), descending: p["desc"].as_bool().unwrap_or(true) }, SortKey { column: "id".into(), descending: false }],
            }),
            n: p["n"].as_u64().unwrap(),
        },
        "agg_stats" => Node::Sort {
            input: Box::new(Node::Aggregate {
                input: Box::new(w),
                group_by: vec!["class".into()],
                aggregates: vec![
                    AggExpr { func: AggFunc::Min, column: Some("height".into()), alias: "lo".into() },
                    AggExpr { func: AggFunc::Max, column: Some("height".into()), alias: "hi".into() },
                    AggExpr { func: AggFunc::Avg, column: Some("height".into()), alias: "mean".into() },
                    AggExpr { func: AggFunc::Count, column: None, alias: "n".into() },
                ],
            }),
            keys: vec![SortKey { column: "class".into(), descending: false }],
        },
        "near_ids" => {
            // the semi join EXACTLY as geo-plan's `near` builds it (degree prefilter on the right, metric copy on the left), projecting id/class/geometry
            let dist = p["distance_m"].as_f64().unwrap();
            let dlat = dist / 110_574.0 * 1.02;
            let lat_max = b.max_y().abs().max(b.min_y().abs()).min(89.0) + dist / 110_574.0;
            let dlon = dist / (111_320.0 * lat_max.to_radians().cos().max(0.01)) * 1.02;
            let expanded = BoundingBox::new(b.min_x() - dlon, b.min_y() - dlat, b.max_x() + dlon, b.max_y() + dlat, Crs::OgcCrs84).unwrap();
            let mut right = windowed(src(e, "transport"), &expanded);
            if let Some(vals) = p["road_classes"].as_array() {
                right = Node::Filter { input: Box::new(right), predicate: Expr::In { column: "class".into(), values: vals.iter().map(|v| Literal::Str(v.as_str().unwrap().into())).collect() } };
            }
            let right = Node::Transform {
                input: Box::new(Node::Project { input: Box::new(right), columns: vec!["geometry".into()] }),
                geometry_column: "geometry".into(),
                input_crs: Crs::OgcCrs84,
                output_crs: Crs::Epsg(32648),
                output_column: "geometry".into(),
            };
            let left = Node::Transform { input: Box::new(w), geometry_column: "geometry".into(), input_crs: Crs::OgcCrs84, output_crs: Crs::Epsg(32648), output_column: "geometry_metric".into() };
            sort_id(project(
                Node::SpatialSemiJoin {
                    left: Box::new(left),
                    right: Box::new(right),
                    left_column: "geometry_metric".into(),
                    right_column: "geometry".into(),
                    predicate: SpatialPredicate::WithinDistance { distance: Distance::new(dist, DistanceUnit::Meter).unwrap() },
                },
                &["id", "class", "geometry"],
            ))
        }
        "transform_col" => sort_id(project(
            Node::Transform { input: Box::new(w), geometry_column: "geometry".into(), input_crs: Crs::OgcCrs84, output_crs: Crs::Epsg(32648), output_column: "geometry_metric".into() },
            &["id", "geometry", "geometry_metric"],
        )),
        "limit_unordered" => Node::Limit { input: Box::new(project(w, &cols)), n: p["n"].as_u64().unwrap() },
        other => return Err(format!("unknown template {other}")),
    };
    let tol = if case["name"] == "transform_col" { 1e-6 } else { 1e-9 };
    Ok((Plan::new(node), tol))
}

fn has_node(plan: &Plan, f: impl Fn(&PlanNode) -> bool) -> bool {
    plan.nodes.values().any(f)
}

#[derive(Default)]
struct Timing {
    status: String,
    message: String,
    first_ms: f64,
    warm_ms: Vec<f64>,
    rows: usize,
    digest: String,
}

/// Run `plan` on `engine`: first execution (process-cold for the very first case after a restart; otherwise "first touch of this case") and warm repetitions.
fn time_engine(e: &Env, plan: &Plan, engine: &str, keep: &mut Option<Vec<RecordBatch>>) -> Timing {
    let mut t = Timing { status: "ok".into(), ..Default::default() };
    let once = |_: ()| -> Result<(Vec<RecordBatch>, f64), String> {
        // the engine's own `execute` (authorization, lowering, bounded execution, Arrow result): the same for both engines, and independent of the
        // planner's health tracker (three timeouts would otherwise mark an engine unhealthy and silently change what is measured)
        let t0 = std::time::Instant::now();
        let r = if engine == "postgis" {
            e.rt.postgis().expect("postgis").execute(plan, &e.ctx)
        } else {
            e.rt.engines().iter().find(|x| x.id().0 == engine).expect("engine").execute(plan, &e.ctx)
        };
        match r {
            Ok(out) => Ok((out.batches, t0.elapsed().as_secs_f64() * 1000.0)),
            Err(err) => Err(err.to_string()),
        }
    };
    match once(()) {
        Ok((batches, ms)) => {
            t.first_ms = ms;
            t.rows = batches.iter().map(|b| b.num_rows()).sum();
            t.digest = result_digest(&batches).1;
            *keep = Some(batches);
        }
        Err(msg) => {
            let low = msg.to_lowercase();
            t.status = if low.contains("timeout") { "timeout" } else if low.contains("cancel") { "cancelled" } else if low.contains("limit") { "limit_exceeded" } else { "error" }.into();
            t.message = msg.chars().take(300).collect();
            return t;
        }
    }
    let reps = if t.first_ms < 10_000.0 { 3 } else if t.first_ms < 30_000.0 { 1 } else { 0 };
    for _ in 0..reps {
        match once(()) {
            Ok((b, ms)) => {
                if b.iter().map(|x| x.num_rows()).sum::<usize>() != t.rows {
                    t.status = "unstable_rowcount".into();
                }
                t.warm_ms.push(ms);
            }
            Err(msg) => {
                t.message = format!("warm repetition failed: {}", msg.chars().take(200).collect::<String>());
                break;
            }
        }
    }
    t
}

fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(s[s.len() / 2])
}

fn timing_json(t: &Timing) -> serde_json::Value {
    serde_json::json!({
        "status": t.status, "message": t.message, "first_ms": t.first_ms, "warm_ms": t.warm_ms, "warm_median_ms": median(&t.warm_ms), "rows": t.rows, "digest": t.digest,
        // the figure the model is fitted on: the warm median when there is one, else the first run (very slow cases are run once)
        "model_ms": median(&t.warm_ms).unwrap_or(t.first_ms),
    })
}

fn compare(plan: &Plan, a: &[RecordBatch], b: &[RecordBatch], geom_tol: f64) -> Vec<String> {
    let (names_a, mut ra) = rows_of(a);
    let (names_b, mut rb) = rows_of(b);
    let mut problems = Vec::new();
    if names_a != names_b && !(ra.is_empty() && rb.is_empty()) {
        problems.push(format!("columns differ: {names_a:?} vs {names_b:?}"));
    }
    if ra.len() != rb.len() {
        problems.push(format!("row counts differ: duckdb {} vs postgis {}", ra.len(), rb.len()));
        return problems;
    }
    // an unordered LIMIT has no defined row set; everything else is compared cell by cell
    let has_sort = has_node(plan, |n| matches!(n, PlanNode::Sort { .. }));
    let has_limit = has_node(plan, |n| matches!(n, PlanNode::Limit { .. }));
    if has_limit && !has_sort {
        return problems;
    }
    if !has_sort {
        let key = |r: &Vec<Cell>| format!("{:?}", r.first());
        ra.sort_by_key(key);
        rb.sort_by_key(key);
    }
    for (i, (x, y)) in ra.iter().zip(&rb).enumerate() {
        if x.len() != y.len() || !x.iter().zip(y).all(|(p, q)| cell_eq(p, q, geom_tol)) {
            problems.push(format!("row {i} differs:\n  duckdb  {x:?}\n  postgis {y:?}"));
            break;
        }
    }
    problems
}

/// Plan-only experiments: PostGIS wrapped so that the classes in GF_COST_FORCE_VALIDATED count as Validated. The PLANNER (policy, R5d, R5e, risk, scoring)
/// is the real one; only the validation level is pretended, to answer "what would the planner choose if this class were validated?". Never used to run.
struct ForcedValidation {
    inner: std::sync::Arc<geo_engine_postgis::PostGisEngine>,
    classes: BTreeSet<String>,
}

impl geo_executor::EngineDescriptor for ForcedValidation {
    fn id(&self) -> geo_executor::EngineId {
        geo_executor::EngineId::new("postgis")
    }
    fn capabilities(&self) -> geo_executor::Capabilities {
        self.inner.capabilities()
    }
    fn source_residency(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<(), String> {
        self.inner.source_residency(plan, ctx)
    }
    fn calibration(&self) -> geo_executor::EngineCalibration {
        self.inner.calibration()
    }
    fn cost_estimate(&self, plan: &Plan, f: &geo_executor::WorkloadFeatures) -> Option<geo_executor::EngineCostEstimate> {
        let mut c = self.inner.cost_estimate(plan, f)?;
        if self.classes.contains(&c.workload_class) && c.index_state == "available" {
            c.validation = geo_executor::ModelValidation::Validated;
        }
        Some(c)
    }
    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<geo_executor::Estimate, geo_executor::EngineError> {
        self.inner.estimate(plan, ctx)
    }
    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<geo_executor::ExplainStep>, geo_executor::EngineError> {
        self.inner.explain(plan, ctx)
    }
}

fn forced_runtime(e: &Env, classes: BTreeSet<String>) -> Runtime {
    let mut rt = Runtime::empty();
    rt.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
    rt.register_client(Box::new(ForcedValidation { inner: e.rt.postgis().expect("postgis").clone(), classes }));
    for uri in e.uris.values() {
        let _ = rt.ensure_histogram(std::path::Path::new(uri), e.ctx.guard.as_ref());
    }
    rt
}

fn planner_json(e: &Env, plan: &Plan) -> serde_json::Value {
    planner_json_with(&e.rt, e, plan)
}

fn planner_json_with(rt: &Runtime, e: &Env, plan: &Plan) -> serde_json::Value {
    match rt.plan(plan, &e.ctx) {
        Ok(p) => serde_json::json!({
            "chosen": p.engine.0,
            "candidates": p.candidates.iter().map(|c| serde_json::json!({
                "engine": c.engine.0, "feasible": c.feasible, "score_ms": c.score_ms, "calibration": c.calibration,
                "rejected_because": c.rejected_because, "cost_model": c.cost_model,
            })).collect::<Vec<_>>(),
            "policy": p.assessment.policy,
            "uncertainty": p.assessment.uncertainty,
            "workload": p.workload,
            "estimate_rows": p.result_rows_estimate,
        }),
        Err(err) => serde_json::json!({"error": err.to_string()}),
    }
}

fn done_ids(path: &str) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter_map(|v| v["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// GF_COST_CASES=<cases.json> GF_COST_OUT=<results.jsonl> [GF_COST_START=<n> GF_COST_COUNT=<n>] [GF_COST_EXPLAIN=0]
/// Appends one JSON line per finished case, skips ids already in the file (a long run can be stopped and resumed).
#[test]
fn postgis_cost_corpus() {
    let Some(e) = env() else { return };
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(get("GF_COST_CASES").unwrap()).unwrap()).unwrap();
    let out_path = get("GF_COST_OUT").expect("GF_COST_OUT");
    let done = done_ids(&out_path);
    let start: usize = get("GF_COST_START").and_then(|v| v.parse().ok()).unwrap_or(0);
    let count: usize = get("GF_COST_COUNT").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let explain = get("GF_COST_EXPLAIN").as_deref() != Some("0");
    // GF_COST_PLAN_ONLY=1: do not execute anything; re-plan every case with the CURRENT planner/models/validation and write {id, planner} lines.
    // Used to evaluate a model change on already measured cases (the timings do not depend on the planner) and to confirm final planner decisions.
    let plan_only = get("GF_COST_PLAN_ONLY").as_deref() == Some("1");
    // GF_COST_FORCE_VALIDATED=class,class: plan as if these PostGIS classes were validated (plan-only experiments)
    let forced = get("GF_COST_FORCE_VALIDATED").filter(|_| plan_only).map(|v| forced_runtime(&e, v.split(',').map(str::to_string).collect()));
    // GF_COST_ONLY=<id,id> runs just those cases; GF_COST_ENGINES=duckdb|postgis restricts the engines (isolating a case that kills the process);
    // GF_COST_SKIP=<id,id> leaves cases out and GF_COST_NOTE_CRASH=<id> records one as a crash instead of silently dropping it
    let only: BTreeSet<String> = get("GF_COST_ONLY").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let skip: BTreeSet<String> = get("GF_COST_SKIP").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let engines: Vec<String> = get("GF_COST_ENGINES").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_else(|| vec!["duckdb".into(), "postgis".into()]);
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).unwrap();
    let mut mismatches = Vec::new();
    for (i, case) in doc["cases"].as_array().unwrap().iter().enumerate().skip(start).take(count) {
        let id = case["id"].as_str().unwrap().to_string();
        if done.contains(&id) || skip.contains(&id) || (!only.is_empty() && !only.contains(&id)) {
            continue;
        }
        let (plan, tol) = match build(&e, case) {
            Ok(x) => x,
            Err(msg) => {
                writeln!(out, "{}", serde_json::json!({"id": id, "template": case["template"], "build_error": msg})).unwrap();
                continue;
            }
        };
        if plan_only {
            writeln!(out, "{}", serde_json::json!({"id": id, "planner": planner_json_with(forced.as_ref().unwrap_or(&e.rt), &e, &plan)})).unwrap();
            continue;
        }
        // alternate / randomize the engine order: a fixed order would hand one engine the warmer OS cache every time
        let order: [&str; 2] = if (i * 2654435761usize) >> 7 & 1 == 0 { ["duckdb", "postgis"] } else { ["postgis", "duckdb"] };
        let planner = planner_json(&e, &plan);
        let (mut kd, mut kp) = (None, None);
        let (mut td, mut tp) = (Timing::default(), Timing::default());
        for engine in order {
            if !engines.iter().any(|x| x == engine) {
                continue;
            }
            if engine == "duckdb" {
                td = time_engine(&e, &plan, "duckdb", &mut kd);
            } else {
                tp = time_engine(&e, &plan, "postgis", &mut kp);
            }
        }
        let problems = match (&kd, &kp) {
            (Some(a), Some(b)) => compare(&plan, a, b, tol),
            _ => vec![],
        };
        let identical = kd.is_some() && kp.is_some() && problems.is_empty();
        let explain_lines: serde_json::Value = if explain && tp.status == "ok" {
            match e.rt.postgis().unwrap().explain_analyze(&plan, &e.ctx) {
                Ok(l) => serde_json::json!(l.into_iter().take(160).collect::<Vec<_>>()),
                Err(err) => serde_json::json!({"error": err.to_string()}),
            }
        } else {
            serde_json::Value::Null
        };
        eprintln!(
            "{:<10} {:<22} duckdb {:>9.0} ms [{}]  postgis {:>9.0} ms [{}]  rows {:>7}  {}",
            id, case["template"].as_str().unwrap(), td.warm_ms.first().copied().unwrap_or(td.first_ms), td.status, tp.warm_ms.first().copied().unwrap_or(tp.first_ms), tp.status, td.rows.max(tp.rows),
            if identical { "IDENTICAL" } else if kd.is_some() && kp.is_some() { "MISMATCH" } else { "(not comparable)" }
        );
        // regression cases carry the answer they must give (Demo B 27,624 ids, the wide window 189,772): a wrong row count is a correctness failure
        let mut problems = problems;
        if let Some(want) = case["expected_rows"].as_u64() {
            for (name, t) in [("duckdb", &td), ("postgis", &tp)] {
                if t.status == "ok" && t.rows as u64 != want {
                    problems.push(format!("{name} returned {} rows, the regression case expects {want}", t.rows));
                }
            }
        }
        if !problems.is_empty() {
            mismatches.push(format!("{id}: {}", problems.join(" | ").chars().take(600).collect::<String>()));
        }
        let record = serde_json::json!({
            "id": id, "seq": i, "template": case["template"], "region": case["region"], "spec": case, "plan_id": plan.plan_id(), "engines_run": engines,
            "engine_order": order, "planner": planner,
            "duckdb": timing_json(&td), "postgis": timing_json(&tp),
            "comparable": kd.is_some() && kp.is_some(), "identical": identical, "problems": problems, "explain": explain_lines,
        });
        writeln!(out, "{record}").unwrap();
        out.flush().unwrap();
    }
    assert!(mismatches.is_empty(), "cross-engine MISMATCH (stop: correctness comes first):\n{}", mismatches.join("\n"));
}
