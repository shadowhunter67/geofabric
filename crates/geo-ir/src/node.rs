use crate::expr::{AggExpr, Expr, SortKey};
use crate::IR_VERSION;
use geo_types::{BoundingBox, Crs, Distance, SpatialSchema};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    GeoParquet,
}

/// Optional source statistics (filled by a source adapter, used by planner).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceStats {
    pub row_count: Option<u64>,
    pub total_bytes: Option<u64>,
    pub row_groups: Option<u32>,
    pub bbox: Option<BoundingBox>,
}

/// Literal geometry in an explicit CRS. WKT is the boundary encoding; this is
/// not GeoJSON and never used on the data hot path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryLiteral {
    pub wkt: String,
    pub crs: Crs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialPredicate {
    Intersects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinType {
    Inner,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Source,
    Filter,
    BboxFilter,
    Intersects,
    Buffer,
    SpatialJoin,
    Aggregate,
    Project,
    Limit,
    Sort,
}

impl OpKind {
    pub fn name(self) -> &'static str {
        match self {
            OpKind::Source => "source",
            OpKind::Filter => "filter",
            OpKind::BboxFilter => "bbox_filter",
            OpKind::Intersects => "intersects",
            OpKind::Buffer => "buffer",
            OpKind::SpatialJoin => "spatial_join",
            OpKind::Aggregate => "aggregate",
            OpKind::Project => "project",
            OpKind::Limit => "limit",
            OpKind::Sort => "sort",
        }
    }
}

/// A logical operator tree node. (v0: tree; shared sub-plans are duplicated.
/// Content hashes via [`Node::node_id`] make a DAG representation possible
/// later without changing node semantics.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Node {
    Source {
        /// Path or URL. Trust/validation of the location is the planner's /
        /// executor's job, not the IR's.
        uri: String,
        format: SourceFormat,
        schema: SpatialSchema,
        #[serde(default)]
        statistics: SourceStats,
    },
    Filter { input: Box<Node>, predicate: Expr },
    BboxFilter { input: Box<Node>, geometry_column: String, bbox: BoundingBox },
    Intersects { input: Box<Node>, geometry_column: String, with: GeometryLiteral },
    /// Distance is mandatory with its unit; both CRSs are explicit.
    Buffer {
        input: Box<Node>,
        geometry_column: String,
        distance: Distance,
        input_crs: Crs,
        output_crs: Crs,
        error_tolerance: Option<Distance>,
        output_column: String,
    },
    SpatialJoin {
        left: Box<Node>,
        right: Box<Node>,
        left_column: String,
        right_column: String,
        predicate: SpatialPredicate,
        join_type: JoinType,
        /// Prefix added to every right-side column in the output (explicit
        /// so self-joins and name clashes never rename silently).
        right_prefix: String,
    },
    Aggregate { input: Box<Node>, group_by: Vec<String>, aggregates: Vec<AggExpr> },
    Project { input: Box<Node>, columns: Vec<String> },
    Limit { input: Box<Node>, n: u64 },
    Sort { input: Box<Node>, keys: Vec<SortKey> },
}

impl Node {
    pub fn kind(&self) -> OpKind {
        match self {
            Node::Source { .. } => OpKind::Source,
            Node::Filter { .. } => OpKind::Filter,
            Node::BboxFilter { .. } => OpKind::BboxFilter,
            Node::Intersects { .. } => OpKind::Intersects,
            Node::Buffer { .. } => OpKind::Buffer,
            Node::SpatialJoin { .. } => OpKind::SpatialJoin,
            Node::Aggregate { .. } => OpKind::Aggregate,
            Node::Project { .. } => OpKind::Project,
            Node::Limit { .. } => OpKind::Limit,
            Node::Sort { .. } => OpKind::Sort,
        }
    }

    pub fn inputs(&self) -> Vec<&Node> {
        match self {
            Node::Source { .. } => vec![],
            Node::SpatialJoin { left, right, .. } => vec![left, right],
            Node::Filter { input, .. }
            | Node::BboxFilter { input, .. }
            | Node::Intersects { input, .. }
            | Node::Buffer { input, .. }
            | Node::Aggregate { input, .. }
            | Node::Project { input, .. }
            | Node::Limit { input, .. }
            | Node::Sort { input, .. } => vec![input],
        }
    }

    /// Content hash (sha256 hex) of this node and its inputs, including the IR
    /// version. Deterministic: serialization has no maps.
    pub fn node_id(&self) -> String {
        let mut h = Sha256::new();
        h.update(IR_VERSION.as_bytes());
        h.update(serde_json::to_vec(self).expect("IR is always serializable"));
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Post-order (sources first) list of `(node_id, operation)` — the minimal
    /// lineage chain of this node.
    pub fn lineage(&self) -> Vec<(String, OpKind)> {
        let mut out = Vec::new();
        fn walk(n: &Node, out: &mut Vec<(String, OpKind)>) {
            for i in n.inputs() {
                walk(i, out);
            }
            out.push((n.node_id(), n.kind()));
        }
        walk(self, &mut out);
        out
    }
}

/// Execution hints: advisory only, never change result semantics.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hints {
    /// Engine id preference; planner may ignore it (and must say so).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_engine: Option<String>,
}
