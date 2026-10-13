//! Semantic validation (separate from parsing and execution), on the PlanGraph.
//!
//! Structure first (root, references, cycles, orphans), then every node once in
//! topological order. Rules: CRS-sensitive ops need known CRSs and matching CRSs
//! (no implicit transform); metric distances need a projected CRS; angular
//! distances need a geographic CRS; unknown geographic-ness is rejected, never
//! guessed.

use crate::graph::{GraphError, NodeId, Plan, PlanNode};
use crate::schema::derive_schema;
use crate::IrError;
use geo_types::{Crs, DistanceUnit, SpatialSchema};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    pub node: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ValidationReport {
    pub issues: Vec<Issue>,
}

impl ValidationReport {
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }
    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Warning)
    }
    pub fn is_ok(&self) -> bool {
        self.errors().next().is_none()
    }
    pub fn has_code(&self, code: &str) -> bool {
        self.issues.iter().any(|i| i.code == code)
    }
    fn push_err(&mut self, code: &'static str, node: &str, message: String) {
        self.issues.push(Issue { severity: Severity::Error, code, node: node.to_string(), message });
    }
    fn push_warn(&mut self, code: &'static str, node: &str, message: String) {
        self.issues.push(Issue { severity: Severity::Warning, code, node: node.to_string(), message });
    }
}

/// Validate a plan. Never mutates; never "fixes" anything.
pub fn validate(plan: &Plan) -> ValidationReport {
    let mut r = ValidationReport::default();
    if plan.ir_version != crate::IR_VERSION {
        r.push_err("unsupported_version", "plan", format!("ir_version {}", plan.ir_version));
    }
    if let Err(e) = plan.check_structure() {
        let (code, node) = match &e {
            GraphError::RootMissing(n) => ("graph_root_missing", n.0.clone()),
            GraphError::MissingNode { from, .. } => ("graph_missing_node", from.0.clone()),
            GraphError::Cycle(n) => ("graph_cycle", n.0.clone()),
            GraphError::Orphan(n) => ("graph_orphan", n.0.clone()),
        };
        r.push_err(code, &node, e.to_string());
        return r;
    }
    let order = plan.topological_order().expect("structure was checked");
    let mut schemas: BTreeMap<NodeId, SpatialSchema> = BTreeMap::new();
    let mut failed: BTreeSet<NodeId> = BTreeSet::new();
    for id in order {
        let node = &plan.nodes[&id];
        let label = format!("{}:{}", node.kind().name(), id);
        let inputs = node.inputs();
        if inputs.iter().any(|i| failed.contains(*i)) {
            failed.insert(id);
            continue; // the failing input already carries the error
        }
        let ins: Vec<&SpatialSchema> = inputs.iter().map(|i| &schemas[*i]).collect();
        match derive_schema(node, &ins) {
            Err(e) => {
                let code = match e {
                    IrError::ColumnNotFound(_) => "column_not_found",
                    _ => "schema",
                };
                r.push_err(code, &label, e.to_string());
                failed.insert(id);
                continue;
            }
            Ok(s) => {
                check_node(&label, node, &ins, &mut r);
                schemas.insert(id, s);
            }
        }
    }
    r
}

fn require_known(r: &mut ValidationReport, id: &str, what: &str, crs: &Crs) -> bool {
    if crs.is_known() {
        true
    } else {
        r.push_err("unknown_crs", id, format!("{what} has unknown CRS '{crs}'; CRS-sensitive operation rejected"));
        false
    }
}

fn check_node(id: &str, n: &PlanNode, ins: &[&SpatialSchema], r: &mut ValidationReport) {
    match n {
        PlanNode::Source { .. } | PlanNode::Aggregate { .. } | PlanNode::Project { .. } | PlanNode::Limit { .. } => {}
        PlanNode::Filter { predicate, .. } => {
            let s = ins[0];
            let mut cols = vec![];
            predicate.columns(&mut cols);
            for c in cols {
                if !s.fields.iter().any(|f| f.name == c) {
                    r.push_err("column_not_found", id, format!("column '{c}' not found"));
                } else if s.geometry(&c).is_some() {
                    r.push_err("geometry_in_filter", id, format!("geometry column '{c}' in attribute filter"));
                }
            }
        }
        PlanNode::BboxFilter { geometry_column, bbox, .. } => match ins[0].geometry(geometry_column) {
            None => r.push_err("not_geometry", id, format!("'{geometry_column}' is not a geometry column")),
            Some(g) => {
                let a = require_known(r, id, "geometry column", &g.crs);
                let b = require_known(r, id, "bbox", bbox.crs());
                if a && b && g.crs != *bbox.crs() {
                    r.push_err(
                        "crs_mismatch",
                        id,
                        format!("bbox CRS {} != column CRS {}; explicit transform required", bbox.crs(), g.crs),
                    );
                }
            }
        },
        PlanNode::Intersects { geometry_column, with, .. } => {
            if with.wkt.trim().is_empty() {
                r.push_err("empty_geometry", id, "empty WKT literal".into());
            }
            match ins[0].geometry(geometry_column) {
                None => r.push_err("not_geometry", id, format!("'{geometry_column}' is not a geometry column")),
                Some(g) => {
                    let a = require_known(r, id, "geometry column", &g.crs);
                    let b = require_known(r, id, "literal geometry", &with.crs);
                    if a && b && g.crs != with.crs {
                        r.push_err(
                            "crs_mismatch",
                            id,
                            format!("literal CRS {} != column CRS {}; explicit transform required", with.crs, g.crs),
                        );
                    }
                }
            }
        }
        PlanNode::Buffer { geometry_column, distance, input_crs, output_crs, error_tolerance, .. } => {
            if let Some(g) = ins[0].geometry(geometry_column) {
                if g.crs != *input_crs {
                    r.push_err("crs_mismatch", id, format!("declared input_crs {input_crs} != column CRS {}", g.crs));
                }
            } else {
                r.push_err("not_geometry", id, format!("'{geometry_column}' is not a geometry column"));
            }
            let in_ok = require_known(r, id, "buffer input_crs", input_crs);
            require_known(r, id, "buffer output_crs", output_crs);
            if in_ok {
                match (distance.unit().is_angular(), input_crs.is_geographic()) {
                    (false, Some(true)) => r.push_err(
                        "metric_on_geographic",
                        id,
                        format!(
                            "{} buffer on geographic CRS {input_crs}: transform to a projected CRS first",
                            unit_name(distance.unit())
                        ),
                    ),
                    (true, Some(false)) => {
                        r.push_err("angular_on_projected", id, format!("degree buffer on projected CRS {input_crs}"))
                    }
                    (_, None) => r.push_err(
                        "crs_kind_unknown",
                        id,
                        format!("cannot tell whether {input_crs} is geographic or projected; refusing to guess"),
                    ),
                    (true, Some(true)) => r.push_warn(
                        "angular_buffer",
                        id,
                        "degree buffers are not isotropic; result shape depends on latitude".into(),
                    ),
                    _ => {}
                }
            }
            if let Some(t) = error_tolerance {
                if t.unit().is_angular() != distance.unit().is_angular() {
                    r.push_err("unit_mismatch", id, "error_tolerance and distance must be both linear or both angular".into());
                }
            }
        }
        PlanNode::SpatialJoin { left_column, right_column, .. } => match (ins[0].geometry(left_column), ins[1].geometry(right_column)) {
            (Some(a), Some(b)) => {
                let ok1 = require_known(r, id, "left geometry", &a.crs);
                let ok2 = require_known(r, id, "right geometry", &b.crs);
                if ok1 && ok2 && a.crs != b.crs {
                    r.push_err(
                        "crs_mismatch",
                        id,
                        format!("left CRS {} != right CRS {}; explicit transform required", a.crs, b.crs),
                    );
                }
            }
            _ => r.push_err("not_geometry", id, "join columns must both be geometry columns".into()),
        },
        PlanNode::Sort { keys, .. } => {
            for k in keys {
                if ins[0].geometry(&k.column).is_some() {
                    r.push_err("sort_geometry", id, format!("cannot sort by geometry column '{}'", k.column));
                }
            }
        }
    }
}

fn unit_name(u: DistanceUnit) -> &'static str {
    match u {
        DistanceUnit::Meter => "meter",
        DistanceUnit::Kilometer => "kilometer",
        DistanceUnit::Foot => "foot",
        DistanceUnit::Degree => "degree",
    }
}
