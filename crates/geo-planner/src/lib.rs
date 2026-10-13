//! Rule-based planner v0 (RFC-003).
//!
//! Turns a logical plan + registered engines into a [`PhysicalPlan`]: the
//! chosen engine, scan analysis and a [`Decision`] log. The log is part of
//! the output contract (CONSTITUTION 13) — UIs render it, it is not debug
//! output. The planner never executes anything and never names an engine
//! concretely: engines arrive only through the [`ExecutionEngine`] trait.

use geo_executor::{Capabilities, EngineId, Estimate, ExecutionContext, ExecutionEngine, ExplainStep};
use geo_ir::{validate, Plan, ValidationReport};
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

#[derive(Debug, Clone, Serialize)]
pub struct PlannerConfig {
    /// Warn when the metadata-based estimate exceeds this many bytes.
    pub warn_scan_bytes: u64,
    /// Refuse plans whose metadata-based estimate exceeds this many bytes.
    pub max_scan_bytes: u64,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        PlannerConfig { warn_scan_bytes: 512 << 20, max_scan_bytes: 8 << 30 }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PhysicalPlan {
    pub plan_id: String,
    pub engine: EngineId,
    pub scans: Vec<SourceScan>,
    pub estimate: Estimate,
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

impl Planner {
    /// Plan against `engines` (registration order = tie-break order).
    pub fn plan(
        &self,
        plan: &Plan,
        engines: &[&dyn ExecutionEngine],
        ctx: &ExecutionContext,
    ) -> Result<PhysicalPlan, PlanError> {
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
            format!("IR valid ({} warning(s))", report.warnings().count()),
        ));
        for w in report.warnings() {
            log.push(dec("validate", w.node.clone(), Outcome::Warning, format!("[{}] {}", w.code, w.message)));
        }

        // R2 scan analysis (source metadata only; no data read)
        let scans = analyze_plan(plan).map_err(|e| PlanError::Other(e.to_string()))?;
        for s in &scans {
            log.push(dec(
                "spatial_prune",
                s.uri.clone(),
                if s.prune.skipped > 0 { Outcome::Info } else { Outcome::Warning },
                format!(
                    "{} constraint(s) pushed to source; skip {}/{} row groups ({:?}); {} of {} bytes remain{}",
                    s.constraints_applied,
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

        // R3 capability filter
        let required = plan.required_ops();
        let mut capable: Vec<(&dyn ExecutionEngine, Capabilities)> = Vec::new();
        for e in engines {
            let caps = e.capabilities();
            let missing = caps.missing_ops(plan);
            let src_ok = scans.is_empty() || plan_sources_supported(plan, &caps);
            if !missing.is_empty() {
                log.push(dec(
                    "capability",
                    e.id().to_string(),
                    Outcome::Rejected,
                    format!("missing operations: {}", missing.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ")),
                ));
            } else if !src_ok {
                log.push(dec("capability", e.id().to_string(), Outcome::Rejected, "source format not supported"));
            } else {
                log.push(dec(
                    "capability",
                    e.id().to_string(),
                    Outcome::Info,
                    format!("supports all {} required operation kinds; runs at: {}", required.len(), caps.location),
                ));
                capable.push((*e, caps));
            }
        }
        if capable.is_empty() {
            return Err(PlanError::NoEngine(
                log.iter()
                    .filter(|d| d.outcome == Outcome::Rejected)
                    .map(|d| format!("{}: {}", d.subject, d.reason))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }

        // R4 hint
        let mut chosen = 0usize;
        if let Some(pref) = &plan.hints.preferred_engine {
            match capable.iter().position(|(e, _)| e.id().0 == *pref) {
                Some(i) => {
                    chosen = i;
                    log.push(dec("hint", pref.clone(), Outcome::Info, "preferred_engine honored"));
                }
                None => log.push(dec(
                    "hint",
                    pref.clone(),
                    Outcome::Warning,
                    "preferred_engine ignored: not registered or not capable",
                )),
            }
        } else if capable.len() > 1 {
            log.push(dec(
                "tie_break",
                "engines",
                Outcome::Info,
                "several capable engines; v0 picks the first registered (cost model not implemented)",
            ));
        }
        let (engine, _) = &capable[chosen];

        // R5 cost guard from metadata
        let estimate = engine.estimate(plan).map_err(|e| PlanError::Other(e.to_string()))?;
        if let Some(b) = estimate.bytes_to_read {
            if b > self.config.max_scan_bytes {
                return Err(PlanError::TooExpensive(format!(
                    "estimated {b} bytes to read exceeds max_scan_bytes={}",
                    self.config.max_scan_bytes
                )));
            }
            if b > self.config.warn_scan_bytes {
                log.push(dec("cost_guard", "scan", Outcome::Warning, format!("estimated {b} bytes to read")));
            }
        }

        let steps = engine.explain(plan, ctx).map_err(|e| PlanError::Other(e.to_string()))?;
        log.push(dec(
            "select_engine",
            engine.id().to_string(),
            Outcome::Chosen,
            format!(
                "capable{}; estimated rows<={} bytes~{}",
                if capable.len() > 1 { " (first of several)" } else { " (only candidate)" },
                estimate.rows_upper_bound.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                estimate.bytes_to_read.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            ),
        ));

        Ok(PhysicalPlan { plan_id: plan.plan_id(), engine: engine.id(), scans, estimate, decisions: log, steps })
    }
}

fn plan_sources_supported(plan: &Plan, caps: &Capabilities) -> bool {
    fn walk(n: &geo_ir::Node, caps: &Capabilities) -> bool {
        if let geo_ir::Node::Source { format, .. } = n {
            return caps.sources.contains(format);
        }
        n.inputs().into_iter().all(|i| walk(i, caps))
    }
    walk(&plan.root, caps)
}
