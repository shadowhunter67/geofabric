//! Network model (ADR-010): the cost of moving bytes between the user, the gateway and the data.
//!
//! The fitted compute terms of the cost model were measured with browser and gateway on one host, so
//! they contain no network. The network is therefore modelled explicitly and *added*:
//!
//! ```text
//! server_total  = compute + RTT + overhead + upload(request) + result_bytes / down
//! browser_total = compute + (RTT + overhead)                      -- the plan request
//!                         + ceil(requests / parallelism) * (RTT + overhead)   -- Parquet range requests
//!                         + (covering + kept bytes) / down
//! ```

use serde::{Deserialize, Serialize};

/// Where the numbers of a profile come from. UIs show it next to the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkOrigin {
    /// A named preset (an assumption about a typical link).
    Preset,
    /// Set by the operator/user for this deployment.
    Configured,
    /// Measured on the actual link (the local profile on this host was measured: loopback).
    Measured,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkProfile {
    pub name: String,
    pub rtt_ms: f64,
    /// Megabits per second towards the user.
    pub down_mbps: f64,
    /// Megabits per second from the user.
    pub up_mbps: f64,
    /// Per-request overhead (TLS/HTTP framing, server think time) on top of the RTT.
    pub request_overhead_ms: f64,
    pub origin: NetworkOrigin,
}

pub const PROFILE_NAMES: [&str; 5] = ["LOCAL", "FAST_LAN", "GOOD_BROADBAND", "MOBILE", "HIGH_LATENCY"];

impl NetworkProfile {
    pub fn local() -> Self {
        // loopback: the calibration host. 8000 Mbps = 1000 MB/s, the figure the fit assumed.
        NetworkProfile { name: "LOCAL".into(), rtt_ms: 0.0, down_mbps: 8000.0, up_mbps: 8000.0, request_overhead_ms: 0.0, origin: NetworkOrigin::Measured }
    }

    fn preset(name: &str, rtt_ms: f64, down_mbps: f64, up_mbps: f64, overhead: f64) -> Self {
        NetworkProfile { name: name.into(), rtt_ms, down_mbps, up_mbps, request_overhead_ms: overhead, origin: NetworkOrigin::Preset }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_uppercase().as_str() {
            "LOCAL" => Self::local(),
            "FAST_LAN" => Self::preset("FAST_LAN", 5.0, 1000.0, 1000.0, 1.0),
            "GOOD_BROADBAND" => Self::preset("GOOD_BROADBAND", 20.0, 200.0, 50.0, 2.0),
            "MOBILE" => Self::preset("MOBILE", 70.0, 30.0, 10.0, 5.0),
            "HIGH_LATENCY" => Self::preset("HIGH_LATENCY", 150.0, 20.0, 5.0, 5.0),
            _ => return None,
        })
    }

    pub fn all() -> Vec<NetworkProfile> {
        PROFILE_NAMES.iter().filter_map(|n| Self::by_name(n)).collect()
    }

    /// Time to move `bytes` towards the user (no latency).
    pub fn download_ms(&self, bytes: u64) -> f64 {
        bytes as f64 * 8.0 / (self.down_mbps * 1e6) * 1000.0
    }

    pub fn upload_ms(&self, bytes: u64) -> f64 {
        bytes as f64 * 8.0 / (self.up_mbps * 1e6) * 1000.0
    }

    /// One request/response turn: RTT plus per-request overhead.
    pub fn turn_ms(&self) -> f64 {
        self.rtt_ms + self.request_overhead_ms
    }

    /// How uncertain the numbers are, 0 (measured) .. 1 (pure guess).
    pub fn uncertainty(&self) -> f64 {
        match self.origin {
            NetworkOrigin::Measured => 0.0,
            NetworkOrigin::Configured => 0.2,
            NetworkOrigin::Preset => 0.4,
        }
    }
}

impl Default for NetworkProfile {
    fn default() -> Self {
        NetworkProfile::local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_the_brief() {
        let lan = NetworkProfile::by_name("fast_lan").unwrap();
        assert_eq!((lan.rtt_ms, lan.down_mbps), (5.0, 1000.0));
        let b = NetworkProfile::by_name("GOOD_BROADBAND").unwrap();
        assert_eq!((b.rtt_ms, b.down_mbps), (20.0, 200.0));
        let m = NetworkProfile::by_name("MOBILE").unwrap();
        assert_eq!((m.rtt_ms, m.down_mbps), (70.0, 30.0));
        let h = NetworkProfile::by_name("HIGH_LATENCY").unwrap();
        assert_eq!((h.rtt_ms, h.down_mbps), (150.0, 20.0));
        assert_eq!(NetworkProfile::all().len(), 5);
        assert!(NetworkProfile::by_name("nope").is_none());
    }

    #[test]
    fn transfer_arithmetic() {
        let m = NetworkProfile::by_name("MOBILE").unwrap();
        // 30 Mbps = 3.75 MB/s: 3.75e6 bytes take one second
        assert!((m.download_ms(3_750_000) - 1000.0).abs() < 1e-6);
        let l = NetworkProfile::local();
        assert!((l.download_ms(1_000_000_000) - 1000.0).abs() < 1e-6, "LOCAL = 1000 MB/s");
    }
}
