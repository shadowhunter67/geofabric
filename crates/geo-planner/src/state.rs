//! Planner state: what the planner *knows about the world* at planning time, separate from the plan.
//!
//! * runtime probes (per engine, per session) and their invalidation,
//! * observed health and known incompatibilities,
//! * telemetry of finished executions -> rolling calibration of the estimator,
//! * the network profile.
//!
//! The planner itself stays a pure function of `(plan, engines, ctx, PlanInputs)`; this type produces the
//! `PlanInputs` snapshot and is the only mutable part (one `Mutex`, short critical sections).

use crate::estimator::{Calibration, Observation};
use crate::failure::{FailureClass, HealthConfig, HealthTracker, HealthView, KnownIncompat};
use crate::network::NetworkProfile;
use crate::probe::ProbeReport;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Everything the planner is told about the world for one planning call.
#[derive(Debug, Clone)]
pub struct PlanInputs {
    pub now_ms: u64,
    pub network: NetworkProfile,
    pub probes: BTreeMap<String, ProbeReport>,
    pub health: BTreeMap<String, HealthView>,
    pub incompat: Vec<KnownIncompat>,
    pub calibration: Calibration,
    /// Probes older than this are `Stale` (None = never).
    pub probe_ttl_ms: Option<u64>,
    /// The client's browser engine instance is already instantiated (`None` = unknown, treated as warm).
    pub browser_warm: Option<bool>,
}

impl Default for PlanInputs {
    fn default() -> Self {
        PlanInputs {
            now_ms: now_ms(),
            network: NetworkProfile::local(),
            probes: BTreeMap::new(),
            health: BTreeMap::new(),
            incompat: Vec::new(),
            calibration: Calibration::default(),
            probe_ttl_ms: Some(30 * 60_000),
            browser_warm: None,
        }
    }
}

/// What the gateway reports after a run (success or failure) so the planner can learn from it.
#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub engine: String,
    pub source: String,
    pub outcome: Result<(), (FailureClass, String)>,
    /// Telemetry for the estimator; only meaningful on success.
    pub observation: Option<Observation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateSummary {
    pub probes: BTreeMap<String, ProbeReport>,
    pub health: BTreeMap<String, HealthView>,
    pub incompat: Vec<KnownIncompat>,
    pub telemetry_observations: usize,
    pub learning: bool,
    pub network: NetworkProfile,
}

struct Inner {
    probes: BTreeMap<String, ProbeReport>,
    health: HealthTracker,
    incompat: Vec<KnownIncompat>,
    telemetry: Vec<Observation>,
    calibration: Calibration,
    network: NetworkProfile,
    learning: bool,
    sink: Option<PathBuf>,
    engines: Vec<String>,
}

pub struct PlannerState {
    inner: Mutex<Inner>,
    pub probe_ttl_ms: Option<u64>,
}

const MAX_TELEMETRY: usize = 5000;
const MAX_INCOMPAT: usize = 200;

impl PlannerState {
    pub fn new() -> Self {
        PlannerState {
            inner: Mutex::new(Inner {
                probes: BTreeMap::new(),
                health: HealthTracker::new(HealthConfig::default()),
                incompat: Vec::new(),
                telemetry: Vec::new(),
                calibration: Calibration::default(),
                network: NetworkProfile::local(),
                learning: true,
                sink: None,
                engines: Vec::new(),
            }),
            probe_ttl_ms: Some(30 * 60_000),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Engines whose health/probe the summary should list.
    pub fn register_engine(&self, id: &str) {
        let mut g = self.lock();
        if !g.engines.iter().any(|e| e == id) {
            g.engines.push(id.to_string());
        }
    }

    pub fn set_network(&self, profile: NetworkProfile) {
        self.lock().network = profile;
    }

    pub fn network(&self) -> NetworkProfile {
        self.lock().network.clone()
    }

    /// `learning = false` freezes the calibration (held-out evaluations must not learn from the test set).
    pub fn set_learning(&self, on: bool) {
        self.lock().learning = on;
    }

    pub fn set_sink(&self, path: Option<PathBuf>) {
        self.lock().sink = path;
    }

    /// Load past telemetry (JSON lines) and fit the calibration. Returns the number of observations.
    pub fn load_telemetry(&self, path: &std::path::Path) -> std::io::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        let obs: Vec<Observation> = text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect();
        let n = obs.len();
        let mut g = self.lock();
        g.telemetry.extend(obs);
        let skip = g.telemetry.len().saturating_sub(MAX_TELEMETRY);
        g.telemetry.drain(..skip);
        g.calibration = Calibration::fit(&g.telemetry);
        Ok(n)
    }

    /// Store a runtime probe. A new session replaces the old report; a different engine version also
    /// forgets the engine's health history and known incompatibilities (a new build is a new engine).
    pub fn put_probe(&self, report: ProbeReport) -> ProbeVerdict {
        let mut g = self.lock();
        let engine = report.engine.clone();
        let version_changed = g.health.set_version(&engine, &report.engine_version);
        if version_changed {
            g.incompat.retain(|i| i.engine != engine);
        }
        let replaced_session = g.probes.get(&engine).is_some_and(|p| p.session != report.session);
        g.probes.insert(engine, report);
        ProbeVerdict { version_changed, replaced_session }
    }

    /// Drop an engine's probe (fatal failure, explicit refresh). The client is expected to probe again.
    pub fn invalidate_probe(&self, engine: &str) -> bool {
        self.lock().probes.remove(engine).is_some()
    }

    pub fn probe(&self, engine: &str) -> Option<ProbeReport> {
        self.lock().probes.get(engine).cloned()
    }

    pub fn record(&self, r: ExecutionReport) {
        let mut g = self.lock();
        let now = now_ms();
        match &r.outcome {
            Ok(()) => {
                g.health.record(&r.engine, now, Ok(()));
                if let Some(o) = r.observation {
                    if g.learning {
                        if let Some(path) = g.sink.clone() {
                            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                                let _ = writeln!(f, "{}", serde_json::to_string(&o).unwrap_or_default());
                            }
                        }
                        g.telemetry.push(o);
                        if g.telemetry.len() > MAX_TELEMETRY {
                            g.telemetry.remove(0);
                        }
                        g.calibration = Calibration::fit(&g.telemetry);
                    }
                }
            }
            Err((class, msg)) => {
                g.health.record(&r.engine, now, Err(*class));
                if class.deterministic() {
                    g.incompat.retain(|i| !(i.engine == r.engine && i.source == r.source && i.class == *class));
                    g.incompat.push(KnownIncompat { engine: r.engine.clone(), source: r.source.clone(), class: *class, message: msg.chars().take(200).collect(), ts_ms: now });
                    if g.incompat.len() > MAX_INCOMPAT {
                        g.incompat.remove(0);
                    }
                }
                // a crash/memory failure means the probe's picture of the runtime is outdated
                if matches!(class, FailureClass::EngineCrash | FailureClass::Memory) {
                    g.probes.remove(&r.engine);
                }
            }
        }
    }

    pub fn snapshot(&self) -> PlanInputs {
        let g = self.lock();
        let now = now_ms();
        let mut health = BTreeMap::new();
        for e in &g.engines {
            health.insert(e.clone(), g.health.view(e, now));
        }
        PlanInputs {
            now_ms: now,
            network: g.network.clone(),
            probes: g.probes.clone(),
            health,
            incompat: g.incompat.clone(),
            calibration: g.calibration.clone(),
            probe_ttl_ms: self.probe_ttl_ms,
            browser_warm: None,
        }
    }

    pub fn summary(&self) -> StateSummary {
        let snap = self.snapshot();
        let g = self.lock();
        StateSummary {
            probes: snap.probes,
            health: snap.health,
            incompat: snap.incompat,
            telemetry_observations: g.telemetry.len(),
            learning: g.learning,
            network: g.network.clone(),
        }
    }

    /// Forget probes, health, known incompatibilities and telemetry (development / test aid).
    pub fn reset(&self) {
        let mut g = self.lock();
        g.probes.clear();
        g.health = HealthTracker::new(HealthConfig::default());
        g.incompat.clear();
        g.telemetry.clear();
        g.calibration = Calibration::default();
    }

    pub fn telemetry(&self) -> Vec<Observation> {
        self.lock().telemetry.clone()
    }
}

impl Default for PlannerState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProbeVerdict {
    pub version_changed: bool,
    pub replaced_session: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::HealthState;

    fn rep(engine: &str, session: &str, version: &str) -> ProbeReport {
        ProbeReport {
            engine: engine.into(), session: session.into(), engine_version: version.into(), runtime_version: "r".into(), captured_at_ms: now_ms(), duration_ms: 1.0,
            checks: vec![], memory_class: crate::probe::MemoryClass::High, device_memory_gb: Some(8.0), js_heap_limit_mb: None, supported_ops: vec![], device: None,
        }
    }

    #[test]
    fn crash_invalidates_the_probe_and_marks_unhealthy_then_version_change_resets() {
        let st = PlannerState::new();
        st.register_engine("wasm");
        let v = st.put_probe(rep("wasm", "s1", "1.0"));
        assert!(!v.version_changed);
        assert!(st.probe("wasm").is_some());
        st.record(ExecutionReport { engine: "wasm".into(), source: "a".into(), outcome: Err((FailureClass::EngineCrash, "Target crashed".into())), observation: None });
        assert!(st.probe("wasm").is_none(), "fatal failure invalidates the probe");
        assert_eq!(st.snapshot().health["wasm"].state, HealthState::Unhealthy);
        let v = st.put_probe(rep("wasm", "s2", "2.0"));
        assert!(v.version_changed);
        assert_eq!(st.snapshot().health["wasm"].state, HealthState::Unknown);
    }

    #[test]
    fn deterministic_failures_become_known_incompatibilities_without_hurting_health() {
        let st = PlannerState::new();
        st.register_engine("wasm");
        st.record(ExecutionReport { engine: "wasm".into(), source: "ov".into(), outcome: Err((FailureClass::UnsupportedFormat, "ActiveTransaction".into())), observation: None });
        let s = st.snapshot();
        assert_eq!(s.incompat.len(), 1);
        assert_eq!(s.health["wasm"].state, HealthState::Unknown);
        // the same failure again does not duplicate the entry
        st.record(ExecutionReport { engine: "wasm".into(), source: "ov".into(), outcome: Err((FailureClass::UnsupportedFormat, "ActiveTransaction".into())), observation: None });
        assert_eq!(st.snapshot().incompat.len(), 1);
    }

    #[test]
    fn frozen_learning_ignores_new_observations() {
        let st = PlannerState::new();
        st.set_learning(false);
        let o = Observation {
            ts_ms: 0, source: "a".into(), kind: "features".into(), engine: "duckdb".into(), raw_selected: 10.0, total_rows: 100, est_rows: 10, actual_rows: 5, limit: None,
            censored: false, group_by: None, est_ms: None, actual_ms: 1.0, network: "LOCAL".into(), fallback: false, layout: String::new(),
        };
        st.record(ExecutionReport { engine: "duckdb".into(), source: "a".into(), outcome: Ok(()), observation: Some(o) });
        assert_eq!(st.summary().telemetry_observations, 0);
        assert_eq!(st.snapshot().calibration.observations, 0);
    }
}
