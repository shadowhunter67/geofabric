//! Failure taxonomy (ADR-010) and the engine health tracker.
//!
//! Failures are classified once, here, so that the gateway, the browser client and the planner agree on
//! what happened. The class decides three things: does it count against the engine's *health*, is it
//! *deterministic* (same plan + same source fails the same way again -> remember it, never retry), and
//! what recovery is allowed (v1: at most one fallback to another engine, never a retry on the same one).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureClass {
    /// The engine/runtime lacks something it was assumed to have (extension, API, feature).
    Capability,
    /// Out of memory / memory guard tripped.
    Memory,
    Timeout,
    /// The user (or client disconnect) cancelled: never a failure of the engine.
    Cancelled,
    /// The engine process/worker/tab died.
    EngineCrash,
    /// The engine cannot read this file/geometry format.
    UnsupportedFormat,
    /// The plan contains an operation the engine cannot run.
    UnsupportedOperation,
    /// Network hiccup, aborted fetch, overloaded server: may succeed if tried again later.
    Transient,
}

impl FailureClass {
    pub const ALL: [FailureClass; 8] = [
        FailureClass::Capability,
        FailureClass::Memory,
        FailureClass::Timeout,
        FailureClass::Cancelled,
        FailureClass::EngineCrash,
        FailureClass::UnsupportedFormat,
        FailureClass::UnsupportedOperation,
        FailureClass::Transient,
    ];

    /// The same plan on the same engine and source fails the same way again.
    pub fn deterministic(self) -> bool {
        matches!(self, FailureClass::Capability | FailureClass::UnsupportedFormat | FailureClass::UnsupportedOperation)
    }

    /// Whether the failure says something about the engine's *health* (rather than about the workload).
    pub fn counts_for_health(self) -> bool {
        matches!(self, FailureClass::Memory | FailureClass::Timeout | FailureClass::EngineCrash | FailureClass::Transient)
    }

    /// Whether a fallback to another engine is allowed. Cancellation is the user's decision: no fallback.
    pub fn fallback_allowed(self) -> bool {
        self != FailureClass::Cancelled
    }

    /// Same-engine retries are never allowed in v1 (bounded recovery: one fallback, no loops).
    pub fn retry_allowed(self) -> bool {
        false
    }

    /// Classify an error message coming from an engine (server error code or browser exception text).
    /// Order matters: the most specific signals first. Unknown text is `Transient` (cheapest recovery:
    /// one fallback, and it counts toward health so repeated unknowns still degrade the engine).
    pub fn classify(message: &str) -> FailureClass {
        let m = message.to_ascii_lowercase();
        let has = |ks: &[&str]| ks.iter().any(|k| m.contains(k));
        if has(&["cancel", "aborted by the user", "client disconnect"]) {
            FailureClass::Cancelled
        } else if has(&["crashed", "target closed", "worker terminated", "worker has been terminated", "engine_crash", "sigsegv", "panicked"]) {
            FailureClass::EngineCrash
        } else if has(&["out of memory", "outofmemory", "oom", "memory limit", "memory guard", "allocation failed", "cannot allocate", "std::bad_alloc", "limit_exceeded"]) {
            FailureClass::Memory
        } else if has(&["timed out", "timeout", "interrupted after", "execution_timeout"]) {
            FailureClass::Timeout
        } else if has(&["activetransaction", "unsupported geometry", "cannot read geometry", "not a parquet", "unsupported encoding", "unsupported_format", "invalid parquet", "geoparquet", "unsupported geometry encoding", "parquet:"]) {
            FailureClass::UnsupportedFormat
        } else if has(&["unsupported operation", "unsupported:", "not supported", "unsupported_operation", "unsupported op"]) {
            FailureClass::UnsupportedOperation
        } else if has(&["capability", "not available", "no such function", "unknown function", "extension"]) {
            FailureClass::Capability
        } else {
            FailureClass::Transient
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
    /// No recent observation: neither trusted nor distrusted.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthConfig {
    /// Only the last `window` health-relevant events count.
    pub window: usize,
    /// Events older than this are forgotten (health decays back to Unknown).
    pub ttl_ms: u64,
    /// After a crash/memory failure the engine is `Unhealthy` this long, then `Degraded` (half-open)
    /// until it succeeds again.
    pub cooldown_ms: u64,
    /// Failure rate in the window at/above which the engine is `Degraded`.
    pub degrade_fail_rate: f64,
    /// This many consecutive failures make the engine `Unhealthy` (during the cooldown).
    pub unhealthy_consecutive: usize,
}

impl Default for HealthConfig {
    fn default() -> Self {
        HealthConfig { window: 12, ttl_ms: 15 * 60_000, cooldown_ms: 30_000, degrade_fail_rate: 0.25, unhealthy_consecutive: 3 }
    }
}

#[derive(Debug, Clone)]
struct Event {
    ts_ms: u64,
    ok: bool,
    class: Option<FailureClass>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthView {
    pub state: HealthState,
    pub reason: String,
    /// Health-relevant events considered.
    pub events: usize,
    pub failures: usize,
    pub last_failure: Option<FailureClass>,
    pub engine_version: Option<String>,
}

impl HealthView {
    pub fn unknown() -> Self {
        HealthView { state: HealthState::Unknown, reason: "no recent observation".into(), events: 0, failures: 0, last_failure: None, engine_version: None }
    }
}

#[derive(Debug, Clone, Default)]
struct EngineLog {
    version: Option<String>,
    events: VecDeque<Event>,
}

#[derive(Debug, Clone, Default)]
pub struct HealthTracker {
    pub cfg: HealthConfig,
    engines: BTreeMap<String, EngineLog>,
}

impl HealthTracker {
    pub fn new(cfg: HealthConfig) -> Self {
        HealthTracker { cfg, engines: BTreeMap::new() }
    }

    /// Record the engine's version. A change forgets the history (a new build is a new engine).
    /// Returns true when the history was reset.
    pub fn set_version(&mut self, engine: &str, version: &str) -> bool {
        let e = self.engines.entry(engine.to_string()).or_default();
        let changed = e.version.as_deref().is_some_and(|v| v != version);
        if changed {
            e.events.clear();
        }
        e.version = Some(version.to_string());
        changed
    }

    pub fn clear(&mut self, engine: &str) {
        if let Some(e) = self.engines.get_mut(engine) {
            e.events.clear();
        }
    }

    /// Record an outcome. Failures that do not concern engine health (deterministic workload problems,
    /// cancellations) are not recorded here; the caller keeps them as "known incompatibilities".
    pub fn record(&mut self, engine: &str, now_ms: u64, outcome: Result<(), FailureClass>) {
        let e = self.engines.entry(engine.to_string()).or_default();
        match outcome {
            Ok(()) => e.events.push_back(Event { ts_ms: now_ms, ok: true, class: None }),
            Err(c) if c.counts_for_health() => e.events.push_back(Event { ts_ms: now_ms, ok: false, class: Some(c) }),
            Err(_) => return,
        }
        while e.events.len() > self.cfg.window {
            e.events.pop_front();
        }
    }

    pub fn view(&self, engine: &str, now_ms: u64) -> HealthView {
        let Some(log) = self.engines.get(engine) else { return HealthView::unknown() };
        let c = &self.cfg;
        let ev: Vec<&Event> = log.events.iter().filter(|e| now_ms.saturating_sub(e.ts_ms) <= c.ttl_ms).collect();
        let version = log.version.clone();
        if ev.is_empty() {
            return HealthView { engine_version: version, ..HealthView::unknown() };
        }
        let failures = ev.iter().filter(|e| !e.ok).count();
        let last = ev[ev.len() - 1];
        let trailing = ev.iter().rev().take_while(|e| !e.ok).count();
        let last_failure = ev.iter().rev().find(|e| !e.ok).and_then(|e| e.class);
        let since_last = now_ms.saturating_sub(last.ts_ms);
        let (state, reason) = if !last.ok && matches!(last.class, Some(FailureClass::EngineCrash) | Some(FailureClass::Memory)) && since_last < c.cooldown_ms {
            (HealthState::Unhealthy, format!("last run ended in {:?} {} ms ago (cooldown {} ms)", last.class.unwrap(), since_last, c.cooldown_ms))
        } else if trailing >= c.unhealthy_consecutive && since_last < c.cooldown_ms {
            (HealthState::Unhealthy, format!("{trailing} consecutive failures, the last {} ms ago (cooldown {} ms)", since_last, c.cooldown_ms))
        } else if !last.ok {
            (HealthState::Degraded, format!("the last run failed ({:?}); degraded until the next success", last.class.unwrap_or(FailureClass::Transient)))
        } else if failures as f64 / ev.len() as f64 >= c.degrade_fail_rate {
            (HealthState::Degraded, format!("{failures} of the last {} runs failed", ev.len()))
        } else {
            (HealthState::Healthy, format!("{} of the last {} runs succeeded", ev.len() - failures, ev.len()))
        };
        HealthView { state, reason, events: ev.len(), failures, last_failure, engine_version: version }
    }
}

/// A workload that failed *deterministically* on an engine: the planner avoids repeating it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownIncompat {
    pub engine: String,
    pub source: String,
    pub class: FailureClass,
    pub message: String,
    pub ts_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_covers_every_class() {
        let cases = [
            ("query cancelled", FailureClass::Cancelled),
            ("Target crashed", FailureClass::EngineCrash),
            ("browser engine failed (engine: out of memory)", FailureClass::Memory),
            ("browser query interrupted after 30000 ms", FailureClass::Timeout),
            ("TransactionContext::ActiveTransaction called without active transaction", FailureClass::UnsupportedFormat),
            ("unsupported: spatial_join is not supported", FailureClass::UnsupportedOperation),
            ("Catalog Error: unknown function st_foo", FailureClass::Capability),
            ("Failed to fetch", FailureClass::Transient),
        ];
        for (m, c) in cases {
            assert_eq!(FailureClass::classify(m), c, "{m}");
        }
        for c in FailureClass::ALL {
            assert!(!c.retry_allowed(), "no same-engine retries in v1");
        }
        assert!(!FailureClass::Cancelled.fallback_allowed() && FailureClass::Timeout.fallback_allowed());
        assert!(FailureClass::UnsupportedFormat.deterministic() && !FailureClass::Transient.deterministic());
    }

    #[test]
    fn health_transitions() {
        let mut h = HealthTracker::new(HealthConfig::default());
        assert_eq!(h.view("wasm", 0).state, HealthState::Unknown);
        for t in 0..4 {
            h.record("wasm", t * 1000, Ok(()));
        }
        assert_eq!(h.view("wasm", 5000).state, HealthState::Healthy);
        h.record("wasm", 6000, Err(FailureClass::EngineCrash));
        assert_eq!(h.view("wasm", 7000).state, HealthState::Unhealthy, "a crash is unhealthy during the cooldown");
        assert_eq!(h.view("wasm", 6000 + 31_000).state, HealthState::Degraded, "after the cooldown: half-open (degraded)");
        h.record("wasm", 40_000, Ok(()));
        assert_eq!(h.view("wasm", 41_000).state, HealthState::Healthy, "recovered: last run ok and 1 failure in 6 runs is below the degrade rate");
    }

    #[test]
    fn deterministic_and_cancelled_failures_do_not_touch_health() {
        let mut h = HealthTracker::new(HealthConfig::default());
        h.record("wasm", 0, Ok(()));
        h.record("wasm", 1, Err(FailureClass::UnsupportedFormat));
        h.record("wasm", 2, Err(FailureClass::Cancelled));
        let v = h.view("wasm", 3);
        assert_eq!((v.state, v.failures), (HealthState::Healthy, 0));
    }

    #[test]
    fn version_change_and_ttl_reset_health() {
        let mut h = HealthTracker::new(HealthConfig::default());
        h.set_version("wasm", "1.0");
        h.record("wasm", 0, Err(FailureClass::EngineCrash));
        assert_eq!(h.view("wasm", 1).state, HealthState::Unhealthy);
        assert!(h.set_version("wasm", "1.1"), "new engine version forgets history");
        assert_eq!(h.view("wasm", 1).state, HealthState::Unknown);
        h.record("wasm", 0, Err(FailureClass::Transient));
        assert_eq!(h.view("wasm", HealthConfig::default().ttl_ms + 10).state, HealthState::Unknown, "old events decay");
    }

    #[test]
    fn trailing_failures_make_unhealthy() {
        let mut h = HealthTracker::new(HealthConfig::default());
        h.record("e", 0, Ok(()));
        for t in 1..=3 {
            h.record("e", t, Err(FailureClass::Timeout));
        }
        assert_eq!(h.view("e", 10).state, HealthState::Unhealthy);
    }
}

#[cfg(test)]
mod shared_fixture {
    use super::*;

    /// The same file is checked by the TypeScript classifier (web/test/failure.test.ts): the two
    /// implementations cannot drift apart silently.
    #[test]
    fn classification_matches_the_shared_fixture() {
        let text = include_str!("../tests/fixtures/failure-classes.json");
        let cases: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        assert!(cases.len() >= 15);
        for c in cases {
            let want: FailureClass = serde_json::from_value(c["class"].clone()).unwrap();
            let msg = c["message"].as_str().unwrap();
            assert_eq!(FailureClass::classify(msg), want, "{msg}");
        }
    }
}
