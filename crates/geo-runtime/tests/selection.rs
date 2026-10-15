//! Engine selection with the REAL engines (DuckDB server + DuckDB-Wasm descriptor) on real files:
//! synthetic 10 MB / 100 MB / 1 GB and the Overture subset. No mocks: the planner reads the actual
//! Parquet footers and applies the calibrated cost model.
//! Tests skip (with a message) when benchmark data is absent.

use geo_executor::EngineKind;
use geo_ir::*;
use geo_planner::{MemoryClass, Observation, Outcome, PlanInputs, PlannerConfig, ProbeCheck, ProbeReport};
use geo_runtime::Runtime;
use geo_types::*;
use std::path::PathBuf;
use std::sync::OnceLock;

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data")
}

fn rt() -> &'static Runtime {
    static R: OnceLock<Runtime> = OnceLock::new();
    R.get_or_init(Runtime::with_duckdb_and_browser)
}

fn ctx() -> geo_executor::ExecutionContext {
    geo_executor::ExecutionContext::new(vec![data()])
}

fn source(name: &str) -> Option<Node> {
    let p = data().join(format!("{name}.parquet"));
    if !p.exists() {
        eprintln!("SKIP: {} missing", p.display());
        return None;
    }
    Some(rt().open_source(p.to_str().unwrap(), &ctx()).unwrap())
}

/// Window = fraction of the dataset extent area, around a focus point.
fn window(e: [f64; 4], f: f64, focus: (f64, f64)) -> [f64; 4] {
    let (w, h) = ((e[2] - e[0]) * f.sqrt(), (e[3] - e[1]) * f.sqrt());
    let cx = focus.0.clamp(e[0] + w / 2.0, e[2] - w / 2.0);
    let cy = focus.1.clamp(e[1] + h / 2.0, e[3] - h / 2.0);
    [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0]
}

const SYN: [f64; 4] = [100.0, 8.0, 110.0, 24.0];

fn bbox_plan(src: Node, crs: Crs, w: [f64; 4], limit: u64, aggregate: bool) -> Plan {
    let f = Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: BoundingBox::new(w[0], w[1], w[2], w[3], crs).unwrap() };
    let top = if aggregate {
        Node::Aggregate { input: Box::new(f), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }] }
    } else {
        Node::Limit { input: Box::new(f), n: limit }
    };
    Plan::new(top)
}

fn pick(plan: &Plan, cfg: &PlannerConfig) -> geo_planner::PhysicalPlan {
    rt().plan_with(cfg, plan, &ctx()).expect("plan")
}

/// A browser runtime that passed every probe check (what the web client reports on this machine) and a
/// planner that has seen this source before (4 earlier runs, so its estimate is calibrated).
fn world() -> PlanInputs {
    let c = |name: &str, ok: bool| ProbeCheck { name: name.into(), ok, required: !name.starts_with("read_class") && name != "extension_load", detail: String::new(), ms: 1.0 };
    let mut i = PlanInputs::default();
    i.probes.insert(
        "duckdb-wasm".into(),
        ProbeReport {
            engine: "duckdb-wasm".into(),
            session: "t".into(),
            engine_version: "1".into(),
            runtime_version: "1".into(),
            captured_at_ms: i.now_ms,
            duration_ms: 1.0,
            checks: vec![c("init", true), c("worker", true), c("parquet", true), c("arrow", true), c("covering", true), c("read_class:plain_wkb", true), c("read_class:duckdb_typed", false)],
            memory_class: MemoryClass::High,
            device_memory_gb: Some(8.0),
            js_heap_limit_mb: Some(4096),
            supported_ops: ["source", "filter", "bbox_filter", "aggregate", "project", "limit", "sort"].iter().map(|s| s.to_string()).collect(),
            device: Some(geo_planner::DeviceProfile { browser_family: "chrome".into(), browser_major: Some(140), platform: "windows".into(), hardware_concurrency: Some(8), wasm_simd: true, wasm_threads: false }),
        },
    );
    let obs: Vec<Observation> = (0..4)
        .map(|k| Observation {
            ts_ms: 0, source: "clustered_10mb_spatial".into(), kind: "features".into(), engine: "duckdb".into(), raw_selected: 226.0 + k as f64, total_rows: 130_000,
            est_rows: 226, actual_rows: 2, limit: Some(2000), censored: false, group_by: None, est_ms: None, actual_ms: 80.0, network: "LOCAL".into(), fallback: false, layout: "coh=high|skew=peaked|cov=declared".into(),
        })
        .collect();
    i.calibration = geo_planner::Calibration::fit(&obs);
    i
}

fn pick_in(plan: &Plan, cfg: &PlannerConfig, inputs: &PlanInputs) -> geo_planner::PhysicalPlan {
    rt().plan_with_inputs(cfg, plan, &ctx(), inputs).expect("plan")
}

fn reason(p: &geo_planner::PhysicalPlan, engine: &str) -> String {
    p.candidates.iter().find(|c| c.engine.0 == engine).map(|c| c.rejected_because.join(" | ")).unwrap_or_default()
}

#[test]
fn small_file_selective_viewport_runs_in_the_browser() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    // 0.2 % of the extent, around a dense cluster: measured faster in the browser (35-62 ms vs 74-91 ms)
    let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.002, (104.5, 12.5)), 2000, false);
    let p = pick_in(&plan, &PlannerConfig::default(), &world());
    assert_eq!(p.engine.0, "duckdb-wasm", "decisions: {:?}", p.decisions);
    assert_eq!(p.placement, EngineKind::Browser);
    let alt = p.candidates.iter().find(|c| c.engine.0 == "duckdb").unwrap();
    assert!(alt.feasible && alt.score_ms.unwrap() > p.candidates.iter().find(|c| c.engine.0 == "duckdb-wasm").unwrap().score_ms.unwrap());
}

#[test]
fn medium_file_loses_on_score_and_large_file_is_rejected_by_the_memory_guard() {
    // 100 MB file: the tab *could* hold it (covering columns ~44 MiB < 50 MiB guard) but the planner knows
    // DuckDB-Wasm must read every bbox column chunk of the file (measured ~250-330 ms vs ~85 ms on the server)
    if let Some(src) = source("clustered_100mb_spatial") {
        let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.002, (104.5, 12.5)), 2000, false);
        let p = pick(&plan, &PlannerConfig::default());
        assert_eq!(p.engine.0, "duckdb");
        let wasm = p.candidates.iter().find(|c| c.engine.0 == "duckdb-wasm").unwrap();
        assert!(wasm.feasible, "feasible but slower: {}", reason(&p, "duckdb-wasm"));
        let server = p.candidates.iter().find(|c| c.engine.0 == "duckdb").unwrap();
        assert!(wasm.score_ms.unwrap() > 2.0 * server.score_ms.unwrap(), "browser {:?} vs server {:?}", wasm.score_ms, server.score_ms);
    }
    // 1 GB file, small window: since the browser engine reads with range requests it is *feasible* (it fetches only the
    // touched row groups), but ~200 sequential requests make it far slower than the server: it loses on score
    if let Some(src) = source("clustered_1gb_spatial") {
        let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.002, (104.5, 12.5)), 2000, false);
        let p = pick(&plan, &PlannerConfig::default());
        assert_eq!(p.engine.0, "duckdb");
        let wasm = p.candidates.iter().find(|c| c.engine.0 == "duckdb-wasm").unwrap();
        let server = p.candidates.iter().find(|c| c.engine.0 == "duckdb").unwrap();
        assert!(wasm.feasible, "{}", reason(&p, "duckdb-wasm"));
        assert!(wasm.score_ms.unwrap() > 3.0 * server.score_ms.unwrap(), "browser {:?} vs server {:?}", wasm.score_ms, server.score_ms);
        assert_eq!(p.assessment.browser_access[0].strategy, geo_planner::AccessStrategy::Range);
    }
}

#[test]
fn real_overture_data_goes_to_duckdb() {
    let Some(src) = source("overture_buildings_vn_south") else { return };
    let plan = bbox_plan(src, Crs::OgcCrs84, window([105.0, 8.5, 108.0, 12.0], 0.002, (106.7, 10.8)), 2000, false);
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.engine.0, "duckdb");
    let wasm = p.candidates.iter().find(|c| c.engine.0 == "duckdb-wasm").unwrap();
    assert!(!wasm.feasible || wasm.score_ms.unwrap() > p.candidates[0].score_ms.unwrap(), "browser is never the better choice on this file");
    // the inferred covering is reported, not hidden
    assert!(p.decisions.iter().any(|d| d.rule == "covering" && d.outcome == Outcome::Warning), "{:?}", p.decisions);
    assert!(p.scans[0].covering_inferred);
}

#[test]
fn unsupported_operation_falls_back_to_duckdb_with_the_reason() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    let plan = Plan::new(Node::Intersects {
        input: Box::new(src),
        geometry_column: "geometry".into(),
        with: GeometryLiteral { wkt: "POLYGON((104 12, 104.2 12, 104.2 12.2, 104 12.2, 104 12))".into(), crs: Crs::Epsg(4326) },
    });
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.engine.0, "duckdb");
    assert!(reason(&p, "duckdb-wasm").contains("intersects"));
}

#[test]
fn spatial_join_dag_is_not_offered_to_the_browser() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    let b = |w: [f64; 4]| Node::BboxFilter { input: Box::new(src.clone()), geometry_column: "geometry".into(), bbox: BoundingBox::new(w[0], w[1], w[2], w[3], Crs::Epsg(4326)).unwrap() };
    let j = Node::SpatialJoin {
        left: Box::new(b([104.0, 12.0, 104.3, 12.3])),
        right: Box::new(b([104.2, 12.2, 104.5, 12.5])),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "r_".into(),
    };
    let plan = Plan::new(Node::Aggregate { input: Box::new(j), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }] });
    assert_eq!(plan.nodes.values().filter(|n| n.kind() == OpKind::Source).count(), 1, "shared source");
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.engine.0, "duckdb");
    assert!(reason(&p, "duckdb-wasm").contains("spatial_join"));
}

#[test]
fn browser_memory_limit_is_a_hard_constraint_not_a_score() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.002, (104.5, 12.5)), 2000, false);
    let mut cfg = PlannerConfig::default();
    cfg.browser_max_input_bytes = Some(1 << 20); // 1 MB: the tab "cannot hold" even this small plan
    let p = pick(&plan, &cfg);
    assert_eq!(p.engine.0, "duckdb");
    assert!(reason(&p, "duckdb-wasm").contains("memory guard"));
    // result-row guard as well
    let mut cfg = PlannerConfig::default();
    cfg.browser_max_result_rows = 10;
    let p = pick(&plan, &cfg);
    assert_eq!(p.engine.0, "duckdb");
    assert!(reason(&p, "duckdb-wasm").contains("result"), "{}", reason(&p, "duckdb-wasm"));
}

#[test]
fn browser_disabled_goes_to_duckdb() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.002, (104.5, 12.5)), 2000, false);
    let mut cfg = PlannerConfig::default();
    cfg.allow_browser = false;
    let p = pick(&plan, &cfg);
    assert_eq!(p.engine.0, "duckdb");
    assert!(reason(&p, "duckdb-wasm").contains("disabled"));
}

#[test]
fn hints_are_honored_only_when_feasible() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    let mut plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.002, (104.5, 12.5)), 2000, false);
    plan.hints.preferred_engine = Some("duckdb".into());
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.engine.0, "duckdb", "a feasible hint beats a better score");
    assert!(p.decisions.iter().any(|d| d.rule == "hint" && d.outcome == Outcome::Info));

    // an infeasible hint (the browser cannot run Intersects) is ignored
    let hinted = Plan::new(Node::Intersects {
        input: Box::new(source("clustered_10mb_spatial").unwrap()),
        geometry_column: "geometry".into(),
        with: GeometryLiteral { wkt: "POLYGON((104 12, 104.2 12, 104.2 12.2, 104 12.2, 104 12))".into(), crs: Crs::Epsg(4326) },
    });
    let mut plan = hinted;
    plan.hints.preferred_engine = Some("duckdb-wasm".into());
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.engine.0, "duckdb", "an infeasible hint is ignored");
    assert!(p.decisions.iter().any(|d| d.rule == "hint" && d.outcome == Outcome::Warning));
}

#[test]
fn large_result_prefers_the_server_even_on_a_small_file() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    // 25 % of the extent, up to 50 000 features: the browser pays ~96 us/row, the server ~18 us/row
    let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.25, (104.5, 12.5)), 50_000, false);
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.engine.0, "duckdb", "decisions: {:?}", p.decisions);
}

#[test]
fn every_candidate_is_explained() {
    let Some(src) = source("clustered_10mb_spatial") else { return };
    let plan = bbox_plan(src, Crs::Epsg(4326), window(SYN, 0.05, (104.5, 12.5)), 2000, true);
    let p = pick(&plan, &PlannerConfig::default());
    assert_eq!(p.candidates.len(), 2);
    for c in &p.candidates {
        assert!(c.feasible == c.score_ms.is_some());
        assert!(!c.reasons.is_empty() || !c.rejected_because.is_empty());
        if let Some(cost) = &c.cost {
            let sum = cost.fixed_ms + cost.scan_ms + cost.result_ms + cost.transfer_ms;
            assert!((sum - c.score_ms.unwrap()).abs() < 1e-6, "score = sum of its components");
        }
    }
    assert!(p.decisions.iter().any(|d| d.rule == "select_engine" && d.outcome == Outcome::Chosen && d.reason.contains("alternatives")));
    assert_eq!(p.stages.len(), 1, "v1 emits single-engine plans");
}
