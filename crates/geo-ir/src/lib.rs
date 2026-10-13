//! Engine-neutral logical Spatial IR (RFC-001).
//!
//! No SQL, no engine function names. Parse != validate != execute:
//! a plan with an unknown CRS deserializes fine but fails [`validate`] when
//! an operation is CRS-sensitive.

mod expr;
mod node;
mod schema;
mod validate;

pub use expr::{AggFunc, AggExpr, CompareOp, Expr, Literal, SortKey};
pub use node::{
    GeometryLiteral, Hints, JoinType, Node, OpKind, Plan, SourceFormat, SourceStats, SpatialPredicate,
};
pub use validate::{validate, Issue, Severity, ValidationReport};

/// Version string written to / required from serialized plans.
pub const IR_VERSION: &str = "0.1";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IrError {
    #[error("invalid IR json: {0}")]
    Parse(String),
    #[error("unsupported ir_version '{found}' (supported: {supported})")]
    UnsupportedVersion { found: String, supported: String },
    #[error("column '{0}' not found")]
    ColumnNotFound(String),
    #[error("schema error: {0}")]
    Schema(String),
}
