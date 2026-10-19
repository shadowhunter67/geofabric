//! Browser cost model (planner hardening III). Fitted ONLY on the calibration set (`benchmarks/results/browser-model-fit.json`,
//! `benchmarks/fit_browser_model.py`), then frozen. Held-out evidence is a separate run; these constants are never refitted on it.
//!
//! The time is a sum of parts the engine itself measures: plan round trip, engine start (cold), opening the sources, the sequential
//! range requests and their bytes, decode + compute, and materialising the result (rows, geometry bytes, attribute bytes).
//! A throttled link costs a request `rtt_factor * rtt + turn_overhead` (the emulated link adds ~25 ms per request beyond the RTT).

use crate::network::NetworkProfile;

/// duckdb-eh.wasm + its worker: what a cold start downloads when the browser has no cached copy.
pub const WASM_BYTES: f64 = 36.7 * 1048576.0;
const MB: f64 = 1048576.0;

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct BrowserModel {
    pub plan_ms: f64,
    pub rtt_factor: f64,
    pub turn_overhead_ms: f64,
    pub init_cpu_ms: f64,
    pub asset_transfer_factor: f64,
    pub asset_rtt_factor: f64,
    pub asset_overhead_ms: f64,
    pub cold_first_query_ms: f64,
    pub full_open_ms: f64,
    pub full_open_rtt_factor: f64,
    pub full_transfer_factor: f64,
    pub full_open_overhead_ms: f64,
    pub full_first_ms: f64,
    pub full_first_ms_per_kept_mb: f64,
    pub full_first_ms_per_result_geom_mb: f64,
    pub range_first_ms: f64,
    pub range_request_cpu_ms: f64,
    pub range_request_rtt_factor: f64,
    pub range_request_overhead_ms: f64,
    pub range_transfer_factor: f64,
    pub result_ms: f64,
    pub result_ms_per_krow: f64,
    pub result_ms_per_geom_mb: f64,
    pub result_ms_per_attr_mb: f64,
}

impl Default for BrowserModel {
    fn default() -> Self {
        BrowserModel {
            plan_ms: 7.045, rtt_factor: 1.179, turn_overhead_ms: 24.815, init_cpu_ms: 388.27, asset_transfer_factor: 0.9458,
            asset_rtt_factor: 1.348, asset_overhead_ms: 15.457, cold_first_query_ms: 164.86, full_open_ms: 8.289, full_open_rtt_factor: 1.418,
            full_transfer_factor: 1.0173, full_open_overhead_ms: 15.55, full_first_ms: 4.229, full_first_ms_per_kept_mb: 0.5373,
            full_first_ms_per_result_geom_mb: 6.053, range_first_ms: 5.929, range_request_cpu_ms: 2.770, range_request_rtt_factor: 0.9674,
            range_request_overhead_ms: 25.686, range_transfer_factor: 1.005, result_ms: 0.7854, result_ms_per_krow: 3.2236,
            result_ms_per_geom_mb: 5.994, result_ms_per_attr_mb: 18.638,
        }
    }
}

/// Everything the model needs, all known before execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct BrowserInputs {
    pub cold: bool,
    pub has_full: bool,
    pub has_range: bool,
    /// Whole-file mode: bytes of the files / of their kept row groups.
    pub full_file_bytes: f64,
    pub full_kept_bytes: f64,
    /// Range mode: expected sequential requests and bytes pulled.
    pub range_requests: f64,
    pub range_bytes: f64,
    pub result_rows: f64,
    pub result_geom_bytes: f64,
    pub result_attr_bytes: f64,
}

/// The parts of the estimate, in milliseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct BrowserParts {
    pub plan_turn_ms: f64,
    pub cold_ms: f64,
    pub open_ms: f64,
    pub first_ms: f64,
    pub result_ms: f64,
    /// Network share of `plan_turn_ms` + `cold_ms` (turn, engine assets) and of `open_ms` + `first_ms` (file requests, bytes).
    pub network_fixed_ms: f64,
    pub network_scan_ms: f64,
}

impl BrowserParts {
    pub fn network_ms(&self) -> f64 {
        self.network_fixed_ms + self.network_scan_ms
    }

    pub fn total(&self) -> f64 {
        self.plan_turn_ms + self.cold_ms + self.open_ms + self.first_ms + self.result_ms
    }
}

impl BrowserModel {
    pub fn estimate(&self, net: &NetworkProfile, i: &BrowserInputs) -> BrowserParts {
        let rtt = net.rtt_ms;
        let thr = if rtt > 0.0 { 1.0 } else { 0.0 };
        let bytes_per_ms = net.down_mbps * 1e6 / 8.0 / 1000.0;
        let link = |bytes: f64| bytes / bytes_per_ms;
        let mut p = BrowserParts::default();
        let turn = self.rtt_factor * rtt + self.turn_overhead_ms * thr;
        p.plan_turn_ms = self.plan_ms + turn;
        p.network_fixed_ms += turn;
        if i.cold {
            let net_part = self.asset_transfer_factor * link(WASM_BYTES) + self.asset_rtt_factor * rtt + self.asset_overhead_ms * thr;
            p.cold_ms = self.init_cpu_ms + net_part + self.cold_first_query_ms;
            p.network_fixed_ms += net_part;
        }
        if i.has_full {
            let net_part = self.full_open_rtt_factor * rtt + self.full_transfer_factor * link(i.full_file_bytes) + self.full_open_overhead_ms * thr;
            p.open_ms = self.full_open_ms + net_part;
            p.network_scan_ms += net_part;
            p.first_ms += self.full_first_ms + self.full_first_ms_per_kept_mb * i.full_kept_bytes / MB + self.full_first_ms_per_result_geom_mb * i.result_geom_bytes / MB;
        }
        if i.has_range {
            let net_part = i.range_requests * (self.range_request_rtt_factor * rtt + self.range_request_overhead_ms * thr) + self.range_transfer_factor * link(i.range_bytes);
            p.first_ms += self.range_first_ms + i.range_requests * self.range_request_cpu_ms + net_part;
            p.network_scan_ms += net_part;
        }
        if i.result_rows >= 1.0 {
            p.result_ms = self.result_ms + self.result_ms_per_krow * i.result_rows / 1000.0 + self.result_ms_per_geom_mb * i.result_geom_bytes / MB + self.result_ms_per_attr_mb * i.result_attr_bytes / MB;
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_warm_range_is_tens_of_ms_and_cold_adds_the_engine_start() {
        let m = BrowserModel::default();
        let net = NetworkProfile::local();
        let i = BrowserInputs { has_range: true, range_requests: 10.0, range_bytes: 5.0 * MB, result_rows: 1.0, ..Default::default() };
        let warm = m.estimate(&net, &i).total();
        let cold = m.estimate(&net, &BrowserInputs { cold: true, ..i }).total();
        assert!(warm > 20.0 && warm < 120.0, "{warm}");
        assert!(cold - warm > 500.0 && cold - warm < 800.0, "{}", cold - warm);
    }

    #[test]
    fn a_slow_link_costs_turns_and_bytes() {
        let m = BrowserModel::default();
        let i = BrowserInputs { has_range: true, range_requests: 10.0, range_bytes: 30.0 * MB, result_rows: 1.0, ..Default::default() };
        let local = m.estimate(&NetworkProfile::local(), &i).total();
        let high = m.estimate(&NetworkProfile::by_name("HIGH_LATENCY").unwrap(), &i).total();
        assert!(high > local + 10.0 * 150.0 && high > local + 10_000.0, "{local} {high}");
    }
}
