//! Semantic validation (separate from parsing and execution).
//!
//! Rules: CRS-sensitive ops need known CRSs and matching CRSs (no implicit
//! transform); metric distances need a projected CRS; angular distances need
//! a geographic CRS; unknown geographic-ness is rejected, never guessed.

use crate::{IrError, Node, Plan};
use geo_types::{Crs, DistanceUnit};
use serde::Serialize;

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
}

/// Validate a plan. Never mutates; never "fixes" anything.
pub fn validate(plan: &Plan) -> ValidationReport {
    let mut r = ValidationReport::default();
    if plan.ir_version != crate::IR_VERSION {
        r.push_err("unsupported_version", "plan", format!("ir_version {}", plan.ir_version));
    }
    visit(&plan.root, &mut r);
    r
}

impl ValidationReport {
    fn push_err(&mut self, code: &'static str, node: &str, message: String) {
        self.issues.push(Issue { severity: Severity::Error, code, node: node.to_string(), message });
    }
    fn push_warn(&mut self, code: &'static str, node: &str, message: String) {
        self.issues.push(Issue { severity: Severity::Warning, code, node: node.to_string(), message });
    }
    fn schema_err(&mut self, node: &str, e: IrError) {
        let code = match e {
            IrError::ColumnNotFound(_) => "column_not_found",
            _ => "schema",
        };
        self.push_err(code, node, e.to_string());
    }
}

fn require_known(r: &mut ValidationReport, id: &str, what: &str, crs: &Crs) -> bool {
    if crs.is_known() {
        true
    } else {
        r.push_err("unknown_crs", id, format!("{what} has unknown CRS '{crs}'; CRS-sensitive operation rejected"));
        false
    }
}

fn visit(n: &Node, r: &mut ValidationReport) {
    for i in n.inputs() {
        visit(i, r);
    }
    let id = format!("{}:{}", n.kind().name(), &n.node_id()[..8]);
    if let Err(e) = n.output_schema() {
        r.schema_err(&id, e);
        return; // downstream checks need a schema
    }
    match n {
        Node::Source { .. } => {}
        Node::Filter { input, predicate } => {
            let s = input.output_schema().expect("checked via output_schema");
            let mut cols = vec![];
            predicate.columns(&mut cols);
            for c in cols {
                if !s.fields.iter().any(|f| f.name == c) {
                    r.push_err("column_not_found", &id, format!("column '{c}' not found"));
                } else if s.geometry(&c).is_some() {
                    r.push_err("geometry_in_filter", &id, format!("geometry column '{c}' in attribute filter"));
                }
            }
        }
        Node::BboxFilter { input, geometry_column, bbox } => {
            let s = input.output_schema().expect("checked");
            match s.geometry(geometry_column) {
                None => r.push_err("not_geometry", &id, format!("'{geometry_column}' is not a geometry column")),
                Some(g) => {
                    let a = require_known(r, &id, "geometry column", &g.crs);
                    let b = require_known(r, &id, "bbox", bbox.crs());
                    if a && b && g.crs != *bbox.crs() {
                        r.push_err(
                            "crs_mismatch",
                            &id,
                            format!("bbox CRS {} != column CRS {}; explicit transform required", bbox.crs(), g.crs),
                        );
                    }
                }
            }
        }
        Node::Intersects { input, geometry_column, with } => {
            let s = input.output_schema().expect("checked");
            if with.wkt.trim().is_empty() {
                r.push_err("empty_geometry", &id, "empty WKT literal".into());
            }
            match s.geometry(geometry_column) {
                None => r.push_err("not_geometry", &id, format!("'{geometry_column}' is not a geometry column")),
                Some(g) => {
                    let a = require_known(r, &id, "geometry column", &g.crs);
                    let b = require_known(r, &id, "literal geometry", &with.crs);
                    if a && b && g.crs != with.crs {
                        r.push_err(
                            "crs_mismatch",
                            &id,
                            format!("literal CRS {} != column CRS {}; explicit transform required", with.crs, g.crs),
                        );
                    }
                }
            }
        }
        Node::Buffer { input, geometry_column, distance, input_crs, output_crs, error_tolerance, .. } => {
            let s = input.output_schema().expect("checked");
            if let Some(g) = s.geometry(geometry_column) {
                if g.crs != *input_crs {
                    r.push_err(
                        "crs_mismatch",
                        &id,
                        format!("declared input_crs {input_crs} != column CRS {}", g.crs),
                    );
                }
            } else {
                r.push_err("not_geometry", &id, format!("'{geometry_column}' is not a geometry column"));
            }
            let in_ok = require_known(r, &id, "buffer input_crs", input_crs);
            require_known(r, &id, "buffer output_crs", output_crs);
            if in_ok {
                match (distance.unit().is_angular(), input_crs.is_geographic()) {
                    (false, Some(true)) => r.push_err(
                        "metric_on_geographic",
                        &id,
                        format!("{} buffer on geographic CRS {input_crs}: transform to a projected CRS first", unit_name(distance.unit())),
                    ),
                    (true, Some(false)) => r.push_err(
                        "angular_on_projected",
                        &id,
                        format!("degree buffer on projected CRS {input_crs}"),
                    ),
                    (_, None) => r.push_err(
                        "crs_kind_unknown",
                        &id,
                        format!("cannot tell whether {input_crs} is geographic or projected; refusing to guess"),
                    ),
                    (true, Some(true)) => r.push_warn(
                        "angular_buffer",
                        &id,
                        "degree buffers are not isotropic; result shape depends on latitude".into(),
                    ),
                    _ => {}
                }
            }
            if let Some(t) = error_tolerance {
                if t.unit().is_angular() != distance.unit().is_angular() {
                    r.push_err("unit_mismatch", &id, "error_tolerance and distance must be both linear or both angular".into());
                }
            }
        }
        Node::SpatialJoin { left, right, left_column, right_column, .. } => {
            let ls = left.output_schema().expect("checked");
            let rs = right.output_schema().expect("checked");
            match (ls.geometry(left_column), rs.geometry(right_column)) {
                (Some(a), Some(b)) => {
                    let ok1 = require_known(r, &id, "left geometry", &a.crs);
                    let ok2 = require_known(r, &id, "right geometry", &b.crs);
                    if ok1 && ok2 && a.crs != b.crs {
                        r.push_err(
                            "crs_mismatch",
                            &id,
                            format!("left CRS {} != right CRS {}; explicit transform required", a.crs, b.crs),
                        );
                    }
                }
                _ => r.push_err("not_geometry", &id, "join columns must both be geometry columns".into()),
            }
        }
        Node::Sort { input, keys } => {
            let s = input.output_schema().expect("checked");
            for k in keys {
                if s.geometry(&k.column).is_some() {
                    r.push_err("sort_geometry", &id, format!("cannot sort by geometry column '{}'", k.column));
                }
            }
        }
        Node::Aggregate { .. } | Node::Project { .. } | Node::Limit { .. } => {}
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
