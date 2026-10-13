//! Planner v1 (RFC-003): chooses *where* a logical plan runs.
//!
//! Input: PlanGraph + source metadata (footer statistics, via the access guard)
//! + engine descriptors (capabilities, limits). Output: a [`PhysicalPlan`] with
//! the selected engine, **every candidate with its score and reasons**, and a
//! decision log. The planner never executes anything and never names an engine
//! concretely: engines arrive only through [`EngineDescriptor`].
//!
//! Cost model (deliberately simple, all numbers configurable, calibrated from
//! benchmarks — see the multi-engine report):
//!
//! ```text
//! score_ms(server)  = server_fixed_ms  + input_bytes / server_scan_rate  + result_bytes / network_rate
//! score_ms(browser) = browser_fixed_ms + input_bytes / browser_scan_rate + input_bytes  / network_rate
//! ```
//!
//! i.e. a server engine scans next to the data and ships the *result*; a
//! browser engine must first pull the *input* it needs across the network.
//! Hard limits (browser memory, result rows, operations, covering column) make a
//! candidate infeasible — they are never traded against score.

use geo_executor::{
    Capabilities, EngineDescriptor, EngineId, EngineKind, Estimate, ExecutionContext, ExplainStep,
};
use geo_ir::{validate, OpKind, Plan, PlanNode, ValidationReport};
use geo_source_geoparquet::{analyze_plan, SourceScan};
use serde::Serialize;

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

/// Linear cost model parameters (ms, MB/s, bytes). Defaults are placeholders that
/// the benchmark calibration step replaces with measured values.
#[derive(Debug, Clone, Serialize)]
pub struct CostModel {
    pub server_fixed_ms: f64,
    pub browser_fixed_ms: f64,
    pub server_scan_mb_per_s: f64,
    pub browser_scan_mb_per_s: f64,
    pub network_mb_per_s: f64,
    pub avg_result_row_bytes: u64,
    /// Result rows assumed for a grouped aggregate (ungrouped = 1).
    pub aggregate_result_rows: u64,
}

impl Default for CostModel {
    fn default() -> Self {
        CostModel {
            server_fixed_ms: 40.0,
            browser_fixed_ms: 25.0,
            server_scan_mb_per_s: 800.0,
            browser_scan_mb_per_s: 150.0,
            network_mb_per_s: 100.0,
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
    /// Browser memory guard: estimated input above this makes the browser infeasible.
    pub browser_max_input_bytes: u64,
    /// Browser memory guard: estimated result rows above this make the browser infeasible.
    pub browser_max_result_rows: u64,
    pub cost: CostModel,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        PlannerConfig {
            warn_scan_bytes: 512 << 20,
            max_scan_bytes: 8 << 30,
            allow_browser: true,
            browser_max_input_bytes: 50 << 20,
            browser_max_result_rows: 100_000,
            cost: CostModel::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CostBreakdown {
    pub fixed_ms: f64,
    pub scan_ms: f64,
    pub transfer_ms: f64,
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

/// Result rows the plan can return, from the root operator and the scan bound.
fn result_rows_estimate(plan: &Plan, input_rows: u64, cost: &CostModel) -> u64 {
    let mut id = &plan.root;
    let mut cap = input_rows;
    loop {
        match plan.nodes.get(id) {
            Some(PlanNode::Limit { input, n }) => {
                cap = cap.min(*n);
                id = input;
            }
            Some(PlanNode::Sort { input, .. }) | Some(PlanNode::Project { input, .. }) => id = input,
            Some(PlanNode::Aggregate { group_by, .. }) => {
                return cap.min(if group_by.is_empty() { 1 } else { cost.aggregate_result_rows });
            }
            _ => return cap,
        }
    }
}

impl Planner {
    /// Plan against `engines` (registration order = tie-break order).
    pub fn plan(
        &self,
        plan: &Plan,
        engines: &[&dyn EngineDescriptor],
        ctx: &ExecutionContext,
    ) -> Result<PhysicalPlan, PlanError> {
        let cfg = &self.config;
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
        let input_rows: u64 = scans.iter().map(|s| s.prune.kept_rows).sum();
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
        let result_rows = result_rows_estimate(plan, input_rows, &cfg.cost);
        let result_bytes = result_rows.saturating_mul(cfg.cost.avg_result_row_bytes);
        let has_bbox_filter = plan.required_ops().contains(&OpKind::BboxFilter);

        // R3 candidates: feasibility (hard limits) then score
        let mut candidates: Vec<Candidate> = Vec::new();
        for e in engines {
            let caps = e.capabilities();
            let mut why_not: Vec<String> = Vec::new();
            let mut why: Vec<String> = Vec::new();
            let missing = caps.missing_ops(plan);
            if !missing.is_empty() {
                why_not.push(format!("unsupported operations: {}", missing.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ")));
            } else {
                why.push(format!("supports all {} required operation kinds", plan.required_ops().len()));
            }
            if !plan_sources_supported(plan, &caps) {
                why_not.push("source format not supported".into());
            }
            if caps.kind == EngineKind::Browser && !cfg.allow_browser {
                why_not.push("browser execution is disabled by configuration".into());
            }
            let mut input_limit = caps.limits.max_input_bytes;
            let mut rows_limit = caps.limits.max_result_rows;
            if caps.kind == EngineKind::Browser {
                input_limit = Some(input_limit.map_or(cfg.browser_max_input_bytes, |l| l.min(cfg.browser_max_input_bytes)));
                rows_limit = Some(rows_limit.map_or(cfg.browser_max_result_rows, |l| l.min(cfg.browser_max_result_rows)));
            }
            if let Some(l) = input_limit {
                if input_bytes > l {
                    why_not.push(format!("estimated input {:.1} MB exceeds the engine limit {:.1} MB (memory guard)", mb(input_bytes), mb(l)));
                } else {
                    why.push(format!("estimated input {:.1} MB within the engine limit {:.1} MB", mb(input_bytes), mb(l)));
                }
            }
            if let Some(l) = rows_limit {
                if result_rows > l {
                    why_not.push(format!("estimated result {result_rows} rows exceeds the engine limit {l}"));
                }
            }
            if has_bbox_filter && caps.bbox_filter_requires_covering {
                if let Some(s) = scans.iter().find(|s| !s.has_covering) {
                    why_not.push(format!("bbox filter needs a bbox covering column, source {} has none", s.uri));
                }
            }
            let feasible = why_not.is_empty();
            let cost = feasible.then(|| {
                let c = &cfg.cost;
                match caps.kind {
                    EngineKind::Server => CostBreakdown {
                        fixed_ms: c.server_fixed_ms,
                        scan_ms: mb(input_bytes) / c.server_scan_mb_per_s * 1000.0,
                        transfer_ms: mb(result_bytes) / c.network_mb_per_s * 1000.0,
                        scan_bytes: input_bytes,
                        transfer_bytes: result_bytes,
                    },
                    EngineKind::Browser => CostBreakdown {
                        fixed_ms: c.browser_fixed_ms,
                        scan_ms: mb(input_bytes) / c.browser_scan_mb_per_s * 1000.0,
                        transfer_ms: mb(input_bytes) / c.network_mb_per_s * 1000.0,
                        scan_bytes: input_bytes,
                        transfer_bytes: input_bytes,
                    },
                }
            });
            let score = cost.as_ref().map(|c| c.fixed_ms + c.scan_ms + c.transfer_ms);
            if let (Some(c), Some(sc)) = (&cost, score) {
                match caps.kind {
                    EngineKind::Server => why.push(format!(
                        "scans next to the data; only the result travels (~{:.2} MB over the network); score {sc:.1} ms",
                        mb(c.transfer_bytes)
                    )),
                    EngineKind::Browser => why.push(format!(
                        "no server execution, but {:.1} MB of input must be fetched into the browser; score {sc:.1} ms",
                        mb(c.transfer_bytes)
                    )),
                }
            }
            log.push(dec(
                "candidate",
                e.id().to_string(),
                if feasible { Outcome::Info } else { Outcome::Rejected },
                if feasible { why.join("; ") } else { why_not.join("; ") },
            ));
            candidates.push(Candidate { engine: e.id(), kind: caps.kind, feasible, score_ms: score, cost, reasons: why, rejected_because: why_not });
        }
        if !candidates.iter().any(|c| c.feasible) {
            return Err(PlanError::NoEngine(
                candidates.iter().map(|c| format!("{}: {}", c.engine, c.rejected_because.join(", "))).collect::<Vec<_>>().join("; "),
            ));
        }

        // R4 hint, then R5 lowest score (ties: first registered)
        let mut chosen: Option<usize> = None;
        if let Some(pref) = &plan.hints.preferred_engine {
            match candidates.iter().position(|c| c.engine.0 == *pref && c.feasible) {
                Some(i) => {
                    chosen = Some(i);
                    log.push(dec("hint", pref.clone(), Outcome::Info, "preferred_engine honored (feasible)"));
                }
                None => log.push(dec("hint", pref.clone(), Outcome::Warning, "preferred_engine ignored: not registered or infeasible for this plan")),
            }
        }
        let chosen = chosen.unwrap_or_else(|| {
            candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| c.feasible)
                .min_by(|(_, a), (_, b)| a.score_ms.partial_cmp(&b.score_ms).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .expect("a feasible candidate exists")
        });
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
            decisions: log,
            steps,
        })
    }
}

fn plan_sources_supported(plan: &Plan, caps: &Capabilities) -> bool {
    plan.nodes.values().all(|n| match n {
        PlanNode::Source { format, .. } => caps.sources.contains(format),
        _ => true,
    })
}
