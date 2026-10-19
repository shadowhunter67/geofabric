//! Engine-neutral logical Spatial IR (RFC-001).
//!
//! No SQL, no engine function names. Parse != validate != execute:
//! a plan with an unknown CRS deserializes fine but fails [`validate`] when
//! an operation is CRS-sensitive.

mod expr;
mod graph;
mod node;
mod schema;
mod validate;

pub use expr::{AggFunc, AggExpr, CompareOp, Expr, Literal, SortKey};
pub use graph::{migrate_v0_1, GraphBuilder, GraphError, NodeId, Plan, PlanNode, TreePlanV01, IR_VERSION, IR_VERSION_PREVIOUS, IR_VERSION_TREE};
pub use node::{GeometryLiteral, Hints, JoinType, Node, OpKind, SourceFormat, SourceStats, SpatialPredicate};
pub use validate::{validate, Issue, Severity, ValidationReport};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IrError {
    #[error("invalid IR json: {0}")]
    Parse(String),
    #[error("unsupported ir_version '{found}' (supported: {supported})")]
    UnsupportedVersion { found: String, supported: String },
    #[error("graph error: {0}")]
    Graph(String),
    #[error("column '{0}' not found")]
    ColumnNotFound(String),
    #[error("schema error: {0}")]
    Schema(String),
}
