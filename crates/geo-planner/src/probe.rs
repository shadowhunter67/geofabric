//! Runtime capability probe model (ADR-009).
//!
//! Three layers describe what an engine can do, and the planner keeps them apart:
//! * **static capabilities** (`EngineDescriptor::capabilities`): what the engine is *designed* for;
//! * **runtime probe** ([`ProbeReport`]): what *this* runtime instance demonstrably does right now
//!   (the browser runs a battery of tiny checks against fixture files and reports);
//! * **observed health** (`failure::HealthTracker`): how real queries have been going.
//!
//! The probe is produced by the engine's own runtime (the browser), reported to the gateway, cached
//! per session and invalidated on page reload, engine-version change, fatal failure or TTL.

use geo_ir::OpKind;
use geo_source_geoparquet::ReadClass;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeCheck {
    pub name: String,
    pub ok: bool,
    /// A failed *required* check makes the engine unusable; a failed optional check is information.
    pub required: bool,
    pub detail: String,
    pub ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryClass {
    Low,
    Standard,
    High,
    Unknown,
}

/// Well-known check names (the TypeScript probe emits exactly these).
pub mod checks {
    pub const INIT: &str = "init";
    pub const WORKER: &str = "worker";
    pub const EXTENSION_LOAD: &str = "extension_load";
    pub const ARROW: &str = "arrow";
    pub const PARQUET: &str = "parquet";
    pub const GEOMETRY_WKB: &str = "geometry_wkb";
    pub const COVERING: &str = "covering";
    /// HTTP range requests reach the file endpoint and return the right bytes (needed for large files).
    pub const HTTP_RANGE: &str = "http_range";
    /// Prefix; one check per [`geo_source_geoparquet::ReadClass`]: `read_class:plain_wkb`, `read_class:duckdb_typed`.
    pub const READ_CLASS: &str = "read_class:";
}

/// What the planner needs to know about the device, nothing more (no fingerprinting surface: major version only,
/// no user-agent string, no screen, no timezone).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    /// `chrome` | `edge` | `firefox` | `safari` | `other`
    pub browser_family: String,
    pub browser_major: Option<u32>,
    /// `windows` | `macos` | `linux` | `android` | `ios` | `other`
    pub platform: String,
    pub hardware_concurrency: Option<u32>,
    pub wasm_simd: bool,
    /// SharedArrayBuffer usable (cross-origin isolation): needed for multi-threaded DuckDB-Wasm.
    pub wasm_threads: bool,
}

impl DeviceProfile {
    /// Browser families whose safe operating envelope was measured (`web/e2e/envelope.spec.ts`).
    pub fn envelope_measured(&self) -> bool {
        matches!(self.browser_family.as_str(), "chrome" | "edge")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub engine: String,
    /// Changes on every page load: a new session invalidates the previous report.
    pub session: String,
    /// Version of the engine build (e.g. the DuckDB-Wasm package version).
    pub engine_version: String,
    /// Underlying runtime version (e.g. the DuckDB core version the wasm build reports).
    pub runtime_version: String,
    pub captured_at_ms: u64,
    pub duration_ms: f64,
    pub checks: Vec<ProbeCheck>,
    pub memory_class: MemoryClass,
    /// `navigator.deviceMemory` (GB, capped by the browser at 8), if exposed.
    pub device_memory_gb: Option<f64>,
    /// `performance.memory.jsHeapSizeLimit` in MB, if exposed (Chromium only).
    pub js_heap_limit_mb: Option<u64>,
    /// Operation kinds (`OpKind::name`) that ran correctly in the probe.
    pub supported_ops: Vec<String>,
    /// Device facts the envelope depends on; absent in reports from older clients.
    #[serde(default)]
    pub device: Option<DeviceProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    Missing,
}

impl ProbeReport {
    pub fn check(&self, name: &str) -> Option<&ProbeCheck> {
        self.checks.iter().find(|c| c.name == name)
    }

    pub fn check_ok(&self, name: &str) -> bool {
        self.check(name).is_some_and(|c| c.ok)
    }

    pub fn failed_required(&self) -> Vec<&ProbeCheck> {
        self.checks.iter().filter(|c| c.required && !c.ok).collect()
    }

    /// All required checks passed.
    pub fn usable(&self) -> bool {
        self.failed_required().is_empty() && !self.checks.is_empty()
    }

    pub fn supports_op(&self, op: OpKind) -> bool {
        self.supported_ops.iter().any(|o| o == op.name())
    }

    /// `Some(true/false)` when the probe tested this file class, `None` when it did not.
    pub fn reads_class(&self, class: ReadClass) -> Option<bool> {
        self.check(&format!("{}{}", checks::READ_CLASS, class.as_str())).map(|c| c.ok)
    }

    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.captured_at_ms)
    }

    pub fn freshness(&self, now_ms: u64, ttl_ms: Option<u64>) -> Freshness {
        match ttl_ms {
            Some(t) if self.age_ms(now_ms) > t => Freshness::Stale,
            _ => Freshness::Fresh,
        }
    }
}

/// Safe operating envelope of the browser engine (range access), from `web/e2e/envelope.spec.ts`
/// (benchmarks/results/envelope-*.json): Chrome 8 GB-class machine, headless, LOCAL network, 23 runs on 1 GB sources.
/// * aggregates over a 1 GB file that pulled **976 MB** (the whole file) ran in 4-8 s without a crash; the Overture
///   subset (1.38 GB, 219 MB pulled) in 4 s -> nothing above ~1 GB was tried: that is where evidence ends;
/// * results are the other limit: 100 000 rows cost 61 MB of JS heap on the synthetic files and **430 MB** on Overture
///   (many columns) and 10-34 s of conversion time.
/// Memory classes below 8 GB scale the numbers linearly (an assumption: no such device was measured).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Envelope {
    /// Pulling at most this many bytes into the tab was always fine (many runs, both browsers).
    pub safe_fetch_bytes: u64,
    /// Above this nothing has been demonstrated (largest successful run ≈ 976 MB): treated as unsafe.
    pub unsafe_fetch_bytes: u64,
    pub safe_result_bytes: u64,
    pub unsafe_result_bytes: u64,
    pub basis: &'static str,
}

pub fn envelope(memory_class: MemoryClass, device_memory_gb: Option<f64>) -> Envelope {
    const MB: f64 = 1024.0 * 1024.0;
    let (scale, basis) = match (memory_class, device_memory_gb) {
        (MemoryClass::Unknown, _) | (_, None) => (0.5, "memory class unknown: half of the measured envelope"),
        (_, Some(gb)) => ((gb / 8.0).clamp(0.125, 1.0), "measured at deviceMemory=8 (Chrome, one machine); scaled linearly below (assumption)"),
    };
    Envelope {
        safe_fetch_bytes: (300.0 * MB * scale) as u64,
        unsafe_fetch_bytes: (1200.0 * MB * scale) as u64,
        safe_result_bytes: (150.0 * MB * scale) as u64,
        unsafe_result_bytes: (1000.0 * MB * scale) as u64,
        basis,
    }
}

/// How safe a browser workload is, judged against the [`Envelope`] with the *upper* bounds of the estimates
/// (an underestimate is the dangerous direction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSafety {
    /// Inside the envelope on a probed, healthy runtime.
    Safe,
    /// Inside the demonstrated range but beyond the comfortable one, or the runtime is degraded/stale.
    Risky,
    /// Beyond what was ever demonstrated: never executed automatically.
    Unsafe,
    /// No evidence about this runtime (not probed, memory class unknown): never chosen on cost alone.
    Unknown,
}

pub fn memory_class_for(device_memory_gb: Option<f64>) -> MemoryClass {
    match device_memory_gb {
        None => MemoryClass::Unknown,
        Some(g) if g <= 2.0 => MemoryClass::Low,
        Some(g) if g < 8.0 => MemoryClass::Standard,
        Some(_) => MemoryClass::High,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample(now: u64) -> ProbeReport {
        let c = |name: &str, ok: bool, required: bool| ProbeCheck { name: name.into(), ok, required, detail: String::new(), ms: 1.0 };
        ProbeReport {
            engine: "duckdb-wasm".into(),
            session: "s1".into(),
            engine_version: "1.33".into(),
            runtime_version: "v1.4".into(),
            captured_at_ms: now,
            duration_ms: 900.0,
            checks: vec![
                c("init", true, true),
                c("parquet", true, true),
                c("read_class:plain_wkb", true, false),
                c("read_class:duckdb_typed", false, false),
            ],
            memory_class: MemoryClass::High,
            device_memory_gb: Some(8.0),
            js_heap_limit_mb: Some(4096),
            supported_ops: vec!["source".into(), "filter".into(), "bbox_filter".into()],
            device: None,
        }
    }

    #[test]
    fn probe_queries() {
        let p = sample(1000);
        assert!(p.usable());
        assert_eq!(p.reads_class(ReadClass::PlainWkb), Some(true));
        assert_eq!(p.reads_class(ReadClass::DuckDbTyped), Some(false));
        assert!(p.supports_op(OpKind::BboxFilter) && !p.supports_op(OpKind::Intersects));
        assert_eq!(p.freshness(1000 + 10, Some(100)), Freshness::Fresh);
        assert_eq!(p.freshness(1000 + 1000, Some(100)), Freshness::Stale);
        let mut bad = p.clone();
        bad.checks[0].ok = false;
        assert!(!bad.usable());
    }

    #[test]
    fn envelope_scales_with_memory() {
        let hi = envelope(MemoryClass::High, Some(8.0));
        let lo = envelope(MemoryClass::Low, Some(2.0));
        let unk = envelope(MemoryClass::Unknown, None);
        assert!(lo.safe_fetch_bytes < hi.safe_fetch_bytes && unk.safe_fetch_bytes < hi.safe_fetch_bytes);
        assert_eq!(hi.safe_fetch_bytes, 300 * 1024 * 1024);
        assert!(hi.unsafe_fetch_bytes > hi.safe_fetch_bytes && hi.unsafe_result_bytes > hi.safe_result_bytes);
    }
}
