//! Plan-time workload features (`geo_executor::WorkloadFeatures`): what an engine's cost model may use.
//!
//! Computed from the plan graph, the source scans (Parquet footer statistics / histogram sidecar) and the row estimator - the same inputs the
//! planner already has before anything runs. Nothing here is a runtime observation, and nothing is engine specific: no SQL, no index names.
//! Cardinality estimation (estimator.rs) and engine cost (an engine's `cost_estimate`) stay separate concerns; this module is the interface
//! between them.

use crate::estimator::{plan_shape, RowEstimate};
use geo_executor::{SemiJoinFeatures, SourceFeatures, WorkloadFeatures};
use geo_ir::{NodeId, Plan, PlanNode};
use geo_source_geoparquet::SourceScan;
use geo_types::{DistanceUnit, Crs};
use std::collections::BTreeSet;

/// Source URIs reachable from `id` (the leaves of that sub-graph).
fn leaf_sources(plan: &Plan, id: &NodeId, out: &mut BTreeSet<String>, metric_seen: &mut bool) {
    match plan.nodes.get(id) {
        Some(PlanNode::Source { uri, .. }) => {
            out.insert(uri.clone());
        }
        Some(n) => {
            if let PlanNode::Transform { output_crs, .. } = n {
                if matches!(output_crs, Crs::Epsg(_)) {
                    *metric_seen = true;
                }
            }
            for i in n.inputs() {
                leaf_sources(plan, i, out, metric_seen);
            }
        }
        None => {}
    }
}

fn selected(s: &SourceScan) -> f64 {
    s.raw_selected_rows * (1.0 - s.null_fraction.unwrap_or(0.0))
}

pub fn extract(plan: &Plan, scans: &[SourceScan], est: &RowEstimate, result_geom_bytes: f64, result_attr_bytes: f64) -> WorkloadFeatures {
    let shape = plan_shape(plan);
    let nodes = || plan.nodes.values();
    let has = |f: &dyn Fn(&PlanNode) -> bool| nodes().any(|n| f(n));
    let sources: Vec<SourceFeatures> = scans.iter().map(|s| SourceFeatures { uri: s.uri.clone(), total_rows: s.total_rows, selected_rows: selected(s) }).collect();
    let bbox_selectivity = scans.first().filter(|s| s.total_rows > 0).map(|s| (selected(s) / s.total_rows as f64).clamp(0.0, 1.0));

    let semi_join = nodes().find_map(|n| match n {
        PlanNode::SpatialSemiJoin { left, right, predicate, .. } => {
            let (mut l, mut r) = (BTreeSet::new(), BTreeSet::new());
            let (mut metric_l, mut metric_r) = (false, false);
            leaf_sources(plan, left, &mut l, &mut metric_l);
            leaf_sources(plan, right, &mut r, &mut metric_r);
            let rows_of = |set: &BTreeSet<String>| -> (f64, u64) {
                let mut sel = 0.0;
                let mut total = 0;
                for s in scans.iter().filter(|s| set.contains(&s.uri)) {
                    sel += selected(s);
                    total += s.total_rows;
                }
                (sel, total)
            };
            let ((left_rows, left_total), (right_rows, right_total)) = (rows_of(&l), rows_of(&r));
            let (pred, distance_m) = match predicate {
                geo_ir::SpatialPredicate::Intersects => ("intersects", None),
                geo_ir::SpatialPredicate::WithinDistance { distance } => ("within_distance", if distance.unit() == DistanceUnit::Degree { None } else { distance.to_meters().ok() }),
            };
            Some(SemiJoinFeatures {
                left_uri: l.iter().next().cloned().unwrap_or_default(),
                right_uri: r.iter().next().cloned().unwrap_or_default(),
                left_rows,
                right_rows,
                left_total_rows: left_total,
                right_total_rows: right_total,
                metric: metric_l || metric_r,
                predicate: pred.into(),
                distance_m,
                // one right-side probe per left row that survives the left-side filters
                expected_probes: left_rows,
            })
        }
        _ => None,
    });

    WorkloadFeatures {
        sources,
        result_rows: est.rows,
        result_rows_high: est.high,
        cardinality: est.cardinality,
        result_geom_bytes,
        result_attr_bytes,
        bbox_selectivity,
        shape: shape.kind().into(),
        has_bbox_filter: has(&|n| matches!(n, PlanNode::BboxFilter { .. })),
        has_filter: has(&|n| matches!(n, PlanNode::Filter { .. })),
        has_sort: has(&|n| matches!(n, PlanNode::Sort { .. })),
        has_aggregate: has(&|n| matches!(n, PlanNode::Aggregate { .. })),
        has_intersects: has(&|n| matches!(n, PlanNode::Intersects { .. })),
        has_transform: has(&|n| matches!(n, PlanNode::Transform { .. })),
        limit: shape.limit,
        limit_certain: est.limit_certain,
        bounded_by_limit: est.bounded_by_limit,
        semi_join,
        estimate_quality: format!("{:?}", est.quality).to_lowercase(),
    }
}
