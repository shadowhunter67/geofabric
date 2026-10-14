//! Estimator v2 (ADR-010): statistical, explainable result-size estimation with rolling calibration.
//!
//! Inputs are metadata only (footer statistics via [`SourceScan`]) plus a [`Calibration`] learned from
//! *previous* executions' telemetry. The estimator never sees the outcome of the query it estimates.
//!
//! ```text
//! raw      = sum over kept row groups of rows x (window ∩ row-group bbox)/area(row-group bbox)   (uniform density)
//!            x (1 - geometry null fraction)
//! selected = min( raw x correction(source, selectivity bucket), rows of the kept row groups )
//! result   = shape(selected): Limit caps it, Aggregate replaces it by the group count
//! interval = [result / spread, result x spread],  spread = 2 | 4 | 10 for Good | Fair | Poor quality
//! ```
//!
//! `correction` is a shrunk median of past `actual / raw` ratios (uncensored observations only: a result
//! that hit its LIMIT only bounds the selectivity from below). Source-specific history beats the global
//! per-bucket history, which beats no correction.

use crate::CostModel;
use geo_ir::{Plan, PlanNode};
use geo_source_geoparquet::SourceScan;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoveringQuality {
    /// Declared in the GeoParquet metadata (GeoParquet 1.1).
    Declared,
    /// Found by naming convention (struct column `bbox`): usable, but nobody promised it covers the geometry.
    Inferred,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateQuality {
    Poor,
    Fair,
    Good,
}

impl EstimateQuality {
    pub fn spread(self) -> f64 {
        match self {
            EstimateQuality::Good => 2.0,
            EstimateQuality::Fair => 4.0,
            EstimateQuality::Poor => 10.0,
        }
    }
    pub fn score(self) -> f64 {
        match self {
            EstimateQuality::Good => 1.0,
            EstimateQuality::Fair => 0.7,
            EstimateQuality::Poor => 0.4,
        }
    }
}

/// One finished execution, as telemetry. JSON lines on disk (`--telemetry`), bounded in memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub ts_ms: u64,
    pub source: String,
    /// Shape of the plan: `features`, `count_by`, `count`, ...
    pub kind: String,
    pub engine: String,
    /// `raw` of the estimator for this query (before any correction).
    pub raw_selected: f64,
    pub total_rows: u64,
    pub est_rows: u64,
    pub actual_rows: u64,
    pub limit: Option<u64>,
    /// `actual_rows` hit the plan's LIMIT: the true selectivity is only bounded from below.
    pub censored: bool,
    pub group_by: Option<String>,
    pub est_ms: Option<f64>,
    pub actual_ms: f64,
    pub network: String,
    pub fallback: bool,
}

/// Selectivity bucket by the fraction of the file a query is expected to select (raw estimate).
pub fn bucket_of(raw_selected: f64, total_rows: u64) -> u8 {
    if total_rows == 0 {
        return 0;
    }
    let f = raw_selected / total_rows as f64;
    match f {
        f if f < 1e-4 => 0,
        f if f < 1e-3 => 1,
        f if f < 1e-2 => 2,
        f if f < 1e-1 => 3,
        _ => 4,
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Learned corrections. Built from observations with [`Calibration::fit`]; cheap to clone.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Calibration {
    /// ln(actual+1 / raw+1) per (source, bucket).
    by_source: BTreeMap<(String, u8), Vec<f64>>,
    by_bucket: BTreeMap<u8, Vec<f64>>,
    /// Observed group counts per (source, column).
    groups: BTreeMap<(String, String), Vec<f64>>,
    all_groups: Vec<f64>,
    familiarity: BTreeMap<String, usize>,
    pub observations: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Correction {
    pub factor: f64,
    pub samples: usize,
    pub basis: &'static str,
}

impl Calibration {
    pub fn fit(obs: &[Observation]) -> Calibration {
        let mut c = Calibration::default();
        for o in obs {
            c.observations += 1;
            *c.familiarity.entry(o.source.clone()).or_default() += 1;
            if !o.censored && o.group_by.is_none() && o.raw_selected >= 0.0 {
                let r = ((o.actual_rows as f64 + 1.0) / (o.raw_selected + 1.0)).ln();
                let b = bucket_of(o.raw_selected, o.total_rows);
                c.by_source.entry((o.source.clone(), b)).or_default().push(r);
                c.by_bucket.entry(b).or_default().push(r);
            }
            if let Some(g) = &o.group_by {
                // grouped result below its LIMIT = the true group count
                if !o.censored {
                    c.groups.entry((o.source.clone(), g.clone())).or_default().push(o.actual_rows as f64);
                    c.all_groups.push(o.actual_rows as f64);
                }
            }
        }
        c
    }

    pub fn familiarity(&self, source: &str) -> usize {
        self.familiarity.get(source).copied().unwrap_or(0)
    }

    pub fn selectivity_correction(&self, source: &str, bucket: u8) -> Correction {
        let shrunk = |v: &Vec<f64>, prior_n: f64| {
            let n = v.len() as f64;
            (median(&mut v.clone()) * n / (n + prior_n)).exp().clamp(1.0 / 200.0, 200.0)
        };
        if let Some(v) = self.by_source.get(&(source.to_string(), bucket)).filter(|v| v.len() >= 2) {
            return Correction { factor: shrunk(v, 1.0), samples: v.len(), basis: "source" };
        }
        if let Some(v) = self.by_bucket.get(&bucket).filter(|v| v.len() >= 3) {
            return Correction { factor: shrunk(v, 2.0), samples: v.len(), basis: "global" };
        }
        Correction { factor: 1.0, samples: 0, basis: "none" }
    }

    /// Cardinality of a GROUP BY column: the largest group count ever observed for it on this source (a window can
    /// only show a subset of the groups, so the maximum is the best lower bound of the cardinality); without own
    /// history, the median of the per-column maxima over all sources. The result has at most `min(cardinality, rows)` groups.
    pub fn group_count(&self, source: &str, column: &str) -> Option<(f64, &'static str)> {
        let max = |v: &Vec<f64>| v.iter().cloned().fold(0.0f64, f64::max);
        if let Some(v) = self.groups.get(&(source.to_string(), column.to_string())).filter(|v| !v.is_empty()) {
            return Some((max(v), "source"));
        }
        (self.all_groups.len() >= 3).then(|| {
            let mut maxima: Vec<f64> = self.groups.values().map(max).collect();
            (median(&mut maxima), "global")
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RowEstimate {
    pub rows: u64,
    pub low: u64,
    pub high: u64,
    pub quality: EstimateQuality,
    pub covering_quality: CoveringQuality,
    /// Uncorrected selectivity estimate (what telemetry stores as `raw_selected`).
    pub raw_selected: f64,
    pub selected_after_correction: f64,
    pub correction: Correction,
    pub bucket: u8,
    pub spatial_coherence: Option<f64>,
    pub density_skew: Option<f64>,
    pub null_fraction: Option<f64>,
    pub row_groups_total: usize,
    pub row_groups_kept: usize,
    /// The result is bounded by a LIMIT that the selection exceeds with margin: the estimate is the limit.
    pub limit_bound: bool,
    pub group_basis: Option<&'static str>,
    pub notes: Vec<String>,
}

/// Shape of the result as far as the estimator is concerned: the root-most LIMIT and aggregate.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanShape {
    pub limit: Option<u64>,
    /// First GROUP BY column of the root aggregate.
    pub group_by: Option<String>,
    /// Root aggregate without GROUP BY: exactly one row.
    pub ungrouped: bool,
}

impl PlanShape {
    /// `count_by` | `count` | `features`: the telemetry label of the plan shape.
    pub fn kind(&self) -> &'static str {
        if self.group_by.is_some() {
            "count_by"
        } else if self.ungrouped {
            "count"
        } else {
            "features"
        }
    }
}

pub fn plan_shape(plan: &Plan) -> PlanShape {
    let mut shape = PlanShape::default();
    let mut id = &plan.root;
    loop {
        match plan.nodes.get(id) {
            Some(PlanNode::Limit { input, n }) => {
                shape.limit = Some(shape.limit.map_or(*n, |l| l.min(*n)));
                id = input;
            }
            Some(PlanNode::Sort { input, .. }) | Some(PlanNode::Project { input, .. }) => id = input,
            Some(PlanNode::Aggregate { group_by, .. }) => {
                if group_by.is_empty() {
                    shape.ungrouped = true;
                } else {
                    shape.group_by = Some(group_by[0].clone());
                }
                return shape;
            }
            _ => return shape,
        }
    }
}

/// Estimate the result rows of `plan` given its scans (see module docs).
pub fn estimate(plan: &Plan, scans: &[SourceScan], cost: &CostModel, cal: &Calibration) -> RowEstimate {
    let source = scans.first().map(|s| s.uri.rsplit(['/', '\\']).next().unwrap_or(&s.uri).trim_end_matches(".parquet").to_string()).unwrap_or_default();
    let total_rows: u64 = scans.iter().map(|s| s.total_rows).sum();
    let raw: f64 = scans.iter().map(|s| s.raw_selected_rows * (1.0 - s.null_fraction.unwrap_or(0.0))).sum();
    let kept_rows: u64 = scans.iter().map(|s| s.prune.kept_rows).sum();
    let bucket = bucket_of(raw, total_rows);
    let corr = cal.selectivity_correction(&source, bucket);
    let selected = (raw * corr.factor).min(kept_rows as f64).max(0.0);
    let mut notes = Vec::new();

    // plan shape
    let shape = plan_shape(plan);
    let (limit, ungrouped) = (shape.limit, shape.ungrouped);
    let groups: Option<(f64, &'static str)> =
        shape.group_by.as_deref().map(|g| cal.group_count(&source, g).unwrap_or((cost.aggregate_result_rows as f64, "prior")));
    let cap = selected;
    let mut result = if ungrouped {
        1.0
    } else if let Some((g, basis)) = groups {
        notes.push(format!("group count {g:.0} from {basis} history"));
        g.min(cap.max(1.0))
    } else {
        cap
    };
    if let Some(l) = limit {
        result = result.min(l as f64);
    }
        let limit_bound = limit.is_some_and(|l| groups.is_none() && !ungrouped && selected >= 2.0 * l as f64);

    // quality: history first, structure second
    let mut quality = match (corr.basis, corr.samples) {
        ("source", n) if n >= 3 => EstimateQuality::Good,
        ("source", _) | ("global", _) => EstimateQuality::Fair,
        _ => EstimateQuality::Poor,
    };
    let coherence = scans.iter().filter_map(|s| s.spatial_coherence).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.min(c))));
    let skew = scans.iter().filter_map(|s| s.density_skew).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.max(c))));
    let covering_quality = if scans.is_empty() || scans.iter().any(|s| !s.has_covering) {
        CoveringQuality::None
    } else if scans.iter().any(|s| s.covering_inferred) {
        CoveringQuality::Inferred
    } else {
        CoveringQuality::Declared
    };
    if covering_quality == CoveringQuality::None {
        quality = quality.min(EstimateQuality::Poor);
        notes.push("no covering statistics: selectivity is a guess".into());
    }
    if coherence.is_some_and(|c| c < 0.3) {
        quality = quality.min(EstimateQuality::Fair);
        notes.push(format!("spatially incoherent file (coherence {:.2}): row-group boxes overlap, estimates are looser", coherence.unwrap()));
    }
    if skew.is_some_and(|s| s > 20.0) {
        quality = quality.min(EstimateQuality::Fair);
        notes.push(format!("skewed density across row groups (max/median {:.0}x)", skew.unwrap()));
    }
    if limit_bound || ungrouped {
        // the answer does not depend on the selectivity estimate: LIMIT-bound or a single row
        quality = EstimateQuality::Good;
        notes.push(if ungrouped { "ungrouped aggregate: exactly one row".into() } else { "result is LIMIT-bound".into() });
    }
    let spread = quality.spread();
    let rows = result.ceil() as u64;
    let low = (result / spread).floor() as u64;
    let mut high = (result * spread).ceil() as u64;
    if let Some(l) = limit {
        high = high.min(l);
    }
    if !ungrouped && groups.is_none() {
        high = high.min(kept_rows.max(rows));
    }
    RowEstimate {
        rows,
        low: low.min(rows),
        high: high.max(rows),
        quality,
        covering_quality,
        raw_selected: raw,
        selected_after_correction: selected,
        correction: corr,
        bucket,
        spatial_coherence: coherence,
        density_skew: skew,
        null_fraction: scans.first().and_then(|s| s.null_fraction),
        row_groups_total: scans.iter().map(|s| s.prune.total_row_groups).sum(),
        row_groups_kept: scans.iter().map(|s| s.prune.kept.len()).sum(),
        limit_bound,
        group_basis: groups.map(|g| g.1),
        notes,
    }
}

/// Multiplicative error factor between an estimate and the truth, symmetric (1 = exact, 2 = off by 2x
/// either way). Both sides are smoothed by +1 so empty results do not blow up.
pub fn error_factor(est: f64, actual: f64) -> f64 {
    let (e, a) = (est + 1.0, actual + 1.0);
    (e / a).max(a / e)
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorSummary {
    pub n: usize,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
    pub over_rate: f64,
    pub under_rate: f64,
    /// Share of cases whose actual value lies inside the estimated [low, high] interval.
    pub interval_coverage: Option<f64>,
}

pub fn summarize_errors(pairs: &[(f64, f64)], intervals: Option<&[(f64, f64)]>) -> Option<ErrorSummary> {
    if pairs.is_empty() {
        return None;
    }
    let mut f: Vec<f64> = pairs.iter().map(|(e, a)| error_factor(*e, *a)).collect();
    f.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q = |p: f64| f[(((f.len() as f64 - 1.0) * p).ceil() as usize).min(f.len() - 1)];
    let over = pairs.iter().filter(|(e, a)| e > a).count() as f64 / pairs.len() as f64;
    let under = pairs.iter().filter(|(e, a)| e < a).count() as f64 / pairs.len() as f64;
    let cover = intervals.map(|iv| {
        let hit = pairs.iter().zip(iv).filter(|((_, a), (lo, hi))| a >= lo && a <= hi).count();
        hit as f64 / pairs.len() as f64
    });
    Some(ErrorSummary { n: pairs.len(), median: q(0.5), p90: q(0.9), max: *f.last().unwrap(), over_rate: over, under_rate: under, interval_coverage: cover })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(source: &str, raw: f64, actual: u64, censored: bool) -> Observation {
        Observation {
            ts_ms: 0, source: source.into(), kind: "features".into(), engine: "duckdb".into(), raw_selected: raw, total_rows: 1_000_000,
            est_rows: raw as u64, actual_rows: actual, limit: None, censored, group_by: None, est_ms: None, actual_ms: 10.0, network: "LOCAL".into(), fallback: false,
        }
    }

    #[test]
    fn error_factor_is_symmetric_and_smoothed() {
        assert_eq!(error_factor(10.0, 10.0), 1.0);
        assert!((error_factor(100.0, 10.0) - error_factor(10.0, 100.0)).abs() < 1e-9);
        assert!(error_factor(0.0, 0.0) == 1.0);
        assert!(error_factor(1000.0, 0.0) > 900.0);
    }

    #[test]
    fn summary_reports_median_p90_max_and_direction() {
        let pairs: Vec<(f64, f64)> = vec![(10.0, 10.0), (20.0, 10.0), (10.0, 40.0), (100.0, 10.0), (10.0, 10.0)];
        let s = summarize_errors(&pairs, None).unwrap();
        assert_eq!(s.n, 5);
        assert!(s.median < 2.5 && s.max > 9.0);
        assert!((s.over_rate - 0.4).abs() < 1e-9 && (s.under_rate - 0.2).abs() < 1e-9);
        let iv = vec![(5.0, 20.0); 5];
        let c = summarize_errors(&pairs, Some(&iv)).unwrap().interval_coverage.unwrap();
        assert!((c - 0.8).abs() < 1e-9, "{c}");
    }

    #[test]
    fn calibration_learns_over_estimation_per_source_then_falls_back_to_global() {
        // the heuristic over-estimates small windows ~20x on source A
        let train: Vec<Observation> = (0..4).map(|i| obs("A", 200.0 + i as f64, 10, false)).collect();
        let cal = Calibration::fit(&train);
        let b = bucket_of(200.0, 1_000_000);
        let ca = cal.selectivity_correction("A", b);
        assert_eq!(ca.basis, "source");
        assert!(ca.factor < 0.15, "learned ~1/20 shrunk toward 1, got {}", ca.factor);
        // a never-seen source in the same bucket uses the global history
        let cb = cal.selectivity_correction("B", b);
        assert_eq!(cb.basis, "global");
        assert!(cb.factor < 0.25);
        // a bucket without any history leaves the estimate untouched
        assert_eq!(cal.selectivity_correction("A", 4).basis, "none");
        assert_eq!(cal.familiarity("A"), 4);
        assert_eq!(cal.familiarity("B"), 0);
    }

    #[test]
    fn censored_observations_do_not_teach_selectivity() {
        let train: Vec<Observation> = (0..5).map(|_| obs("A", 5000.0, 2000, true)).collect();
        let cal = Calibration::fit(&train);
        assert_eq!(cal.selectivity_correction("A", bucket_of(5000.0, 1_000_000)).basis, "none");
    }

    #[test]
    fn group_counts_are_learned() {
        let mut o = obs("A", 100.0, 8, false);
        o.group_by = Some("category".into());
        let cal = Calibration::fit(&vec![o.clone(), o.clone(), o]);
        assert_eq!(cal.group_count("A", "category"), Some((8.0, "source")));
        assert_eq!(cal.group_count("Z", "x").map(|g| g.1), Some("global"));
    }
}
