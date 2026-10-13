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

use crate::{GeoParquetSource, PruneMethod, PruneReport, SourceError};
use geo_ir::{NodeId, Plan, PlanNode};
use geo_types::{BoundingBox, Crs, SourceAccessGuard};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, Serialize)]
pub struct SourceScan {
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
        let kept: Vec<usize> = kept.into_iter().collect();
        let total_bytes: u64 = info.row_groups.iter().map(|r| r.compressed_bytes).sum();
        let kept_rows: u64 = kept.iter().map(|i| info.row_groups[*i].rows).sum();
        let kept_bytes = src.estimate_bytes(None, &kept);
        let has_stats = info.row_groups.iter().any(|r| r.bbox.is_some());
        out.push(SourceScan {
            node: id.0.clone(),
            uri: uri.clone(),
            crs: info.crs.clone(),
            total_rows: info.row_count,
            total_bytes,
            constraints_applied: applied,
            constraints_skipped: skipped,
            access_paths: path_sets.len(),
            has_covering: info.covering.is_some(),
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
        PlanNode::Limit { input, .. } | PlanNode::Aggregate { input, .. } | PlanNode::Buffer { input, .. } => {
            collect(plan, input, &[], out, seen)
        }
        PlanNode::SpatialJoin { left, right, .. } => {
            collect(plan, left, &[], out, seen);
            collect(plan, right, &[], out, seen);
        }
    }
}
