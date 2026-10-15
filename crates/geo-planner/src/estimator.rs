//! Estimator v3 (ADR-010): cardinality and output-size estimation with isolated, evidence-based calibration.
//!
//! Inputs are metadata only (footer statistics via [`SourceScan`]) plus a [`Calibration`] learned from
//! *previous* executions' telemetry. The estimator never sees the outcome of the query it estimates.
//!
//! ```text
//! raw         = sum over kept row groups of rows x (window ∩ row-group bbox)/area(row-group bbox)   (uniform density)
//!               x (1 - geometry null fraction)
//! cardinality = min( raw x correction(layout class, selectivity bucket), rows of the kept row groups )   -- how many rows match
//! output      = shape(cardinality): aggregate -> group count, LIMIT -> min(limit, .)                     -- how many rows come back
//! interval    = cardinality / spread .. cardinality x spread, spread = P90 residual of the calibration key
//! ```
//!
//! v3 separates what v2 conflated:
//! * **cardinality ≠ output cap.** A LIMIT bounds the output, not the number of matching rows. The output is
//!   only *certain* when the lower bound of the cardinality already reaches the limit (`limit_certain`).
//! * **Calibration is isolated by context.** A correction learned on one *layout class* (spatial coherence of the
//!   row groups × density skew × covering quality) is never applied to another class. Lookup order: this source
//!   (≥ 2 runs) → this layout class (≥ 3 runs) → no correction at all (factor 1, spread 12, quality Poor).
//!
//! Censored runs (the result hit its LIMIT) teach nothing about selectivity.

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
    pub fn score(self) -> f64 {
        match self {
            EstimateQuality::Good => 1.0,
            EstimateQuality::Fair => 0.7,
            EstimateQuality::Poor => 0.4,
        }
    }
}

/// Spread (multiplicative half-width of the interval) assumed when there is no calibration at all.
pub const PRIOR_SPREAD: f64 = 12.0;

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
    /// Layout class of the source when the run was planned (see [`layout_class`]); empty in old telemetry.
    #[serde(default)]
    pub layout: String,
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

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[(((v.len() - 1) as f64) * p).ceil() as usize]
}

/// The context a correction is valid in: how well the file's row groups are ordered in space, how uneven the
/// density is, and whether the bbox covering is declared. Corrections never cross classes.
pub fn layout_class(scans: &[SourceScan], covering: CoveringQuality, filtered: bool) -> String {
    let coh = scans.iter().filter_map(|s| s.spatial_coherence).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.min(c))));
    let skew = scans.iter().filter_map(|s| s.density_skew).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.max(c))));
    let coh = match coh {
        None => "unknown",
        Some(c) if c >= 0.8 => "high",
        Some(c) if c >= 0.4 => "mid",
        Some(_) => "low",
    };
    let skew = match skew {
        None => "unknown",
        Some(s) if s < 4.0 => "flat",
        Some(s) if s < 20.0 => "mid",
        Some(_) => "peaked",
    };
    let cov = match covering {
        CoveringQuality::Declared => "declared",
        CoveringQuality::Inferred => "inferred",
        CoveringQuality::None => "none",
    };
    let hist = if !scans.is_empty() && scans.iter().all(|s| s.has_histogram) { "yes" } else { "no" };
    format!("coh={coh}|skew={skew}|cov={cov}|hist={hist}|filt={}", if filtered { "yes" } else { "no" })
}

/// Learned corrections. Built from observations with [`Calibration::fit`]; cheap to clone.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Calibration {
    /// ln(actual+1 / raw+1) per (source, bucket).
    by_source: BTreeMap<(String, u8), Vec<f64>>,
    /// ... per (layout class, bucket): the only way knowledge crosses sources.
    by_class: BTreeMap<(String, u8), Vec<f64>>,
    /// Observed group counts per (source, column).
    groups: BTreeMap<(String, String), Vec<f64>>,
    familiarity: BTreeMap<String, usize>,
    pub observations: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Correction {
    pub factor: f64,
    pub samples: usize,
    /// `source` | `class` | `none`
    pub basis: &'static str,
    /// Half-width of the interval (multiplicative): P90 of the residuals around the median.
    pub spread: f64,
}

impl Correction {
    pub fn none() -> Correction {
        Correction { factor: 1.0, samples: 0, basis: "none", spread: PRIOR_SPREAD }
    }
}

impl Calibration {
    pub fn fit(obs: &[Observation]) -> Calibration {
        let mut c = Calibration::default();
        for o in obs {
            c.observations += 1;
            *c.familiarity.entry(o.source.clone()).or_default() += 1;
            // `kind == "count"` rows are the single row of an ungrouped aggregate, not a cardinality; offline calibration
            // records the matching-row count as kind "cardinality"
            if !o.censored && o.group_by.is_none() && o.kind != "count" && o.raw_selected >= 0.0 {
                let r = ((o.actual_rows as f64 + 1.0) / (o.raw_selected + 1.0)).ln();
                let b = bucket_of(o.raw_selected, o.total_rows);
                c.by_source.entry((o.source.clone(), b)).or_default().push(r);
                if !o.layout.is_empty() {
                    c.by_class.entry((o.layout.clone(), b)).or_default().push(r);
                }
            }
            if let Some(g) = &o.group_by {
                // grouped result below its LIMIT = the true group count
                if !o.censored {
                    c.groups.entry((o.source.clone(), g.clone())).or_default().push(o.actual_rows as f64);
                }
            }
        }
        c
    }

    pub fn familiarity(&self, source: &str) -> usize {
        self.familiarity.get(source).copied().unwrap_or(0)
    }

    fn summarise(v: &[f64], prior_n: f64, basis: &'static str) -> Correction {
        let n = v.len() as f64;
        let med = median(&mut v.to_vec());
        let mut dev: Vec<f64> = v.iter().map(|x| (x - med).abs()).collect();
        let spread_ln = if v.len() >= 3 { percentile(&mut dev, 0.9) } else { dev.iter().cloned().fold(0.0, f64::max) * 1.5 };
        Correction {
            factor: (med * n / (n + prior_n)).exp().clamp(1.0 / 200.0, 200.0),
            samples: v.len(),
            basis,
            spread: spread_ln.exp().clamp(1.5, PRIOR_SPREAD),
        }
    }

    /// Correction for a source in a layout class: this source's own runs, else the class's, else none.
    pub fn selectivity_correction(&self, source: &str, layout: &str, bucket: u8) -> Correction {
        if let Some(v) = self.by_source.get(&(source.to_string(), bucket)).filter(|v| v.len() >= 2) {
            return Self::summarise(v, 1.0, "source");
        }
        if let Some(v) = self.by_class.get(&(layout.to_string(), bucket)).filter(|v| v.len() >= 3) {
            return Self::summarise(v, 2.0, "class");
        }
        Correction::none()
    }

    /// Cardinality of a GROUP BY column: the largest group count ever observed for it on this source (a window can
    /// only show a subset of the groups, so the maximum is the best lower bound of the cardinality); without own
    /// history, the median of the per-source maxima of a column **with the same name** on other sources. Never another
    /// column's history (a v3 held-out run showed the cost: `height` has hundreds of groups, `category` eight).
    /// The result has at most `min(cardinality, rows)` groups.
    pub fn group_count(&self, source: &str, column: &str) -> Option<(f64, &'static str)> {
        let max = |v: &Vec<f64>| v.iter().cloned().fold(0.0f64, f64::max);
        if let Some(v) = self.groups.get(&(source.to_string(), column.to_string())).filter(|v| !v.is_empty()) {
            return Some((max(v), "source"));
        }
        let mut maxima: Vec<f64> = self.groups.iter().filter(|((_, c), v)| c == column && !v.is_empty()).map(|(_, v)| max(v)).collect();
        (maxima.len() >= 2).then(|| (median(&mut maxima), "global"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RowEstimate {
    /// Expected rows the query returns (after aggregate / LIMIT).
    pub rows: u64,
    /// Interval of the *output* rows; `high` is what safety decisions use.
    pub low: u64,
    pub high: u64,
    /// Quality of the output estimate.
    pub quality: EstimateQuality,
    /// Expected rows that MATCH (before aggregate / LIMIT) and their interval.
    pub cardinality: u64,
    pub cardinality_low: u64,
    pub cardinality_high: u64,
    pub cardinality_quality: EstimateQuality,
    /// The LIMIT is smaller than the expected cardinality: the output is capped, the cardinality is not known to be that small.
    pub bounded_by_limit: bool,
    /// The lower bound of the cardinality already reaches the LIMIT: the output is certain to be the limit.
    pub limit_certain: bool,
    pub covering_quality: CoveringQuality,
    pub layout: String,
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

fn quality_of(c: &Correction) -> EstimateQuality {
    match (c.basis, c.samples) {
        ("source", n) if n >= 3 && c.spread <= 3.0 => EstimateQuality::Good,
        ("class", n) if n >= 6 && c.spread <= 3.0 => EstimateQuality::Good,
        (_, _) if c.basis != "none" && c.spread <= 8.0 => EstimateQuality::Fair,
        _ => EstimateQuality::Poor,
    }
}

/// Estimate the result rows of `plan` given its scans (see module docs).
pub fn estimate(plan: &Plan, scans: &[SourceScan], cost: &CostModel, cal: &Calibration, hists: Option<&geo_source_geoparquet::HistogramStore>) -> RowEstimate {
    let source = scans.first().map(|s| s.uri.rsplit(['/', '\\']).next().unwrap_or(&s.uri).trim_end_matches(".parquet").to_string()).unwrap_or_default();
    let total_rows: u64 = scans.iter().map(|s| s.total_rows).sum();
    let mut raw: f64 = scans.iter().map(|s| s.raw_selected_rows * (1.0 - s.null_fraction.unwrap_or(0.0))).sum();
    // attribute predicates (Filter nodes): selectivity from the column statistics of the source's histogram sidecar
    let filters: Vec<&geo_ir::Expr> = plan.nodes.values().filter_map(|n| if let PlanNode::Filter { predicate, .. } = n { Some(predicate) } else { None }).collect();
    let mut attr_note: Option<String> = None;
    let mut attr_known = true;
    if !filters.is_empty() && scans.len() == 1 {
        let hist = hists.and_then(|h| h.get(std::path::Path::new(&scans[0].source_path)));
        let empty = std::collections::BTreeMap::new();
        let cols = hist.as_ref().map_or(&empty, |h| &h.columns);
        let sel: Vec<geo_source_geoparquet::Selectivity> = filters.iter().map(|e| geo_source_geoparquet::predicate_selectivity(e, cols)).collect();
        let v: f64 = sel.iter().map(|s| s.value).product();
        attr_known = sel.iter().all(|s| s.known);
        raw *= v;
        attr_note = Some(format!("attribute filter keeps ~{:.1}% of the rows ({})", v * 100.0, if attr_known { "from column statistics" } else { "ASSUMED: no statistics for it" }));
    } else if !filters.is_empty() {
        attr_known = false;
        attr_note = Some("attribute filters over several sources are not modelled".into());
    }
    let kept_rows: u64 = scans.iter().map(|s| s.prune.kept_rows).sum();
    let bucket = bucket_of(raw, total_rows);
    let covering_quality = if scans.is_empty() || scans.iter().any(|s| !s.has_covering) {
        CoveringQuality::None
    } else if scans.iter().any(|s| s.covering_inferred) {
        CoveringQuality::Inferred
    } else {
        CoveringQuality::Declared
    };
    let layout = layout_class(scans, covering_quality, !filters.is_empty());
    let corr = cal.selectivity_correction(&source, &layout, bucket);
    let mut notes = Vec::new();
    let kept = kept_rows as f64;
    let card = (raw * corr.factor).min(kept).max(0.0);
    let card_low = (card / corr.spread).min(card);
    let card_high = (card * corr.spread).min(kept).max(card);
    let mut card_quality = quality_of(&corr);
    let coherence = scans.iter().filter_map(|s| s.spatial_coherence).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.min(c))));
    let skew = scans.iter().filter_map(|s| s.density_skew).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.max(c))));
    if let Some(n) = attr_note {
        notes.push(n);
    }
    if !attr_known {
        card_quality = card_quality.min(EstimateQuality::Fair);
    }
    if covering_quality == CoveringQuality::None {
        card_quality = EstimateQuality::Poor;
        notes.push("no covering statistics: selectivity is a guess".into());
    }
    if coherence.is_some_and(|c| c < 0.3) {
        card_quality = card_quality.min(EstimateQuality::Fair);
        notes.push(format!("spatially incoherent file (coherence {:.2}): row-group boxes overlap, estimates are looser", coherence.unwrap()));
    }
    if skew.is_some_and(|s| s > 20.0) {
        card_quality = card_quality.min(EstimateQuality::Fair);
        notes.push(format!("skewed density across row groups (max/median {:.0}x)", skew.unwrap()));
    }
    notes.push(match corr.basis {
        "none" => format!("no calibration for layout class '{layout}': uncorrected estimate, interval ×/÷{:.0}", corr.spread),
        b => format!("correction {:.2} from {} runs of this {b} (interval ×/÷{:.1})", corr.factor, corr.samples, corr.spread),
    });

    // plan shape: output rows
    let shape = plan_shape(plan);
    // group count: exact frequencies of the column when the sidecar has them (expected distinct values among the matching
    // rows), else what past runs of this source/column showed, else the prior
    let groups: Option<(f64, &'static str)> = shape.group_by.as_deref().map(|g| {
        let hist = hists.and_then(|h| scans.first().and_then(|s| h.get(std::path::Path::new(&s.source_path))));
        match hist.as_ref().and_then(|h| h.columns.get(g)).and_then(|c| c.expected_distinct(card)) {
            Some(d) => (d.max(1.0), "statistics"),
            None => cal.group_count(&source, g).unwrap_or((cost.aggregate_result_rows as f64, "prior")),
        }
    });
    let (mut out, mut out_low, mut out_high, mut out_quality) = if shape.ungrouped {
        notes.push("ungrouped aggregate: exactly one row".into());
        (1.0, 1.0, 1.0, EstimateQuality::Good)
    } else if let Some((g, basis)) = groups {
        notes.push(format!("group count ≤ {g:.0} from {basis} history"));
        let e = g.min(card.max(1.0));
        let gq = if basis == "source" || basis == "statistics" { card_quality } else { card_quality.min(EstimateQuality::Fair) };
        (e, g.min(card_low.max(1.0)).min(e), g.min(card_high.max(1.0)).max(e), gq)
    } else {
        (card, card_low, card_high, card_quality)
    };
    let mut bounded = false;
    let mut certain = false;
    if let Some(l) = shape.limit {
        let lf = l as f64;
        bounded = out > lf;
        // certain only when even the pessimistic cardinality reaches the limit
        certain = groups.is_none() && !shape.ungrouped && out_low >= lf;
        out = out.min(lf);
        out_low = out_low.min(lf);
        out_high = out_high.min(lf);
        if certain {
            out_quality = EstimateQuality::Good;
            notes.push("result is certain to hit the LIMIT (lower bound of the cardinality ≥ limit)".into());
        } else if bounded {
            notes.push("result is CAPPED by the LIMIT, but how many rows match is not certain".into());
        }
    }
    RowEstimate {
        rows: out.ceil() as u64,
        low: out_low.floor() as u64,
        high: out_high.ceil() as u64,
        quality: out_quality,
        cardinality: card.ceil() as u64,
        cardinality_low: card_low.floor() as u64,
        cardinality_high: card_high.ceil() as u64,
        cardinality_quality: card_quality,
        bounded_by_limit: bounded,
        limit_certain: certain,
        covering_quality,
        layout,
        raw_selected: raw,
        selected_after_correction: card,
        correction: corr,
        bucket,
        spatial_coherence: coherence,
        density_skew: skew,
        null_fraction: scans.first().and_then(|s| s.null_fraction),
        row_groups_total: scans.iter().map(|s| s.prune.total_row_groups).sum(),
        row_groups_kept: scans.iter().map(|s| s.prune.kept.len()).sum(),
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
    pub p95: f64,
    pub max: f64,
    pub over_rate: f64,
    pub under_rate: f64,
    /// Share of cases where `(actual+1)/(est+1) > 5`: the estimate was dangerously low.
    pub unsafe_under_rate: f64,
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
    let n = pairs.len() as f64;
    let over = pairs.iter().filter(|(e, a)| e > a).count() as f64 / n;
    let under = pairs.iter().filter(|(e, a)| e < a).count() as f64 / n;
    let unsafe_under = pairs.iter().filter(|(e, a)| (a + 1.0) / (e + 1.0) > 5.0).count() as f64 / n;
    let cover = intervals.map(|iv| pairs.iter().zip(iv).filter(|((_, a), (lo, hi))| a >= lo && a <= hi).count() as f64 / n);
    Some(ErrorSummary { n: pairs.len(), median: q(0.5), p90: q(0.9), p95: q(0.95), max: *f.last().unwrap(), over_rate: over, under_rate: under, unsafe_under_rate: unsafe_under, interval_coverage: cover })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(source: &str, layout: &str, raw: f64, actual: u64, censored: bool) -> Observation {
        Observation {
            ts_ms: 0, source: source.into(), kind: "features".into(), engine: "duckdb".into(), raw_selected: raw, total_rows: 1_000_000,
            est_rows: raw as u64, actual_rows: actual, limit: None, censored, group_by: None, est_ms: None, actual_ms: 10.0, network: "LOCAL".into(),
            fallback: false, layout: layout.into(),
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
    fn summary_reports_median_p90_p95_max_direction_and_unsafe_underestimates() {
        let pairs: Vec<(f64, f64)> = vec![(10.0, 10.0), (20.0, 10.0), (10.0, 400.0), (100.0, 10.0), (10.0, 10.0)];
        let s = summarize_errors(&pairs, None).unwrap();
        assert_eq!(s.n, 5);
        assert!(s.median < 2.5 && s.max > 9.0 && s.p95 >= s.p90);
        assert!((s.over_rate - 0.4).abs() < 1e-9 && (s.under_rate - 0.2).abs() < 1e-9);
        assert!((s.unsafe_under_rate - 0.2).abs() < 1e-9, "one run came back 36x larger than estimated");
        let iv = vec![(5.0, 20.0); 5];
        let c = summarize_errors(&pairs, Some(&iv)).unwrap().interval_coverage.unwrap();
        assert!((c - 0.8).abs() < 1e-9, "{c}");
    }

    #[test]
    fn calibration_never_crosses_layout_classes() {
        let a = "coh=high|skew=peaked|cov=declared|hist=no";
        let b = "coh=high|skew=flat|cov=declared|hist=no";
        // the heuristic over-estimates 20x on clustered files (class a)
        let train: Vec<Observation> = (0..6).map(|i| obs(&format!("S{i}"), a, 200.0 + i as f64, 10, false)).collect();
        let cal = Calibration::fit(&train);
        let bk = bucket_of(200.0, 1_000_000);
        let ca = cal.selectivity_correction("NEW", a, bk);
        assert_eq!(ca.basis, "class");
        assert!(ca.factor < 0.2, "{}", ca.factor);
        // a uniform (flat) file must NOT inherit it
        let cb = cal.selectivity_correction("NEW", b, bk);
        assert_eq!((cb.basis, cb.factor), ("none", 1.0));
        assert_eq!(cb.spread, PRIOR_SPREAD);
        // a source with its own runs uses them
        let own: Vec<Observation> = (0..3).map(|_| obs("OWN", b, 200.0, 190, false)).collect();
        let cal2 = Calibration::fit(&[train, own].concat());
        let co = cal2.selectivity_correction("OWN", b, bk);
        assert_eq!(co.basis, "source");
        assert!(co.factor > 0.7 && co.spread < 3.0);
        assert_eq!(cal2.familiarity("OWN"), 3);
    }

    #[test]
    fn too_few_class_samples_means_no_correction() {
        let a = "coh=high|skew=peaked|cov=declared|hist=no";
        let cal = Calibration::fit(&[obs("X", a, 200.0, 10, false), obs("Y", a, 210.0, 10, false)]);
        assert_eq!(cal.selectivity_correction("Z", a, bucket_of(200.0, 1_000_000)).basis, "none");
    }

    #[test]
    fn spread_follows_the_residuals_and_drives_quality() {
        let a = "coh=high|skew=peaked|cov=declared|hist=no";
        let tight: Vec<Observation> = (0..8).map(|i| obs(&format!("S{i}"), a, 200.0, 10 + (i % 2), false)).collect();
        let c = Calibration::fit(&tight).selectivity_correction("N", a, bucket_of(200.0, 1_000_000));
        assert!(c.spread < 1.6, "{}", c.spread);
        assert_eq!(quality_of(&c), EstimateQuality::Good);
        let wild: Vec<Observation> = (0..8).map(|i| obs(&format!("S{i}"), a, 200.0, [2, 400, 5, 900, 3, 200, 1, 700][i], false)).collect();
        let w = Calibration::fit(&wild).selectivity_correction("N", a, bucket_of(200.0, 1_000_000));
        assert!(w.spread > 5.0, "{}", w.spread);
        assert_ne!(quality_of(&w), EstimateQuality::Good);
    }

    #[test]
    fn censored_observations_do_not_teach_selectivity() {
        let a = "coh=high|skew=peaked|cov=declared|hist=no";
        let train: Vec<Observation> = (0..5).map(|_| obs("A", a, 5000.0, 2000, true)).collect();
        let cal = Calibration::fit(&train);
        assert_eq!(cal.selectivity_correction("A", a, bucket_of(5000.0, 1_000_000)).basis, "none");
    }

    #[test]
    fn group_counts_are_learned() {
        let mut o = obs("A", "x", 100.0, 8, false);
        o.group_by = Some("category".into());
        let cal = Calibration::fit(&[o.clone(), o.clone(), o.clone()]);
        assert_eq!(cal.group_count("A", "category"), Some((8.0, "source")));
        assert_eq!(cal.group_count("Z", "category"), None, "one other source is not enough to generalise");
        let mut o2 = obs("B", "x", 100.0, 8, false);
        o2.group_by = Some("category".into());
        let cal2 = Calibration::fit(&[o.clone(), o2]);
        assert_eq!(cal2.group_count("Z", "category").map(|g| g.1), Some("global"));
        assert_eq!(cal2.group_count("Z", "height"), None, "another column's history never answers for this one");
    }

    fn test_scan() -> SourceScan {
        use geo_source_geoparquet::{PruneMethod, PruneReport, ReadClass};
        SourceScan {
            node: "n".into(), uri: "x.parquet".into(), crs: geo_types::Crs::Epsg(4326), total_rows: 1000, total_bytes: 1000, constraints_applied: 1, constraints_skipped: vec![],
            source_path: String::new(), has_histogram: false, uniform_selected_rows: 10.0, access_paths: 1, has_covering: true, covering_inferred: false, covering: None, covering_bytes_total: 0, selected_rows_estimate: 10, raw_selected_rows: 10.0,
            read_class: ReadClass::PlainWkb, null_fraction: Some(0.0), spatial_coherence: None, density_skew: None,
            prune: PruneReport { method: PruneMethod::CoveringStats, total_row_groups: 10, kept: vec![0], skipped: 9, total_rows: 1000, kept_rows: 100, total_bytes: 1000, kept_bytes: 100 },
        }
    }

    #[test]
    fn layout_class_separates_clustered_uniform_and_shuffled() {
        let sc = |coh: f64, skew: f64| {
            let mut s = test_scan();
            s.spatial_coherence = Some(coh);
            s.density_skew = Some(skew);
            s
        };
        let c = CoveringQuality::Declared;
        assert_eq!(layout_class(&[sc(0.99, 60.0)], c, false), "coh=high|skew=peaked|cov=declared|hist=no|filt=no");
        assert_eq!(layout_class(&[sc(0.95, 1.5)], c, false), "coh=high|skew=flat|cov=declared|hist=no|filt=no");
        assert_eq!(layout_class(&[sc(0.05, 1.2)], c, true), "coh=low|skew=flat|cov=declared|hist=no|filt=yes");
        assert_eq!(layout_class(&[], CoveringQuality::None, false), "coh=unknown|skew=unknown|cov=none|hist=no|filt=no");
    }

    fn plan_with_limit(limit: Option<u64>) -> Plan {
        use geo_ir::Node;
        let src = Node::Source {
            uri: "x.parquet".into(), format: geo_ir::SourceFormat::GeoParquet,
            schema: geo_types::SpatialSchema { fields: vec![], geometry_columns: vec![] }, statistics: Default::default(),
        };
        match limit {
            Some(n) => Plan::new(Node::Limit { input: Box::new(src), n }),
            None => Plan::new(src),
        }
    }

    #[test]
    fn a_limit_caps_the_output_but_not_the_cardinality() {
        let mut s = test_scan();
        s.raw_selected_rows = 80.0;
        s.prune.kept_rows = 100_000;
        s.total_rows = 1_000_000;
        // no calibration: interval ×/÷12 -> cardinality 80 (6..960): a LIMIT of 50 caps the output but cannot be certain
        let e = estimate(&plan_with_limit(Some(50)), &[s.clone()], &CostModel::default(), &Calibration::default(), None);
        assert_eq!((e.rows, e.cardinality), (50, 80));
        assert!(e.bounded_by_limit && !e.limit_certain);
        assert_ne!(e.quality, EstimateQuality::Good, "v2 called this 'good': a capped estimate is not a certain one");
        assert!(e.low < 50 && e.high == 50);
        // a cardinality whose LOWER bound reaches the limit is certain
        s.raw_selected_rows = 90_000.0;
        let e = estimate(&plan_with_limit(Some(50)), &[s], &CostModel::default(), &Calibration::default(), None);
        assert!(e.limit_certain && e.quality == EstimateQuality::Good && e.low == 50 && e.high == 50);
    }
}
