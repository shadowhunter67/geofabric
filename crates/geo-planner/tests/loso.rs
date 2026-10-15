//! Leave-one-source-out check of the estimator calibration on the calibration set only (no held-out data involved):
//! each source's windows are estimated with a calibration fitted on ALL OTHER sources. This is what "a new dataset that
//! looks like known ones" will experience, and it is the guard against the cross-distribution leakage v2 had.

use geo_planner::{bucket_of, error_factor, summarize_errors, Calibration, Observation};
use std::path::PathBuf;

fn load() -> Option<Vec<Observation>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/results/calibration-v3.jsonl");
    let text = std::fs::read_to_string(p).ok()?;
    Some(text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect())
}

fn loso(obs: &[Observation], baseline_none: bool) -> (Vec<(f64, f64)>, usize, usize) {
    let sources: std::collections::BTreeSet<&str> = obs.iter().map(|o| o.source.as_str()).collect();
    let mut pairs = Vec::new();
    let (mut class_hits, mut total) = (0, 0);
    for s in sources {
        let train: Vec<Observation> = obs.iter().filter(|o| o.source != s).cloned().collect();
        let cal = Calibration::fit(&train);
        for o in obs.iter().filter(|o| o.source == s && o.kind == "cardinality") {
            let corr = cal.selectivity_correction(&o.source, &o.layout, bucket_of(o.raw_selected, o.total_rows));
            total += 1;
            if corr.basis == "class" {
                class_hits += 1;
            }
            let factor = if baseline_none { 1.0 } else { corr.factor };
            pairs.push((o.raw_selected * factor, o.actual_rows as f64));
        }
    }
    (pairs, class_hits, total)
}

#[test]
fn leave_one_source_out_error_is_bounded_and_better_than_no_calibration() {
    let Some(obs) = load() else { return eprintln!("SKIP: benchmarks/results/calibration-v3.jsonl missing") };
    let (cal, hits, total) = loso(&obs, false);
    let (none, _, _) = loso(&obs, true);
    let (c, n) = (summarize_errors(&cal, None).unwrap(), summarize_errors(&none, None).unwrap());
    eprintln!("LOSO n={} class-corrected {hits}/{total}: calibrated median {:.2} P90 {:.2} P95 {:.2} max {:.1} unsafe-under {:.2}; uncalibrated median {:.2} P90 {:.2} max {:.1}", c.n, c.median, c.p90, c.p95, c.max, c.unsafe_under_rate, n.median, n.p90, n.max);
    assert!(c.median < 2.0, "median {}", c.median);
    assert!(c.p90 <= n.p90 * 1.01, "calibration must not make the tail worse than no calibration: {} vs {}", c.p90, n.p90);
    assert!(c.unsafe_under_rate <= n.unsafe_under_rate + 0.02, "calibration must not add dangerous under-estimates: {} vs {}", c.unsafe_under_rate, n.unsafe_under_rate);
    let _ = error_factor(1.0, 1.0);
}
