//! Planner hardening (ADR-009/010) with the REAL engine descriptors on real footers: runtime probe,
//! health, known incompatibilities, network profiles, browser risk, confidence policy and fallback.
//! Tests skip (with a message) when benchmark data is absent.

use geo_executor::EngineKind;
use geo_ir::*;
use geo_planner::{
    FailureClass, HealthState, KnownIncompat, MemoryClass, NetworkProfile, Observation, PlanInputs, PlannerConfig, ProbeCheck, ProbeReport,
};
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
fn window(e: [f64; 4], f: f64, focus: (f64, f64)) -> [f64; 4] {
    let (w, h) = ((e[2] - e[0]) * f.sqrt(), (e[3] - e[1]) * f.sqrt());
    let cx = focus.0.clamp(e[0] + w / 2.0, e[2] - w / 2.0);
    let cy = focus.1.clamp(e[1] + h / 2.0, e[3] - h / 2.0);
    [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0]
}
const SYN: [f64; 4] = [100.0, 8.0, 110.0, 24.0];

fn plan_for(name: &str, f: f64, limit: u64) -> Option<Plan> {
    let src = source(name)?;
    let w = window(SYN, f, (104.5, 12.5));
    let crs = if name.starts_with("overture") { Crs::OgcCrs84 } else { Crs::Epsg(4326) };
    let w = if name.starts_with("overture") { window([105.0, 8.5, 108.0, 12.0], f, (106.7, 10.8)) } else { w };
    let b = Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: BoundingBox::new(w[0], w[1], w[2], w[3], crs).unwrap() };
    Some(Plan::new(Node::Limit { input: Box::new(b), n: limit }))
}

fn check(name: &str, ok: bool, required: bool) -> ProbeCheck {
    ProbeCheck { name: name.into(), ok, required, detail: if ok { "ok".into() } else { "failed".into() }, ms: 1.0 }
}

fn probe(mem: MemoryClass, gb: Option<f64>, typed_ok: bool) -> ProbeReport {
    ProbeReport {
        engine: "duckdb-wasm".into(),
        session: "t".into(),
        engine_version: "1".into(),
        runtime_version: "1".into(),
        captured_at_ms: geo_planner::now_ms(),
        duration_ms: 1.0,
        checks: vec![
            check("init", true, true),
            check("worker", true, true),
            check("parquet", true, true),
            check("arrow", true, true),
            check("covering", true, true),
            check("read_class:plain_wkb", true, false),
            check("read_class:duckdb_typed", typed_ok, false),
        ],
        memory_class: mem,
        device_memory_gb: gb,
        js_heap_limit_mb: None,
        supported_ops: ["source", "filter", "bbox_filter", "aggregate", "project", "limit", "sort"].iter().map(|s| s.to_string()).collect(),
    }
}

/// Probed browser + a calibrated source: the setting in which the browser is allowed to win on small files.
fn world(source_name: &str) -> PlanInputs {
    let mut i = PlanInputs::default();
    i.probes.insert("duckdb-wasm".into(), probe(MemoryClass::High, Some(8.0), false));
    let obs: Vec<Observation> = (0..4)
        .map(|k| Observation {
            ts_ms: 0, source: source_name.into(), kind: "features".into(), engine: "duckdb".into(), raw_selected: 226.0 + k as f64, total_rows: 130_000,
            est_rows: 226, actual_rows: 2, limit: Some(2000), censored: false, group_by: None, est_ms: None, actual_ms: 80.0, network: "LOCAL".into(), fallback: false,
        })
        .collect();
    i.calibration = geo_planner::Calibration::fit(&obs);
    i
}

fn pick(plan: &Plan, i: &PlanInputs) -> geo_planner::PhysicalPlan {
    rt().plan_with_inputs(&PlannerConfig::default(), plan, &ctx(), i).expect("plan")
}
fn cand<'a>(p: &'a geo_planner::PhysicalPlan, e: &str) -> &'a geo_planner::Candidate {
    p.candidates.iter().find(|c| c.engine.0 == e).unwrap()
}

#[test]
fn unprobed_browser_is_not_trusted_on_a_close_call() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let mut i = world("clustered_10mb_spatial");
    i.probes.clear();
    let p = pick(&plan, &i);
    assert_eq!(p.engine.0, "duckdb", "{:?}", p.assessment.policy);
    let b = cand(&p, "duckdb-wasm");
    assert!(b.feasible, "static capabilities say yes");
    assert_eq!(b.probe_status, "missing");
    assert!(b.confidence.as_ref().unwrap().score < cand(&p, "duckdb").confidence.as_ref().unwrap().score);
    // with a fresh probe the same plan goes to the browser
    let p = pick(&plan, &world("clustered_10mb_spatial"));
    assert_eq!(p.engine.0, "duckdb-wasm", "{:?}", p.decisions);
    assert_eq!(p.assessment.fallback.as_ref().unwrap().engine.0, "duckdb", "a browser plan always has a server fallback");
    assert_eq!(p.assessment.fallback.as_ref().unwrap().max_fallbacks, 1);
}

#[test]
fn confidence_is_explained_by_seven_factors_and_drops_with_poor_inputs() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let good = pick(&plan, &world("clustered_10mb_spatial"));
    let names: Vec<&str> = good.candidates[1].confidence.as_ref().unwrap().factors.iter().map(|f| f.name).collect();
    assert_eq!(
        names,
        ["estimate_quality", "covering_quality", "probe_freshness", "engine_health", "network_uncertainty", "dataset_familiarity", "operation_support"]
    );
    // same plan, unknown source (no calibration), stale probe, degraded browser
    let mut worse = world("other");
    worse.now_ms += 3_600_000; // the probe is now older than its TTL
    let w = pick(&plan, &worse);
    assert!(
        cand(&w, "duckdb-wasm").confidence.as_ref().unwrap().score < cand(&good, "duckdb-wasm").confidence.as_ref().unwrap().score,
        "calibration + fresh probe must raise confidence"
    );
    assert_eq!(cand(&w, "duckdb-wasm").probe_status, "stale");
}

#[test]
fn probe_failure_pre_rejects_the_browser() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let mut i = world("clustered_10mb_spatial");
    i.probes.get_mut("duckdb-wasm").unwrap().checks[2].ok = false; // parquet read failed in this runtime
    let p = pick(&plan, &i);
    assert_eq!(p.engine.0, "duckdb");
    assert!(cand(&p, "duckdb-wasm").rejected_because.join(" ").contains("runtime probe failed"), "{:?}", cand(&p, "duckdb-wasm").rejected_because);
}

#[test]
fn file_class_the_runtime_cannot_read_is_pre_rejected_not_discovered_by_crashing() {
    let Some(plan) = plan_for("overture_buildings_vn_south", 0.002, 2000) else { return };
    let mut cfg = PlannerConfig::default();
    cfg.browser_max_input_bytes = Some(2 << 30); // operator override: take the memory model out of the picture
    let p = rt().plan_with_inputs(&cfg, &plan, &ctx(), &world("overture_buildings_vn_south")).unwrap();
    assert_eq!(p.engine.0, "duckdb");
    assert!(cand(&p, "duckdb-wasm").rejected_because.join(" ").contains("duckdb_typed"), "{:?}", cand(&p, "duckdb-wasm").rejected_because);
    // a runtime that DOES read the class is allowed to try
    let mut i = world("overture_buildings_vn_south");
    i.probes.insert("duckdb-wasm".into(), probe(MemoryClass::High, Some(8.0), true));
    let p = rt().plan_with_inputs(&cfg, &plan, &ctx(), &i).unwrap();
    assert!(cand(&p, "duckdb-wasm").feasible, "{:?}", cand(&p, "duckdb-wasm").rejected_because);
}

#[test]
fn unhealthy_browser_is_rejected_degraded_is_penalised_and_all_unhealthy_still_runs() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let mut i = world("clustered_10mb_spatial");
    i.health.insert(
        "duckdb-wasm".into(),
        geo_planner::HealthView { state: HealthState::Unhealthy, reason: "crashed 2 s ago".into(), events: 3, failures: 1, last_failure: Some(FailureClass::EngineCrash), engine_version: None },
    );
    let p = pick(&plan, &i);
    assert_eq!(p.engine.0, "duckdb");
    assert!(cand(&p, "duckdb-wasm").rejected_because.join(" ").contains("unhealthy"));
    // degraded: still feasible, lower confidence, higher risk
    let mut d = world("clustered_10mb_spatial");
    d.health.insert("duckdb-wasm".into(), geo_planner::HealthView { state: HealthState::Degraded, reason: "1 of 3 failed".into(), events: 3, failures: 1, last_failure: Some(FailureClass::Transient), engine_version: None });
    let pd = pick(&plan, &d);
    let b = cand(&pd, "duckdb-wasm");
    assert!(b.feasible);
    assert!(b.risk.as_ref().unwrap().health > 0.3);
    // every engine unhealthy: health is ignored (with a warning) so that the query can still run
    let mut all = world("clustered_10mb_spatial");
    for e in ["duckdb", "duckdb-wasm"] {
        all.health.insert(e.into(), geo_planner::HealthView { state: HealthState::Unhealthy, reason: "x".into(), events: 3, failures: 3, last_failure: Some(FailureClass::EngineCrash), engine_version: None });
    }
    let pa = pick(&plan, &all);
    assert!(pa.decisions.iter().any(|d| d.rule == "health"), "{:?}", pa.decisions);
}

#[test]
fn deterministic_failure_on_a_source_is_remembered_and_not_repeated() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let mut i = world("clustered_10mb_spatial");
    i.incompat.push(KnownIncompat { engine: "duckdb-wasm".into(), source: "clustered_10mb_spatial".into(), class: FailureClass::UnsupportedFormat, message: "ActiveTransaction".into(), ts_ms: 0 });
    let p = pick(&plan, &i);
    assert_eq!(p.engine.0, "duckdb");
    assert!(cand(&p, "duckdb-wasm").rejected_because.join(" ").contains("failed before"), "{:?}", cand(&p, "duckdb-wasm").rejected_because);
}

#[test]
fn network_profile_changes_the_decision_and_the_cost_split() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let mut scores = Vec::new();
    for name in geo_planner::PROFILE_NAMES {
        let mut i = world("clustered_10mb_spatial");
        i.network = NetworkProfile::by_name(name).unwrap();
        let p = pick(&plan, &i);
        let (s, b) = (cand(&p, "duckdb"), cand(&p, "duckdb-wasm"));
        scores.push((name, p.engine.0.clone(), s.score_ms.unwrap(), b.score_ms.unwrap(), b.cost.as_ref().unwrap().latency_ms));
    }
    // the browser pays for its many range requests: every step to a slower network widens the gap
    let gap = |k: usize| scores[k].3 - scores[k].2;
    assert!(gap(1) > gap(0) && gap(3) > gap(2) && gap(4) > gap(3), "{scores:?}");
    assert_eq!(scores[0].1, "duckdb-wasm", "LOCAL (the calibration host): the browser wins this case");
    for k in [3, 4] {
        assert_eq!(scores[k].1, "duckdb", "{}: the server wins when bytes and round trips are expensive: {scores:?}", scores[k].0);
    }
    assert!(scores[4].4 > 2.0 * 155.0 - 1.0, "HIGH_LATENCY: plan request + whole-file download: {scores:?}");
    // a file above the whole-file limit is read with many sequential range requests: latency explodes on a slow link
    if let Some(big) = plan_for("clustered_100mb_spatial", 0.01, 2000) {
        let mut i = world("clustered_100mb_spatial");
        i.network = NetworkProfile::by_name("HIGH_LATENCY").unwrap();
        let p = pick(&big, &i);
        let b = cand(&p, "duckdb-wasm");
        assert!(b.cost.as_ref().unwrap().latency_ms > 1000.0, "range mode on HIGH_LATENCY: {:?}", b.cost);
        assert_eq!(p.assessment.browser_access[0].strategy, geo_planner::AccessStrategy::Range);
    }
}

#[test]
fn cold_browser_instance_is_priced_in() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let warm = pick(&plan, &world("clustered_10mb_spatial"));
    let mut cold_in = world("clustered_10mb_spatial");
    cold_in.browser_warm = Some(false);
    let cold = pick(&plan, &cold_in);
    let (w, c) = (cand(&warm, "duckdb-wasm"), cand(&cold, "duckdb-wasm"));
    assert!(c.score_ms.unwrap() - w.score_ms.unwrap() > 1000.0);
    assert_eq!(c.cost.as_ref().unwrap().cold_start_ms, 1400.0);
    assert_eq!(cold.engine.0, "duckdb", "1.4 s of instantiation outweighs a 20 ms advantage");
}

#[test]
fn browser_risk_scales_with_memory_class_and_replaces_the_fixed_limit() {
    let Some(plan) = plan_for("clustered_100mb_spatial", 0.002, 2000) else { return };
    let mut hi = world("clustered_100mb_spatial");
    hi.probes.insert("duckdb-wasm".into(), probe(MemoryClass::High, Some(8.0), false));
    let mut lo = world("clustered_100mb_spatial");
    lo.probes.insert("duckdb-wasm".into(), probe(MemoryClass::Low, Some(1.0), false));
    let (ph, pl) = (pick(&plan, &hi), pick(&plan, &lo));
    let (rh, rl) = (cand(&ph, "duckdb-wasm").risk.as_ref().unwrap(), cand(&pl, "duckdb-wasm").risk.as_ref().unwrap());
    assert!(rl.memory > rh.memory * 4.0, "a 1 GB device has a much smaller crash threshold: {} vs {}", rl.memory, rh.memory);
    assert_eq!(rh.crash_threshold_bytes, 245 * 1024 * 1024);
    assert!(rh.score >= rh.memory.min(1.0) - 1e-9 && rh.score <= 1.0);
    // 1 GB file, half of the extent, 100k rows: the browser would have to pull far more than a tab holds ->
    // infeasible through the risk model, with the numbers in the message (a small window on the same file is fine now:
    // range requests fetch only what the query touches)
    if let Some(src) = source("clustered_1gb_spatial") {
        let w = window(SYN, 0.5, (104.5, 12.5));
        let b = Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: BoundingBox::new(w[0], w[1], w[2], w[3], Crs::Epsg(4326)).unwrap() };
        let big = Plan::new(Node::Aggregate { input: Box::new(b), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }] });
        let p = pick(&big, &hi);
        let why = cand(&p, "duckdb-wasm").rejected_because.join(" ");
        assert!(why.contains("memory risk") && why.contains("crash threshold"), "{why}");
        assert_eq!(p.engine.0, "duckdb");
    }
}

#[test]
fn result_rows_unsafe_for_the_browser_are_pre_rejected() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.25, 100_000) else { return };
    let mut cfg = PlannerConfig::default();
    cfg.browser_max_result_rows = 5_000;
    let p = rt().plan_with_inputs(&cfg, &plan, &ctx(), &world("clustered_10mb_spatial")).unwrap();
    assert_eq!(p.engine.0, "duckdb");
    assert!(cand(&p, "duckdb-wasm").rejected_because.join(" ").contains("result"), "{:?}", cand(&p, "duckdb-wasm").rejected_because);
}

#[test]
fn estimator_v2_exposes_quality_interval_and_calibration() {
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let raw = pick(&plan, &PlanInputs::default());
    let cal = pick(&plan, &world("clustered_10mb_spatial"));
    let (er, ec) = (&raw.assessment.estimate, &cal.assessment.estimate);
    assert_eq!(er.correction.basis, "none");
    assert_eq!(ec.correction.basis, "source");
    assert!(ec.rows < er.rows / 5, "the learned correction removes most of the over-estimate: {} vs {}", ec.rows, er.rows);
    assert!(ec.low <= ec.rows && ec.rows <= ec.high);
    assert_eq!(er.covering_quality, geo_planner::CoveringQuality::Declared);
    assert!(er.spatial_coherence.unwrap() > 0.9, "spatially sorted file");
    assert_eq!(er.row_groups_total, 26);
    // the shuffled twin is recognised as incoherent
    if let Some(sh) = plan_for("clustered_10mb_shuffled", 0.002, 2000) {
        let p = pick(&sh, &PlanInputs::default());
        assert!(p.assessment.estimate.spatial_coherence.unwrap() < 0.2);
        assert!(p.assessment.estimate.quality <= geo_planner::EstimateQuality::Fair);
    }
}

#[test]
fn browser_selection_never_exceeds_one_fallback_and_never_retries() {
    for c in FailureClass::ALL {
        assert!(!c.retry_allowed());
    }
    let Some(plan) = plan_for("clustered_10mb_spatial", 0.002, 2000) else { return };
    let p = pick(&plan, &world("clustered_10mb_spatial"));
    assert_eq!(p.placement, EngineKind::Browser);
    assert_eq!(p.assessment.fallback.as_ref().map(|f| f.max_fallbacks), Some(1));
    let server_plan = pick(&plan, &PlanInputs { probes: Default::default(), ..world("x") });
    assert!(server_plan.assessment.fallback.is_none(), "a server plan has no fallback");
}
