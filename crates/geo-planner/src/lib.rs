//! Planner v1 (RFC-003): chooses *where* a logical plan runs.
//!
//! Input: PlanGraph + source metadata (footer statistics, via the access guard)
//! + engine descriptors (capabilities, limits). Output: a [`PhysicalPlan`] with
//! the selected engine, **every candidate with its score and reasons**, and a
//! decision log. The planner never executes anything and never names an engine
//! concretely: engines arrive only through [`EngineDescriptor`].
//!
//! Cost model (deliberately simple, all numbers configurable and **fitted on measurements**, see
//! [`CostModel`]; network terms come from a [`NetworkProfile`], see `network.rs`):
//!
//! ```text
//! score_ms(server)  = fixed + a_s*input_MB + r_s*result_rows + RTT + upload + result_bytes/down
//! score_ms(browser) = fixed + a_b*covering_MB + b_b*input_MB + r_b*result_rows
//!                     + (1 + ceil(range_requests/parallelism))*RTT + (covering+input)_bytes/down
//! ```
//!
//! i.e. a server engine scans next to the data and ships the *result*; a
//! browser engine must first pull the data it needs across the network.
//! Hard limits (browser memory risk, result rows, operations, covering column, runtime probe, health,
//! known incompatibilities) make a candidate infeasible — they are never traded against score.
//! Among the feasible candidates the lowest score wins, **unless** the explicit confidence policy
//! (R5b) prefers the server: see [`PlannerConfig::low_confidence_floor`] and `close_call_margin`.
//!
//! Hardening (ADR-009/010): the planner takes a [`PlanInputs`] snapshot — runtime probes, observed
//! health, known incompatibilities, calibration, network — and returns an [`Assessment`] with a
//! confidence score and reasons, the browser risk score and the planned fallback.

mod confidence;
mod estimator;
mod failure;
mod network;
mod probe;
mod state;

pub use confidence::{Confidence, ConfidenceFactor};
pub use estimator::{
    bucket_of, error_factor, estimate as estimate_rows, plan_shape, summarize_errors, PlanShape, Calibration, CoveringQuality, Correction, ErrorSummary,
    EstimateQuality, Observation, RowEstimate,
};
pub use failure::{FailureClass, HealthConfig, HealthState, HealthTracker, HealthView, KnownIncompat};
pub use network::{NetworkOrigin, NetworkProfile, PROFILE_NAMES};
pub use probe::{checks as probe_checks, crash_threshold_bytes, memory_class_for, Freshness, MemoryClass, ProbeCheck, ProbeReport};
pub use state::{now_ms, ExecutionReport, PlanInputs, PlannerState, ProbeVerdict, StateSummary};

use geo_executor::{
    Capabilities, EngineDescriptor, EngineId, EngineKind, Estimate, ExecutionContext, ExplainStep,
};
use geo_ir::{validate, OpKind, Plan, PlanNode, ValidationReport};
use geo_source_geoparquet::{analyze_plan, ReadClass, SourceScan};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Info,
    Chosen,
    Rejected,
    Warning,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub rule: String,
    pub subject: String,
    pub outcome: Outcome,
    pub reason: String,
}

/// Linear cost model (ms, MB, µs). Defaults were **fitted on measurements** (crossover benchmark,
/// 65 samples over synthetic 10 MB / 100 MB / 1 GB files and a real 1.45 GB Overture subset, one
/// machine, browser and gateway on the same host, so local transfer is ~free). The network is
/// therefore modelled separately (`NetworkProfile`), never part of the fit.
///
/// Fitted (non-negative least squares on *relative* error, so small queries are not drowned out by
/// the large ones; the first fit minimised absolute error and mis-ranked every small case):
///   server  = 83 ms + 0.199 ms/MB(kept input) + 17.8 µs/result row                    (median relative error 14 %)
///   browser = 19 ms + 5.38 ms/MB(covering columns of the whole file) + 0.58 ms/MB(kept) + 96 µs/result row
///                                                                                      (median relative error 17.5 %)
/// The browser term on the *whole file's* covering columns exists because DuckDB-Wasm (this build) does
/// not prune row groups through struct-field statistics: it reads every bbox column chunk.
#[derive(Debug, Clone, Serialize)]
pub struct CostModel {
    pub server_fixed_ms: f64,
    pub server_ms_per_input_mb: f64,
    pub server_us_per_result_row: f64,
    pub browser_fixed_ms: f64,
    pub browser_ms_per_covering_mb: f64,
    pub browser_ms_per_input_mb: f64,
    pub browser_us_per_result_row: f64,
    /// Bytes of the request a client uploads (plan JSON / template).
    pub request_bytes: u64,
    /// Range requests a browser engine can have in flight at once (assumption; sequential turns = requests / this).
    pub range_request_parallelism: f64,
    /// One-off cost when the browser engine instance is not instantiated yet (measured median 1.3-1.5 s).
    pub browser_cold_start_ms: f64,
    pub avg_result_row_bytes: u64,
    /// Result rows assumed for a grouped aggregate (ungrouped = 1).
    pub aggregate_result_rows: u64,
}

impl Default for CostModel {
    fn default() -> Self {
        CostModel {
            server_fixed_ms: 83.0,
            server_ms_per_input_mb: 0.199,
            server_us_per_result_row: 17.8,
            browser_fixed_ms: 19.0,
            browser_ms_per_covering_mb: 5.38,
            browser_ms_per_input_mb: 0.58,
            browser_us_per_result_row: 96.0,
            // The fitted terms were measured on loopback and contain no network: the network is added
            // from the NetworkProfile (LOCAL = the measured calibration host).
            request_bytes: 2048,
            range_request_parallelism: 4.0,
            browser_cold_start_ms: 1400.0,
            avg_result_row_bytes: 300,
            aggregate_result_rows: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannerConfig {
    /// Warn when the metadata-based estimate exceeds this many bytes.
    pub warn_scan_bytes: u64,
    /// Refuse plans whose metadata-based estimate exceeds this many bytes.
    pub max_scan_bytes: u64,
    /// Browser execution can be switched off entirely.
    pub allow_browser: bool,
    /// Operator override of the browser memory guard. `None` (default) = the risk model decides
    /// (`browser_reject_risk` x the probed crash threshold); `Some(n)` = reject above n bytes, and the
    /// risk model's memory rejection is off (used by measurement runs that must go past the guard).
    pub browser_max_input_bytes: Option<u64>,
    /// Browser memory guard: estimated result rows above this make the browser infeasible.
    pub browser_max_result_rows: u64,
    /// Reject the browser when estimated input / probed crash threshold exceeds this (safety margin).
    pub browser_reject_risk: f64,
    /// R5b: the browser is only chosen on a cost advantage when its confidence reaches this floor.
    pub low_confidence_floor: f64,
    /// R5b: a cost advantage smaller than this fraction of the server cost is a "close call".
    pub close_call_margin: f64,
    pub cost: CostModel,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        PlannerConfig {
            warn_scan_bytes: 512 << 20,
            max_scan_bytes: 8 << 30,
            allow_browser: true,
            browser_max_input_bytes: None,
            browser_max_result_rows: 100_000,
            browser_reject_risk: 0.6,
            low_confidence_floor: 0.45,
            close_call_margin: 0.2,
            cost: CostModel::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CostBreakdown {
    pub fixed_ms: f64,
    /// Reading/scanning (server: kept input; browser: covering columns + kept input).
    pub scan_ms: f64,
    /// Producing/converting the result rows.
    pub result_ms: f64,
    /// Network time: latency + bytes (server: request turn + result; browser: plan turn + range requests + bytes pulled into the tab).
    pub transfer_ms: f64,
    /// The latency part of `transfer_ms` (turns x (RTT + overhead)); informational, already included.
    pub latency_ms: f64,
    /// One-off browser instance start (already included in `fixed_ms`); 0 when the instance is warm.
    pub cold_start_ms: f64,
    pub scan_bytes: u64,
    pub transfer_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub engine: EngineId,
    pub kind: EngineKind,
    pub feasible: bool,
    /// Estimated milliseconds (lower is better); `None` when infeasible.
    pub score_ms: Option<f64>,
    pub cost: Option<CostBreakdown>,
    pub reasons: Vec<String>,
    pub rejected_because: Vec<String>,
    /// Trust in this candidate's score (see [`Confidence`]); `None` when infeasible.
    pub confidence: Option<Confidence>,
    /// Browser candidates only: risk of the tab running out of memory / failing (0 safe .. 1 certain).
    pub risk: Option<BrowserRisk>,
    pub health: HealthView,
    /// `fresh | stale | missing | not_required`
    pub probe_status: String,
}

/// Browser risk model: replaces the fixed 245 MB constant. `score` = 1 - prod(1 - component).
#[derive(Debug, Clone, Serialize)]
pub struct BrowserRisk {
    pub score: f64,
    /// estimated input / crash threshold.
    pub memory: f64,
    pub result_rows: f64,
    pub probe: f64,
    pub health: f64,
    pub format: f64,
    pub memory_input_bytes: u64,
    pub crash_threshold_bytes: u64,
    pub threshold_basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FallbackPlan {
    pub engine: EngineId,
    pub reason: String,
    /// Bounded recovery: at most one fallback, never a retry of the same engine.
    pub max_fallbacks: u32,
}

/// What the planner thinks about its own decision (ADR-010).
#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    /// Confidence in the selected engine's choice (engine confidence x decision margin).
    pub confidence: Confidence,
    pub engine_confidence: f64,
    /// 1 = a clear winner (or the only feasible engine), 0.6 = a coin flip.
    pub margin_factor: f64,
    pub estimate: RowEstimate,
    pub network: NetworkProfile,
    pub health: BTreeMap<String, HealthView>,
    pub probes: BTreeMap<String, ProbeSummary>,
    pub fallback: Option<FallbackPlan>,
    /// Policy decisions that overrode the lowest score, in words.
    pub policy: Vec<String>,
    pub browser_warm: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeSummary {
    pub status: String,
    pub usable: bool,
    pub age_ms: Option<u64>,
    pub failed_checks: Vec<String>,
    pub memory_class: Option<MemoryClass>,
    pub engine_version: Option<String>,
    pub runtime_version: Option<String>,
}

/// A group of plan nodes executed by one engine. v1 always produces a single stage
/// (single-engine plan); the shape allows multi-engine plans later.
#[derive(Debug, Clone, Serialize)]
pub struct Stage {
    pub engine: EngineId,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PhysicalPlan {
    pub plan_id: String,
    pub engine: EngineId,
    pub placement: EngineKind,
    pub stages: Vec<Stage>,
    pub candidates: Vec<Candidate>,
    pub scans: Vec<SourceScan>,
    pub estimate: Estimate,
    pub input_bytes_upper_bound: u64,
    pub result_rows_estimate: u64,
    pub assessment: Assessment,
    pub decisions: Vec<Decision>,
    /// Engine-provided execution steps (what will actually run).
    pub steps: Vec<ExplainStep>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("plan failed validation")]
    Invalid(ValidationReport),
    #[error("no engine can execute this plan: {0}")]
    NoEngine(String),
    #[error("plan too expensive: {0}")]
    TooExpensive(String),
    /// Opaque on purpose (no path, no reason).
    #[error("source not permitted")]
    AccessDenied,
    #[error("planning failed: {0}")]
    Other(String),
}

pub struct Planner {
    pub config: PlannerConfig,
}

impl Default for Planner {
    fn default() -> Self {
        Planner { config: PlannerConfig::default() }
    }
}

fn dec(rule: &str, subject: impl Into<String>, outcome: Outcome, reason: impl Into<String>) -> Decision {
    Decision { rule: rule.into(), subject: subject.into(), outcome, reason: reason.into() }
}

fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

/// Stable short name of a source (file stem): the key used by telemetry and known incompatibilities.
pub fn source_key(uri: &str) -> String {
    let name = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    name.strip_suffix(".parquet").unwrap_or(name).to_string()
}

/// Everything `plan_with_inputs` knows about one engine before scoring it.
struct EngineView<'a> {
    id: EngineId,
    caps: Capabilities,
    health: HealthView,
    probe: Option<&'a ProbeReport>,
    probe_status: Freshness,
}

/// Per-plan facts shared by every candidate.
struct PlanFacts<'a> {
    plan: &'a Plan,
    scans: &'a [SourceScan],
    est: &'a RowEstimate,
    input_bytes: u64,
    covering_total: u64,
    result_bytes: u64,
    has_bbox_filter: bool,
    source_keys: &'a [String],
    read_classes: &'a [ReadClass],
    familiarity: usize,
    inputs: &'a PlanInputs,
}

impl Planner {
    /// Plan with no knowledge of the world (no probes, no health, no calibration, LOCAL network).
    pub fn plan(&self, plan: &Plan, engines: &[&dyn EngineDescriptor], ctx: &ExecutionContext) -> Result<PhysicalPlan, PlanError> {
        self.plan_with_inputs(plan, engines, ctx, &PlanInputs::default())
    }

    /// Plan against `engines` (registration order = tie-break order) given what is known about the world.
    pub fn plan_with_inputs(
        &self,
        plan: &Plan,
        engines: &[&dyn EngineDescriptor],
        ctx: &ExecutionContext,
        inputs: &PlanInputs,
    ) -> Result<PhysicalPlan, PlanError> {
        let cfg = &self.config;
        let net = &inputs.network;
        let mut log = Vec::new();

        // R1 validation
        let report = validate(plan);
        if !report.is_ok() {
            return Err(PlanError::Invalid(report));
        }
        log.push(dec(
            "validate",
            "plan",
            Outcome::Info,
            format!("IR valid ({} nodes, {} warning(s))", plan.nodes.len(), report.warnings().count()),
        ));
        for w in report.warnings() {
            log.push(dec("validate", w.node.clone(), Outcome::Warning, format!("[{}] {}", w.code, w.message)));
        }

        // R2 scan analysis (source metadata only; sources authorized first)
        let scans = analyze_plan(plan, ctx.guard.as_ref()).map_err(|e| match e {
            geo_source_geoparquet::SourceError::AccessDenied => PlanError::AccessDenied,
            other => PlanError::Other(other.to_string()),
        })?;
        let input_bytes: u64 = scans.iter().map(|s| s.prune.kept_bytes).sum();
        for s in &scans {
            log.push(dec(
                "spatial_prune",
                s.uri.clone(),
                if s.prune.skipped > 0 { Outcome::Info } else { Outcome::Warning },
                format!(
                    "{} constraint(s) pushed to source over {} access path(s); estimated skip {}/{} row groups ({:?}); {} of {} bytes remain{}",
                    s.constraints_applied,
                    s.access_paths,
                    s.prune.skipped,
                    s.prune.total_row_groups,
                    s.prune.method,
                    s.prune.kept_bytes,
                    s.prune.total_bytes,
                    if s.constraints_skipped.is_empty() {
                        String::new()
                    } else {
                        format!("; not pushed: {}", s.constraints_skipped.join(" | "))
                    }
                ),
            ));
        }
        for s in scans.iter().filter(|s| s.covering_inferred) {
            log.push(dec(
                "covering",
                s.uri.clone(),
                Outcome::Warning,
                "bbox covering column inferred by naming convention (struct `bbox`), not declared in the GeoParquet metadata; pruning relies on that column really covering the geometry",
            ));
        }
        let covering_total: u64 = scans.iter().map(|s| s.covering_bytes_total).sum();

        // R2b estimator v2: result rows with calibration, interval and quality
        let est = estimate_rows(plan, &scans, &cfg.cost, &inputs.calibration);
        let result_rows = est.rows;
        let result_bytes = result_rows.saturating_mul(cfg.cost.avg_result_row_bytes);
        log.push(dec(
            "estimate",
            "result rows",
            if est.quality == EstimateQuality::Poor { Outcome::Warning } else { Outcome::Info },
            format!(
                "~{} rows (interval {}..{}), quality {:?}; raw selectivity {:.0} rows x correction {:.2} ({} history, {} sample(s)); covering {:?}{}",
                est.rows,
                est.low,
                est.high,
                est.quality,
                est.raw_selected,
                est.correction.factor,
                est.correction.basis,
                est.correction.samples,
                est.covering_quality,
                if est.notes.is_empty() { String::new() } else { format!("; {}", est.notes.join("; ")) }
            ),
        ));
        let source_keys: Vec<String> = scans.iter().map(|s| source_key(&s.uri)).collect();
        let read_classes: Vec<ReadClass> = scans.iter().map(|s| s.read_class).collect();
        let facts = PlanFacts {
            plan,
            scans: &scans,
            est: &est,
            input_bytes,
            covering_total,
            result_bytes,
            has_bbox_filter: plan.required_ops().contains(&OpKind::BboxFilter),
            source_keys: &source_keys,
            read_classes: &read_classes,
            familiarity: source_keys.iter().map(|k| inputs.calibration.familiarity(k)).min().unwrap_or(0),
            inputs,
        };

        // R3 candidates: feasibility (hard limits) then score. Observed health is a hard filter unless it
        // would leave no engine at all (then it is ignored with a warning: a degraded answer beats none).
        let mut apply_health = true;
        let candidates = loop {
            let mut candidates: Vec<Candidate> = Vec::new();
            for e in engines {
                let id = e.id().to_string();
                let probe = inputs.probes.get(&id);
                let view = EngineView {
                    id: e.id(),
                    caps: e.capabilities(),
                    health: inputs.health.get(&id).cloned().unwrap_or_else(HealthView::unknown),
                    probe,
                    probe_status: match probe {
                        Some(p) => p.freshness(inputs.now_ms, inputs.probe_ttl_ms),
                        None => Freshness::Missing,
                    },
                };
                candidates.push(self.candidate(&view, &facts, net, apply_health));
            }
            if candidates.iter().any(|c| c.feasible) || !apply_health {
                break candidates;
            }
            apply_health = false;
            log.push(dec("health", "engines", Outcome::Warning, "every engine is unhealthy or infeasible: health is ignored for this plan so that something can run"));
        };
        for c in &candidates {
            log.push(dec(
                "candidate",
                c.engine.to_string(),
                if c.feasible { Outcome::Info } else { Outcome::Rejected },
                if c.feasible { c.reasons.join("; ") } else { c.rejected_because.join("; ") },
            ));
        }
        if !candidates.iter().any(|c| c.feasible) {
            return Err(PlanError::NoEngine(
                candidates.iter().map(|c| format!("{}: {}", c.engine, c.rejected_because.join(", "))).collect::<Vec<_>>().join("; "),
            ));
        }

        // R4 hint, then R5 lowest score (ties: first registered), then R5b confidence policy
        let mut policy: Vec<String> = Vec::new();
        let mut chosen: Option<usize> = None;
        let mut hinted = false;
        if let Some(pref) = &plan.hints.preferred_engine {
            match candidates.iter().position(|c| c.engine.0 == *pref && c.feasible) {
                Some(i) => {
                    chosen = Some(i);
                    hinted = true;
                    log.push(dec("hint", pref.clone(), Outcome::Info, "preferred_engine honored (feasible)"));
                }
                None => log.push(dec("hint", pref.clone(), Outcome::Warning, "preferred_engine ignored: not registered or infeasible for this plan")),
            }
        }
        let cheapest = |keep: &dyn Fn(&Candidate) -> bool| {
            candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| c.feasible && keep(c))
                .min_by(|(_, a), (_, b)| a.score_ms.partial_cmp(&b.score_ms).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
        };
        let mut chosen = chosen.unwrap_or_else(|| cheapest(&|_| true).expect("a feasible candidate exists"));
        if !hinted && candidates[chosen].kind == EngineKind::Browser {
            if let Some(si) = cheapest(&|c| c.kind == EngineKind::Server) {
                let (b, s) = (&candidates[chosen], &candidates[si]);
                let (bs, ss) = (b.score_ms.unwrap_or(0.0), s.score_ms.unwrap_or(0.0));
                let (bc, sc) = (b.confidence.as_ref().map_or(0.0, |c| c.score), s.confidence.as_ref().map_or(0.0, |c| c.score));
                let gap = if ss > 0.0 { (ss - bs) / ss } else { 1.0 };
                if b.probe_status == "missing" {
                    policy.push(format!(
                        "R5b unprobed runtime: {} is {:.0}% cheaper on paper, but its runtime has not been probed (static description only); {} chosen. Probe the runtime or pass an explicit engine hint to use it",
                        b.engine,
                        gap * 100.0,
                        s.engine
                    ));
                    chosen = si;
                } else if bc < cfg.low_confidence_floor {
                    policy.push(format!(
                        "R5b low-confidence guard: {} is {:.0} ms cheaper ({:.0}%) but its confidence {:.2} is below the floor {:.2}; {} chosen",
                        b.engine,
                        ss - bs,
                        gap * 100.0,
                        bc,
                        cfg.low_confidence_floor,
                        s.engine
                    ));
                    chosen = si;
                } else if gap < cfg.close_call_margin && bc < sc {
                    policy.push(format!(
                        "R5b close call: {} is only {:.0}% cheaper (< {:.0}%) and less trusted (confidence {:.2} vs {:.2}); {} chosen",
                        b.engine,
                        gap * 100.0,
                        cfg.close_call_margin * 100.0,
                        bc,
                        sc,
                        s.engine
                    ));
                    chosen = si;
                }
            }
        }
        for p in &policy {
            log.push(dec("policy", candidates[chosen].engine.to_string(), Outcome::Info, p.clone()));
        }
        let sel = &candidates[chosen];
        let engine = engines[chosen];
        let tied = candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != chosen && c.feasible && c.score_ms == sel.score_ms)
            .count();
        if tied > 0 && plan.hints.preferred_engine.is_none() {
            log.push(dec("tie_break", "engines", Outcome::Info, "several candidates have the same score; the first registered wins"));
        }

        // R6 cost guard from metadata
        if input_bytes > cfg.max_scan_bytes {
            return Err(PlanError::TooExpensive(format!(
                "estimated {input_bytes} bytes to read exceeds max_scan_bytes={}",
                cfg.max_scan_bytes
            )));
        }
        if input_bytes > cfg.warn_scan_bytes {
            log.push(dec("cost_guard", "scan", Outcome::Warning, format!("estimated {input_bytes} bytes to read")));
        }

        let estimate = engine.estimate(plan, ctx).map_err(|e| PlanError::Other(e.to_string()))?;
        let steps = engine.explain(plan, ctx).map_err(|e| PlanError::Other(e.to_string()))?;
        let others: Vec<String> = candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != chosen)
            .map(|(_, c)| match c.score_ms {
                Some(s) => format!("{} (score {s:.1} ms)", c.engine),
                None => format!("{} (rejected: {})", c.engine, c.rejected_because.join("; ")),
            })
            .collect();
        log.push(dec(
            "select_engine",
            sel.engine.to_string(),
            Outcome::Chosen,
            format!(
                "score {} ms; alternatives: {}",
                sel.score_ms.map(|s| format!("{s:.1}")).unwrap_or_else(|| "n/a".into()),
                if others.is_empty() { "none".to_string() } else { others.join(", ") }
            ),
        ));

        // assessment: confidence of the decision, planned fallback
        let engine_conf = sel.confidence.clone().expect("feasible candidates have a confidence");
        let alt_best = candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != chosen && c.feasible)
            .filter_map(|(_, c)| c.score_ms)
            .fold(None, |a: Option<f64>, s| Some(a.map_or(s, |x| x.min(s))));
        let margin_factor = match (sel.score_ms, alt_best) {
            (Some(a), Some(b)) if a.max(b) > 0.0 => 0.6 + 0.4 * ((a - b).abs() / a.max(b) / 0.5).min(1.0),
            _ => 1.0,
        };
        let plan_conf = {
            let mut c = engine_conf.clone();
            c.score = (c.score * margin_factor).clamp(0.0, 1.0);
            c.label = confidence::label_for(c.score);
            c
        };
        let fallback = if sel.kind == EngineKind::Browser {
            candidates
                .iter()
                .filter(|c| c.feasible && c.kind == EngineKind::Server)
                .min_by(|a, b| a.score_ms.partial_cmp(&b.score_ms).unwrap_or(std::cmp::Ordering::Equal))
                .map(|c| FallbackPlan {
                    engine: c.engine.clone(),
                    reason: "if the browser run fails (any class except cancellation) the same logical plan runs once on the server; no retry on the browser".into(),
                    max_fallbacks: 1,
                })
        } else {
            None
        };
        let probes = engines
            .iter()
            .map(|e| {
                let id = e.id().to_string();
                let c = candidates.iter().find(|c| c.engine == e.id());
                let p = inputs.probes.get(&id);
                (
                    id,
                    ProbeSummary {
                        status: c.map_or("missing".into(), |c| c.probe_status.clone()),
                        usable: p.is_some_and(|p| p.usable()),
                        age_ms: p.map(|p| p.age_ms(inputs.now_ms)),
                        failed_checks: p.map(|p| p.checks.iter().filter(|c| !c.ok).map(|c| c.name.clone()).collect()).unwrap_or_default(),
                        memory_class: p.map(|p| p.memory_class),
                        engine_version: p.map(|p| p.engine_version.clone()),
                        runtime_version: p.map(|p| p.runtime_version.clone()),
                    },
                )
            })
            .collect();
        let health = candidates.iter().map(|c| (c.engine.to_string(), c.health.clone())).collect();
        let assessment = Assessment {
            engine_confidence: engine_conf.score,
            confidence: plan_conf,
            margin_factor,
            estimate: est,
            network: net.clone(),
            health,
            probes,
            fallback,
            policy,
            browser_warm: inputs.browser_warm,
        };

        let order = plan.topological_order().map_err(|e| PlanError::Other(e.to_string()))?;
        Ok(PhysicalPlan {
            plan_id: plan.plan_id(),
            engine: sel.engine.clone(),
            placement: sel.kind,
            stages: vec![Stage { engine: sel.engine.clone(), nodes: order.iter().map(|n| n.0.clone()).collect() }],
            candidates,
            scans,
            estimate,
            input_bytes_upper_bound: input_bytes,
            result_rows_estimate: result_rows,
            assessment,
            decisions: log,
            steps,
        })
    }

    fn candidate(&self, e: &EngineView<'_>, f: &PlanFacts<'_>, net: &NetworkProfile, apply_health: bool) -> Candidate {
        let cfg = &self.config;
        let caps = &e.caps;
        let browser = caps.kind == EngineKind::Browser;
        let (plan, scans, est, inputs) = (f.plan, f.scans, f.est, f.inputs);
        let (input_bytes, covering_total, result_bytes) = (f.input_bytes, f.covering_total, f.result_bytes);
        let result_rows = est.rows;
        let mut why_not: Vec<String> = Vec::new();
        let mut why: Vec<String> = Vec::new();

        let missing = caps.missing_ops(plan);
        if !missing.is_empty() {
            why_not.push(format!("unsupported operations: {}", missing.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ")));
        } else {
            why.push(format!("supports all {} required operation kinds", plan.required_ops().len()));
        }
        if !plan_sources_supported(plan, caps) {
            why_not.push("source format not supported".into());
        }
        if browser && !cfg.allow_browser {
            why_not.push("browser execution is disabled by configuration".into());
        }

        // memory: engine limit; for the browser either the operator override or the risk model.
        // A browser engine reads the covering columns of the whole file (no struct-stat pruning) on top
        // of the kept data: both count against its memory.
        let engine_input = if browser { input_bytes + covering_total } else { input_bytes };
        let mut input_limit = caps.limits.max_input_bytes;
        let mut rows_limit = caps.limits.max_result_rows;
        if browser {
            if let Some(o) = cfg.browser_max_input_bytes {
                input_limit = Some(input_limit.map_or(o, |l| l.min(o)));
            }
            rows_limit = Some(rows_limit.map_or(cfg.browser_max_result_rows, |l| l.min(cfg.browser_max_result_rows)));
        }
        if let Some(l) = input_limit {
            if engine_input > l {
                why_not.push(format!(
                    "estimated input {:.1} MB{} exceeds the engine limit {:.1} MB (memory guard)",
                    mb(engine_input),
                    if browser { " (kept data + covering columns of the whole file)" } else { "" },
                    mb(l)
                ));
            } else {
                why.push(format!("estimated input {:.1} MB within the engine limit {:.1} MB", mb(engine_input), mb(l)));
            }
        }
        if let Some(l) = rows_limit {
            if result_rows > l {
                why_not.push(format!("estimated result {result_rows} rows exceeds the engine limit {l}"));
            }
        }
        if f.has_bbox_filter && caps.bbox_filter_requires_covering {
            if let Some(s) = scans.iter().find(|s| !s.has_covering) {
                why_not.push(format!("bbox filter needs a bbox covering column, source {} has none", s.uri));
            }
        }

        // runtime probe (browser): what this runtime demonstrably does
        let probe_status = if browser {
            match e.probe_status {
                Freshness::Fresh => "fresh",
                Freshness::Stale => "stale",
                Freshness::Missing => "missing",
            }
        } else {
            "not_required"
        };
        let mut format_risk = 0.0;
        let mut ops_certain = true;
        match (browser, e.probe) {
            (true, Some(p)) => {
                if !p.usable() {
                    why_not.push(format!(
                        "runtime probe failed: {}",
                        p.failed_required().iter().map(|c| format!("{} ({})", c.name, c.detail)).collect::<Vec<_>>().join(", ")
                    ));
                }
                let unverified: Vec<&str> = plan.required_ops().into_iter().filter(|o| !p.supports_op(*o)).map(|o| o.name()).collect();
                if !unverified.is_empty() {
                    why_not.push(format!("runtime probe: operations not verified in this runtime: {}", unverified.join(", ")));
                    ops_certain = false;
                }
                for rc in f.read_classes.iter().copied().collect::<std::collections::BTreeSet<_>>() {
                    match p.reads_class(rc) {
                        Some(false) => why_not.push(format!("runtime probe: this browser runtime cannot read '{}' GeoParquet files", rc.as_str())),
                        Some(true) => {}
                        None => format_risk = 0.3,
                    }
                }
                if f.has_bbox_filter && p.check(probe_checks::COVERING).is_some_and(|c| !c.ok) {
                    why_not.push("runtime probe: covering-column bbox filtering failed in this runtime".into());
                }
            }
            (true, None) => {
                ops_certain = false;
                format_risk = 0.3;
                why.push("runtime not probed yet: capabilities are the static description only".into());
            }
            _ => {}
        }
        if browser && e.probe_status == Freshness::Stale {
            why.push("runtime probe is stale (older than its TTL)".into());
        }

        // observed health
        if apply_health && e.health.state == HealthState::Unhealthy {
            why_not.push(format!("engine unhealthy: {}", e.health.reason));
        }
        // known deterministic failures of this engine on these sources
        for k in &inputs.incompat {
            if k.engine == e.id.0 && f.source_keys.iter().any(|s| *s == k.source) {
                why_not.push(format!("failed before on source {} ({:?}: {}); not repeated", k.source, k.class, k.message));
            }
        }

        // browser risk (replaces the fixed memory constant)
        let risk = browser.then(|| {
            let (mem_class, dev_mem) = e.probe.map_or((MemoryClass::Unknown, None), |p| (p.memory_class, p.device_memory_gb));
            let (threshold, basis) = crash_threshold_bytes(mem_class, dev_mem);
            let memory = engine_input as f64 / threshold as f64;
            let rows = result_rows as f64 / cfg.browser_max_result_rows.max(1) as f64;
            let probe_r = match e.probe_status {
                Freshness::Fresh => 0.0,
                Freshness::Stale => 0.15,
                Freshness::Missing => 0.3,
            };
            let health_r = match e.health.state {
                HealthState::Healthy => 0.0,
                HealthState::Unknown => 0.1,
                HealthState::Degraded => 0.4,
                HealthState::Unhealthy => 1.0,
            };
            let parts = [memory.clamp(0.0, 1.0), rows.clamp(0.0, 1.0), probe_r, health_r, format_risk];
            BrowserRisk {
                score: 1.0 - parts.iter().map(|p| 1.0 - p).product::<f64>(),
                memory,
                result_rows: rows,
                probe: probe_r,
                health: health_r,
                format: format_risk,
                memory_input_bytes: engine_input,
                crash_threshold_bytes: threshold,
                threshold_basis: basis.into(),
            }
        });
        if let Some(r) = &risk {
            if cfg.browser_max_input_bytes.is_none() && r.memory > cfg.browser_reject_risk {
                why_not.push(format!(
                    "browser memory risk {:.2} exceeds {:.2}: estimated input {:.1} MB vs crash threshold {:.0} MB ({})",
                    r.memory,
                    cfg.browser_reject_risk,
                    mb(r.memory_input_bytes),
                    mb(r.crash_threshold_bytes),
                    r.threshold_basis
                ));
            } else {
                why.push(format!("browser risk {:.2} (memory {:.2} of the crash threshold)", r.score, r.memory));
            }
        }

        let feasible = why_not.is_empty();
        let cost = feasible.then(|| {
            let c = &cfg.cost;
            match caps.kind {
                EngineKind::Server => {
                    let latency = net.turn_ms();
                    CostBreakdown {
                        fixed_ms: c.server_fixed_ms,
                        scan_ms: mb(input_bytes) * c.server_ms_per_input_mb,
                        result_ms: result_rows as f64 * c.server_us_per_result_row / 1000.0,
                        transfer_ms: latency + net.upload_ms(c.request_bytes) + net.download_ms(result_bytes),
                        latency_ms: latency,
                        cold_start_ms: 0.0,
                        scan_bytes: input_bytes,
                        transfer_bytes: result_bytes,
                    }
                }
                EngineKind::Browser => {
                    let rg_total: usize = scans.iter().map(|s| s.prune.total_row_groups).sum();
                    let rg_kept: usize = scans.iter().map(|s| s.prune.kept.len()).sum();
                    let requests = 2.0 + rg_total as f64 + 2.0 * rg_kept as f64;
                    let turns = (requests / c.range_request_parallelism.max(1.0)).ceil();
                    let latency = net.turn_ms() * (1.0 + turns);
                    let cold = if inputs.browser_warm == Some(false) { c.browser_cold_start_ms } else { 0.0 };
                    CostBreakdown {
                        fixed_ms: c.browser_fixed_ms + cold,
                        scan_ms: mb(covering_total) * c.browser_ms_per_covering_mb + mb(input_bytes) * c.browser_ms_per_input_mb,
                        result_ms: result_rows as f64 * c.browser_us_per_result_row / 1000.0,
                        transfer_ms: latency + net.download_ms(input_bytes + covering_total),
                        latency_ms: latency,
                        cold_start_ms: cold,
                        scan_bytes: input_bytes + covering_total,
                        transfer_bytes: input_bytes + covering_total,
                    }
                }
            }
        });
        let score = cost.as_ref().map(|c| c.fixed_ms + c.scan_ms + c.result_ms + c.transfer_ms);
        if let (Some(c), Some(sc)) = (&cost, score) {
            match caps.kind {
                EngineKind::Server => why.push(format!(
                    "scans next to the data; only the result travels (~{:.2} MB over {}); score {sc:.1} ms",
                    mb(c.transfer_bytes),
                    net.name
                )),
                EngineKind::Browser => why.push(format!(
                    "no server execution, but {:.1} MB of input must be fetched into the browser over {} ({:.0} ms latency){}; score {sc:.1} ms",
                    mb(c.transfer_bytes),
                    net.name,
                    c.latency_ms,
                    if c.cold_start_ms > 0.0 { format!(" + {:.0} ms cold start", c.cold_start_ms) } else { String::new() }
                )),
            }
        }

        // confidence (only meaningful when feasible)
        let confidence = feasible.then(|| {
            use confidence::*;
            let transfer_share = match (&cost, score) {
                (Some(c), Some(s)) if s > 0.0 => (c.transfer_ms / s).clamp(0.0, 1.0),
                _ => 0.0,
            };
            let needs_cov = browser && f.has_bbox_filter;
            let cov = match (est.covering_quality, needs_cov) {
                (CoveringQuality::Declared, _) => 1.0,
                (CoveringQuality::Inferred, true) => 0.75,
                (CoveringQuality::None, true) => 0.3,
                (_, false) => 1.0,
            };
            let probe_v = match (browser, e.probe_status) {
                (false, _) => 1.0,
                (true, Freshness::Fresh) => 1.0,
                (true, Freshness::Stale) => 0.6,
                (true, Freshness::Missing) => 0.35,
            };
            let health_v = match e.health.state {
                HealthState::Healthy => 1.0,
                HealthState::Unknown => 0.8,
                HealthState::Degraded => 0.5,
                HealthState::Unhealthy => 0.2,
            };
            let net_v = 1.0 - net.uncertainty() * transfer_share;
            let fam_v = match f.familiarity {
                0 => 0.6,
                1 | 2 => 0.8,
                _ => 1.0,
            };
            let ops_v = if !browser || (ops_certain && e.probe.is_some()) { 1.0 } else { 0.7 };
            combine(vec![
                factor("estimate_quality", est.quality.score(), W_ESTIMATE, format!("result-size estimate quality {:?}", est.quality)),
                factor(
                    "covering_quality",
                    cov,
                    W_COVERING,
                    format!("bbox covering {:?}{}", est.covering_quality, if needs_cov { " (this engine depends on it)" } else { "" }),
                ),
                factor(
                    "probe_freshness",
                    probe_v,
                    W_PROBE,
                    if browser { format!("runtime probe {probe_status}") } else { "in-process engine: no runtime probe needed".into() },
                ),
                factor("engine_health", health_v, W_HEALTH, format!("{:?}: {}", e.health.state, e.health.reason)),
                factor(
                    "network_uncertainty",
                    net_v,
                    W_NETWORK,
                    format!("{} ({:?}), network is {:.0}% of the estimated time", net.name, net.origin, transfer_share * 100.0),
                ),
                factor("dataset_familiarity", fam_v, W_FAMILIARITY, format!("{} earlier observation(s) on this source", f.familiarity)),
                factor(
                    "operation_support",
                    ops_v,
                    W_OPS,
                    if ops_v >= 1.0 { "operations verified (probe or in-process engine)" } else { "operation support from the static description only" },
                ),
            ])
        });

        Candidate {
            engine: e.id.clone(),
            kind: caps.kind,
            feasible,
            score_ms: score,
            cost,
            reasons: why,
            rejected_because: why_not,
            confidence,
            risk,
            health: e.health.clone(),
            probe_status: probe_status.into(),
        }
    }
}

fn plan_sources_supported(plan: &Plan, caps: &Capabilities) -> bool {
    plan.nodes.values().all(|n| match n {
        PlanNode::Source { format, .. } => caps.sources.contains(format),
        _ => true,
    })
}
