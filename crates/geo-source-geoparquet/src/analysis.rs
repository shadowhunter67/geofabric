//! Plan-level scan analysis: which row groups can a plan's spatial predicates
//! skip? Shared by the planner (explanations) and engine adapters (metrics).
//!
//! Works on the PlanGraph. Predicates only constrain a source when every
//! operator between the predicate and the source is row-preserving (Filter,
//! BboxFilter, Intersects, Sort, Project). Limit / Aggregate / Buffer /
//! SpatialJoin reset the constraint set (pushing a filter below them would
//! change results).
//!
//! A source node that is **shared** (several consumers) is read once but may be
//! needed by several access paths: the row groups to read are the **union** over
//! all paths of (the intersection of that path's constraints); a path without
//! applicable constraints needs everything.

use crate::{Covering, GeoParquetSource, PruneMethod, PruneReport, SourceError};
use geo_ir::{NodeId, Plan, PlanNode};
use geo_types::{BoundingBox, Crs, SourceAccessGuard};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, Serialize)]
pub struct SourceScan {
    /// Authorized file path (the key of the histogram store).
    pub source_path: String,
    /// A spatial histogram of the rows was available: `raw_selected_rows` comes from it, not from the uniform-density heuristic.
    pub has_histogram: bool,
    /// The uniform-density-per-row-group estimate (always computed; the histogram replaces it when present).
    pub uniform_selected_rows: f64,
    pub node: String,
    pub uri: String,
    pub crs: Crs,
    pub total_rows: u64,
    pub total_bytes: u64,
    pub constraints_applied: usize,
    pub constraints_skipped: Vec<String>,
    /// Number of distinct access paths reaching this source (>1 = shared source).
    pub access_paths: usize,
    /// The source has a bbox covering column (needed by engines without a spatial extension).
    pub has_covering: bool,
    /// The covering was inferred by naming convention, not declared in the file metadata.
    pub covering_inferred: bool,
    /// Column paths of the covering (for engines that prune through it, e.g. the browser engine).
    pub covering: Option<Covering>,
    /// Compressed bytes of the covering columns over **all** row groups. Engines that cannot prune
    /// through struct-field statistics (measured: DuckDB-Wasm) have to read all of it.
    pub covering_bytes_total: u64,
    /// Heuristic row estimate after the spatial constraints: per kept row group, rows x overlap
    /// fraction of the query window with the row group's bbox (uniform-density assumption).
    /// An estimate (`SourceEstimated`), not a bound: dense clusters make it optimistic, sparse ones pessimistic.
    pub selected_rows_estimate: u64,
    /// Uniform-density estimate **before** the null-fraction adjustment is applied, kept for calibration.
    pub raw_selected_rows: f64,
    /// Reader-compatibility class of the file (see [`crate::ReadClass`]).
    pub read_class: crate::ReadClass,
    /// Fraction of null geometries (column statistics), if known.
    pub null_fraction: Option<f64>,
    /// 1 = row groups tile space without overlap (spatially sorted file); 0 = every row group spans the
    /// whole extent (shuffled file: pruning cannot help). From the row-group bboxes; `None` without bboxes.
    pub spatial_coherence: Option<f64>,
    /// Row-group-level density spread: max/median of rows per unit bbox area (>1 = skewed, estimates less reliable).
    pub density_skew: Option<f64>,
    /// Uncompressed bytes per row of the geometry column (WKB size: 21 for a point, tens of KB for an administrative boundary).
    pub geometry_row_bytes: f64,
    /// Exact (row index of a small source): geometry bytes of the rows in the window; `None` when only the mean is known.
    pub selected_geometry_bytes: Option<f64>,
    /// Uncompressed bytes per row of the other columns a feature result carries (not geometry, not the covering struct).
    pub attribute_row_bytes: f64,
    pub prune: PruneReport,
}

#[derive(Clone)]
struct Constraint {
    column: String,
    bbox: BoundingBox,
}

/// Best-effort envelope of a 2D WKT literal (all coordinate pairs).
pub fn wkt_envelope(wkt: &str, crs: &Crs) -> Option<BoundingBox> {
    let up = wkt.to_ascii_uppercase();
    if up.contains("EMPTY") || up.contains(" Z") || up.contains("ZM") || up.contains(" M") {
        return None;
    }
    let body = &wkt[wkt.find('(')?..];
    let nums: Vec<f64> = body
        .split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    if nums.is_empty() || nums.len() % 2 != 0 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in nums.chunks(2) {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    BoundingBox::new(x0, y0, x1, y1, crs.clone()).ok()
}

/// Scan analysis reads source metadata, so every source is authorized by
/// `guard` **before** it is opened.
pub fn analyze_plan(plan: &Plan, guard: &dyn SourceAccessGuard) -> Result<Vec<SourceScan>, SourceError> {
    analyze_plan_with(plan, guard, None)
}

/// Like [`analyze_plan`], using spatial histograms (when the store has one for a source) for the selectivity estimate.
pub fn analyze_plan_with(plan: &Plan, guard: &dyn SourceAccessGuard, histograms: Option<&crate::HistogramStore>) -> Result<Vec<SourceScan>, SourceError> {
    // 1. collect, per source node, the constraint sets of every access path
    let mut paths: BTreeMap<NodeId, Vec<Vec<Constraint>>> = BTreeMap::new();
    let mut seen: HashSet<(NodeId, String)> = HashSet::new();
    collect(plan, &plan.root, &[], &mut paths, &mut seen);

    // 2. analyse each source once, in plan order
    let order = plan.topological_order().map_err(|_| SourceError::NotGeoParquet("malformed plan graph".into()))?;
    let mut out = Vec::new();
    for id in order {
        let Some(path_sets) = paths.get(&id) else { continue };
        let PlanNode::Source { uri, .. } = &plan.nodes[&id] else { continue };
        let authorized = guard.authorize(uri).map_err(|_| SourceError::AccessDenied)?;
        let src = GeoParquetSource::open_authorized(&authorized)?;
        let info = src.info();
        let all: BTreeSet<usize> = info.row_groups.iter().map(|r| r.index).collect();

        let mut kept: BTreeSet<usize> = BTreeSet::new();
        let mut applied = 0;
        let mut skipped = Vec::new();
        for set in path_sets {
            let mut path_kept: Option<BTreeSet<usize>> = None;
            for c in set {
                if c.column != info.geometry_column {
                    skipped.push(format!("column '{}' is not the source geometry column", c.column));
                    continue;
                }
                match src.prune(&c.bbox) {
                    Ok(r) => {
                        applied += 1;
                        let s: BTreeSet<usize> = r.kept.into_iter().collect();
                        path_kept = Some(match path_kept {
                            None => s,
                            Some(k) => k.intersection(&s).copied().collect(),
                        });
                    }
                    Err(e) => skipped.push(e.to_string()),
                }
            }
            // a path without applicable constraints needs every row group
            kept.extend(path_kept.unwrap_or_else(|| all.clone()));
        }
        if path_sets.len() > 1 {
            skipped.push(format!("shared source: union of {} access paths", path_sets.len()));
        }
        // selectivity estimate: for each kept row group the best (max) path fraction; a path's fraction is
        // the min over its constraints of overlap_area / row-group area
        let mut selected_rows = 0.0f64;
        for g in &kept {
            let rg = &info.row_groups[*g];
            let mut best = 0.0f64;
            for set in path_sets {
                let mut frac = 1.0f64;
                for c in set {
                    if c.column != info.geometry_column {
                        continue;
                    }
                    if let Some(b) = rg.bbox {
                        let ow = (b[2].min(c.bbox.max_x()) - b[0].max(c.bbox.min_x())).max(0.0);
                        let oh = (b[3].min(c.bbox.max_y()) - b[1].max(c.bbox.min_y())).max(0.0);
                        let area = (b[2] - b[0]) * (b[3] - b[1]);
                        let f = if area > 0.0 { (ow * oh / area).min(1.0) } else { 1.0 };
                        frac = frac.min(f);
                    }
                }
                best = best.max(frac);
            }
            selected_rows += rg.rows as f64 * best;
        }
        let uniform_selected = selected_rows;
        let mut has_histogram = false;
        let mut selected_geometry_bytes: Option<f64> = None;
        if let Some(h) = histograms.and_then(|s| s.get(src.path())).filter(|h| h.matches(&src)) {
            // the histogram knows where the rows really are: per access path, the window is the intersection of its constraints
            let mut best = 0.0f64;
            for set in path_sets {
                let mut w: Option<[f64; 4]> = None;
                for c in set.iter().filter(|c| c.column == info.geometry_column) {
                    let b = [c.bbox.min_x(), c.bbox.min_y(), c.bbox.max_x(), c.bbox.max_y()];
                    w = Some(match w {
                        None => b,
                        Some(a) => [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])],
                    });
                }
                let est = match w {
                    Some(w) if w[0] > w[2] || w[1] > w[3] => 0.0,
                    Some(w) => h.estimate(w),
                    None => h.counted as f64,
                };
                best = best.max(est);
            }
            selected_rows = best;
            has_histogram = true;
            // a small source: exact counts and exact geometry payload per access path (rows whose bbox intersects the window)
            if let Some(ix) = &h.row_index {
                let (mut n_best, mut b_best) = (0u64, 0.0f64);
                for set in path_sets {
                    let mut w: Option<[f64; 4]> = None;
                    for c in set.iter().filter(|c| c.column == info.geometry_column) {
                        let b = [c.bbox.min_x(), c.bbox.min_y(), c.bbox.max_x(), c.bbox.max_y()];
                        w = Some(match w {
                            None => b,
                            Some(a) => [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])],
                        });
                    }
                    let (n, b) = match w {
                        Some(w) if w[0] > w[2] || w[1] > w[3] => (0, 0.0),
                        Some(w) => ix.window(w),
                        None => (ix.xmin.len() as u64, ix.geometry_bytes.iter().map(|x| *x as f64).sum()),
                    };
                    if n >= n_best {
                        n_best = n;
                        b_best = b;
                    }
                }
                selected_rows = n_best as f64;
                selected_geometry_bytes = Some(b_best);
            }
        }
        let kept: Vec<usize> = kept.into_iter().collect();
        let total_bytes: u64 = info.row_groups.iter().map(|r| r.compressed_bytes).sum();
        let kept_rows: u64 = kept.iter().map(|i| info.row_groups[*i].rows).sum();
        let kept_bytes = src.estimate_bytes(None, &kept);
        let has_stats = info.row_groups.iter().any(|r| r.bbox.is_some());
        out.push(SourceScan {
            source_path: src.path().to_string_lossy().into_owned(),
            has_histogram,
            uniform_selected_rows: uniform_selected,
            node: id.0.clone(),
            uri: uri.clone(),
            crs: info.crs.clone(),
            total_rows: info.row_count,
            total_bytes,
            constraints_applied: applied,
            constraints_skipped: skipped,
            access_paths: path_sets.len(),
            has_covering: info.covering.is_some(),
            covering_inferred: info.covering.as_ref().is_some_and(|c| !c.declared),
            covering: info.covering.clone(),
            selected_rows_estimate: (selected_rows * (1.0 - info.geometry_null_fraction.unwrap_or(0.0))).ceil() as u64,
            raw_selected_rows: selected_rows,
            read_class: info.read_class,
            null_fraction: info.geometry_null_fraction,
            spatial_coherence: coherence(info),
            density_skew: density_skew(info),
            geometry_row_bytes: uncompressed_row_bytes(info, |c| c == info.geometry_column),
            selected_geometry_bytes,
            attribute_row_bytes: uncompressed_row_bytes(info, |c| {
                c != info.geometry_column && info.covering.as_ref().map_or(true, |cv| c != cv.xmin.split('.').next().unwrap_or(""))
            }),
            covering_bytes_total: info.covering.as_ref().map_or(0, |c| {
                let top = c.xmin.split('.').next().unwrap_or("");
                info.row_groups.iter().map(|r| r.column_bytes.get(top).copied().unwrap_or(0)).sum()
            }),
            prune: PruneReport {
                method: if has_stats && applied > 0 { PruneMethod::CoveringStats } else { PruneMethod::NoStatistics },
                total_row_groups: info.row_groups.len(),
                skipped: info.row_groups.len() - kept.len(),
                kept,
                total_rows: info.row_count,
                kept_rows,
                total_bytes,
                kept_bytes,
            },
        });
    }
    Ok(out)
}

fn collect(
    plan: &Plan,
    id: &NodeId,
    constraints: &[Constraint],
    out: &mut BTreeMap<NodeId, Vec<Vec<Constraint>>>,
    seen: &mut HashSet<(NodeId, String)>,
) {
    let sig = constraints.iter().map(|c| format!("{}:{:?}", c.column, (c.bbox.min_x(), c.bbox.min_y(), c.bbox.max_x(), c.bbox.max_y()))).collect::<Vec<_>>().join("|");
    if !seen.insert((id.clone(), sig)) {
        return; // same node reached with the same constraints: already recorded
    }
    let Some(node) = plan.nodes.get(id) else { return };
    match node {
        PlanNode::Source { .. } => out.entry(id.clone()).or_default().push(constraints.to_vec()),
        PlanNode::BboxFilter { input, geometry_column, bbox } => {
            let mut c = constraints.to_vec();
            c.push(Constraint { column: geometry_column.clone(), bbox: bbox.clone() });
            collect(plan, input, &c, out, seen);
        }
        PlanNode::Intersects { input, geometry_column, with } => {
            let mut c = constraints.to_vec();
            if let Some(b) = wkt_envelope(&with.wkt, &with.crs) {
                c.push(Constraint { column: geometry_column.clone(), bbox: b });
            }
            collect(plan, input, &c, out, seen);
        }
        PlanNode::Filter { input, .. } | PlanNode::Sort { input, .. } | PlanNode::Project { input, .. } => {
            collect(plan, input, constraints, out, seen)
        }
        // constraints above a CRS change or a buffer are in other coordinates: they cannot be pushed into the source
        PlanNode::Limit { input, .. }
        | PlanNode::Aggregate { input, .. }
        | PlanNode::Buffer { input, .. }
        | PlanNode::Transform { input, .. } => collect(plan, input, &[], out, seen),
        PlanNode::SpatialJoin { left, right, .. } | PlanNode::SpatialSemiJoin { left, right, .. } => {
            collect(plan, left, &[], out, seen);
            collect(plan, right, &[], out, seen);
        }
    }
}

/// Average uncompressed bytes per row over the columns selected by `pick`, from the row-group metadata.
fn uncompressed_row_bytes(info: &crate::SourceInfo, pick: impl Fn(&str) -> bool) -> f64 {
    let rows: u64 = info.row_groups.iter().map(|r| r.rows).sum();
    if rows == 0 {
        return 0.0;
    }
    let bytes: u64 = info
        .row_groups
        .iter()
        .flat_map(|r| r.uncompressed_column_bytes.iter())
        .filter(|(c, _)| pick(c))
        .map(|(_, b)| *b)
        .sum();
    bytes as f64 / rows as f64
}

fn rg_area(b: &[f64; 4]) -> f64 {
    ((b[2] - b[0]) * (b[3] - b[1])).max(0.0)
}

/// Spatial coherence from row-group bboxes: sum(row-group area) relative to the file bbox area.
/// A perfectly tiled file gives ~1, a shuffled one ~number of row groups. Mapped to [0, 1].
fn coherence(info: &crate::SourceInfo) -> Option<f64> {
    let fb = info.file_bbox?;
    let fa = rg_area(&fb);
    let boxes: Vec<&[f64; 4]> = info.row_groups.iter().filter_map(|r| r.bbox.as_ref()).collect();
    if fa <= 0.0 || boxes.len() < 2 || boxes.len() != info.row_groups.len() {
        return None;
    }
    let n = boxes.len() as f64;
    let ratio = boxes.iter().map(|b| rg_area(b)).sum::<f64>() / fa;
    Some(((n - ratio) / (n - 1.0)).clamp(0.0, 1.0))
}

fn density_skew(info: &crate::SourceInfo) -> Option<f64> {
    let mut d: Vec<f64> = info
        .row_groups
        .iter()
        .filter_map(|r| r.bbox.as_ref().map(|b| (r.rows as f64, rg_area(b))))
        .filter(|(_, a)| *a > 0.0)
        .map(|(rows, a)| rows / a)
        .collect();
    if d.len() < 2 {
        return None;
    }
    d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med = d[d.len() / 2];
    (med > 0.0).then(|| d[d.len() - 1] / med)
}
