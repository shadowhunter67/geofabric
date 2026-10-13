//! Plan-level scan analysis: which row groups can a plan's spatial predicates
//! skip? Shared by the planner (explanations) and engine adapters (metrics).
//!
//! Predicates only constrain a source when every operator between the
//! predicate and the source is row-preserving (Filter, BboxFilter,
//! Intersects, Sort, Project). Limit / Aggregate / Buffer / SpatialJoin
//! reset the constraint set (pushing a filter below them would change
//! results).

use crate::{GeoParquetSource, PruneMethod, PruneReport, SourceError};
use geo_ir::{Node, Plan};
use geo_types::{BoundingBox, Crs, SourceAccessGuard};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize)]
pub struct SourceScan {
    pub uri: String,
    pub crs: Crs,
    pub total_rows: u64,
    pub total_bytes: u64,
    pub constraints_applied: usize,
    pub constraints_skipped: Vec<String>,
    pub prune: PruneReport,
}

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
    let mut out = Vec::new();
    walk(&plan.root, &[], guard, &mut out)?;
    Ok(out)
}

fn walk(node: &Node, constraints: &[Constraint], guard: &dyn SourceAccessGuard, out: &mut Vec<SourceScan>) -> Result<(), SourceError> {
    match node {
        Node::Source { uri, .. } => {
            let authorized = guard.authorize(uri).map_err(|_| SourceError::AccessDenied)?;
            let src = GeoParquetSource::open_authorized(&authorized)?;
            let info = src.info();
            let mut kept: Option<BTreeSet<usize>> = None;
            let mut applied = 0;
            let mut skipped = Vec::new();
            for c in constraints {
                if c.column != info.geometry_column {
                    skipped.push(format!("column '{}' is not the source geometry column", c.column));
                    continue;
                }
                match src.prune(&c.bbox) {
                    Ok(r) => {
                        applied += 1;
                        let s: BTreeSet<usize> = r.kept.into_iter().collect();
                        kept = Some(match kept {
                            None => s,
                            Some(k) => k.intersection(&s).copied().collect(),
                        });
                    }
                    Err(e) => skipped.push(e.to_string()),
                }
            }
            let kept: Vec<usize> = match kept {
                Some(k) => k.into_iter().collect(),
                None => info.row_groups.iter().map(|r| r.index).collect(),
            };
            let total_bytes: u64 = info.row_groups.iter().map(|r| r.compressed_bytes).sum();
            let kept_rows: u64 = kept.iter().map(|i| info.row_groups[*i].rows).sum();
            let kept_bytes = src.estimate_bytes(None, &kept);
            let has_stats = info.row_groups.iter().any(|r| r.bbox.is_some());
            out.push(SourceScan {
                uri: uri.clone(),
                crs: info.crs.clone(),
                total_rows: info.row_count,
                total_bytes,
                constraints_applied: applied,
                constraints_skipped: skipped,
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
        Node::BboxFilter { input, geometry_column, bbox } => {
            let mut c: Vec<Constraint> = constraints.iter().map(clone_c).collect();
            c.push(Constraint { column: geometry_column.clone(), bbox: bbox.clone() });
            walk(input, &c, guard, out)?;
        }
        Node::Intersects { input, geometry_column, with } => {
            let mut c: Vec<Constraint> = constraints.iter().map(clone_c).collect();
            if let Some(b) = wkt_envelope(&with.wkt, &with.crs) {
                c.push(Constraint { column: geometry_column.clone(), bbox: b });
            }
            walk(input, &c, guard, out)?;
        }
        Node::Filter { input, .. } | Node::Sort { input, .. } | Node::Project { input, .. } => {
            let c: Vec<Constraint> = constraints.iter().map(clone_c).collect();
            walk(input, &c, guard, out)?;
        }
        Node::Limit { input, .. } | Node::Aggregate { input, .. } | Node::Buffer { input, .. } => {
            walk(input, &[], guard, out)?;
        }
        Node::SpatialJoin { left, right, .. } => {
            walk(left, &[], guard, out)?;
            walk(right, &[], guard, out)?;
        }
    }
    Ok(())
}

fn clone_c(c: &Constraint) -> Constraint {
    Constraint { column: c.column.clone(), bbox: c.bbox.clone() }
}
