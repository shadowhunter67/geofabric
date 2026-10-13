//! PlanGraph (IR 0.2): the logical plan as a DAG.
//!
//! * Nodes live in a map keyed by [`NodeId`] and reference their inputs by id,
//!   so a sub-plan (e.g. a source used twice) is represented **once**.
//! * Ids produced by [`GraphBuilder`] are content hashes (deterministic, equal
//!   sub-plans collapse into one node). Hand-written ids are allowed; they are
//!   opaque. [`Plan::plan_id`] never depends on id spelling (it hashes content).
//! * Structural validation: missing references, cycles, unreachable nodes.
//! * Serialization is stable (ordered maps) and versioned (`ir_version = "0.2"`).
//!   IR 0.1 (nested tree) is accepted by [`Plan::from_json`] and migrated
//!   deterministically ([`migrate_v0_1`]).

use crate::expr::{AggExpr, Expr, SortKey};
use crate::node::{
    GeometryLiteral, Hints, JoinType, Node, OpKind, SourceFormat, SourceStats, SpatialPredicate,
};
use crate::IrError;
use geo_types::{BoundingBox, Crs, Distance, SpatialSchema};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const IR_VERSION: &str = "0.2";
pub const IR_VERSION_TREE: &str = "0.1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(s: impl Into<String>) -> Self {
        NodeId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One operator; inputs are node ids, not nested nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanNode {
    Source {
        uri: String,
        format: SourceFormat,
        schema: SpatialSchema,
        #[serde(default)]
        statistics: SourceStats,
    },
    Filter { input: NodeId, predicate: Expr },
    BboxFilter { input: NodeId, geometry_column: String, bbox: BoundingBox },
    Intersects { input: NodeId, geometry_column: String, with: GeometryLiteral },
    Buffer {
        input: NodeId,
        geometry_column: String,
        distance: Distance,
        input_crs: Crs,
        output_crs: Crs,
        error_tolerance: Option<Distance>,
        output_column: String,
    },
    SpatialJoin {
        left: NodeId,
        right: NodeId,
        left_column: String,
        right_column: String,
        predicate: SpatialPredicate,
        join_type: JoinType,
        right_prefix: String,
    },
    Aggregate { input: NodeId, group_by: Vec<String>, aggregates: Vec<AggExpr> },
    Project { input: NodeId, columns: Vec<String> },
    Limit { input: NodeId, n: u64 },
    Sort { input: NodeId, keys: Vec<SortKey> },
}

impl PlanNode {
    pub fn kind(&self) -> OpKind {
        match self {
            PlanNode::Source { .. } => OpKind::Source,
            PlanNode::Filter { .. } => OpKind::Filter,
            PlanNode::BboxFilter { .. } => OpKind::BboxFilter,
            PlanNode::Intersects { .. } => OpKind::Intersects,
            PlanNode::Buffer { .. } => OpKind::Buffer,
            PlanNode::SpatialJoin { .. } => OpKind::SpatialJoin,
            PlanNode::Aggregate { .. } => OpKind::Aggregate,
            PlanNode::Project { .. } => OpKind::Project,
            PlanNode::Limit { .. } => OpKind::Limit,
            PlanNode::Sort { .. } => OpKind::Sort,
        }
    }

    /// Input ids in operator order (`left`, `right` for joins).
    pub fn inputs(&self) -> Vec<&NodeId> {
        match self {
            PlanNode::Source { .. } => vec![],
            PlanNode::SpatialJoin { left, right, .. } => vec![left, right],
            PlanNode::Filter { input, .. }
            | PlanNode::BboxFilter { input, .. }
            | PlanNode::Intersects { input, .. }
            | PlanNode::Buffer { input, .. }
            | PlanNode::Aggregate { input, .. }
            | PlanNode::Project { input, .. }
            | PlanNode::Limit { input, .. }
            | PlanNode::Sort { input, .. } => vec![input],
        }
    }

    /// Same node with its inputs replaced (same arity, same order as [`Self::inputs`]).
    fn with_inputs(&self, ids: &[NodeId]) -> PlanNode {
        let mut n = self.clone();
        match &mut n {
            PlanNode::Source { .. } => {}
            PlanNode::SpatialJoin { left, right, .. } => {
                *left = ids[0].clone();
                *right = ids[1].clone();
            }
            PlanNode::Filter { input, .. }
            | PlanNode::BboxFilter { input, .. }
            | PlanNode::Intersects { input, .. }
            | PlanNode::Buffer { input, .. }
            | PlanNode::Aggregate { input, .. }
            | PlanNode::Project { input, .. }
            | PlanNode::Limit { input, .. }
            | PlanNode::Sort { input, .. } => *input = ids[0].clone(),
        }
        n
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("root node '{0}' does not exist")]
    RootMissing(NodeId),
    #[error("node '{from}' references missing node '{missing}'")]
    MissingNode { from: NodeId, missing: NodeId },
    #[error("cycle detected at node '{0}'")]
    Cycle(NodeId),
    #[error("node '{0}' is not reachable from the root (orphan)")]
    Orphan(NodeId),
}

/// Content-addressed node insertion: equal nodes (with equal inputs) get one id.
#[derive(Debug, Default)]
pub struct GraphBuilder {
    nodes: BTreeMap<NodeId, PlanNode>,
}

fn content_id(n: &PlanNode) -> NodeId {
    let mut h = Sha256::new();
    h.update(IR_VERSION.as_bytes());
    h.update(serde_json::to_vec(n).expect("IR is always serializable"));
    let hex: String = h.finalize().iter().take(10).map(|b| format!("{b:02x}")).collect();
    NodeId(format!("n_{hex}"))
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, node: PlanNode) -> NodeId {
        let id = content_id(&node);
        self.nodes.entry(id.clone()).or_insert(node);
        id
    }

    pub fn finish(self, root: NodeId) -> Plan {
        Plan { ir_version: IR_VERSION.to_string(), nodes: self.nodes, root, hints: Hints::default() }
    }
}

/// The logical plan (IR 0.2): a DAG of operators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub ir_version: String,
    pub nodes: BTreeMap<NodeId, PlanNode>,
    pub root: NodeId,
    #[serde(default)]
    pub hints: Hints,
}

/// IR 0.1 wire format: nested tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreePlanV01 {
    pub ir_version: String,
    pub root: Node,
    #[serde(default)]
    pub hints: Hints,
}

impl Plan {
    /// Build a graph from a tree-shaped description (convenience builder; equal
    /// sub-trees are shared automatically).
    pub fn new(root: Node) -> Plan {
        Plan::from_tree(&root)
    }

    pub fn from_tree(root: &Node) -> Plan {
        fn go(n: &Node, b: &mut GraphBuilder) -> NodeId {
            let node = match n {
                Node::Source { uri, format, schema, statistics } => {
                    PlanNode::Source { uri: uri.clone(), format: *format, schema: schema.clone(), statistics: statistics.clone() }
                }
                Node::Filter { input, predicate } => PlanNode::Filter { input: go(input, b), predicate: predicate.clone() },
                Node::BboxFilter { input, geometry_column, bbox } => {
                    PlanNode::BboxFilter { input: go(input, b), geometry_column: geometry_column.clone(), bbox: bbox.clone() }
                }
                Node::Intersects { input, geometry_column, with } => {
                    PlanNode::Intersects { input: go(input, b), geometry_column: geometry_column.clone(), with: with.clone() }
                }
                Node::Buffer { input, geometry_column, distance, input_crs, output_crs, error_tolerance, output_column } => PlanNode::Buffer {
                    input: go(input, b),
                    geometry_column: geometry_column.clone(),
                    distance: *distance,
                    input_crs: input_crs.clone(),
                    output_crs: output_crs.clone(),
                    error_tolerance: *error_tolerance,
                    output_column: output_column.clone(),
                },
                Node::SpatialJoin { left, right, left_column, right_column, predicate, join_type, right_prefix } => {
                    let l = go(left, b);
                    let r = go(right, b);
                    PlanNode::SpatialJoin {
                        left: l,
                        right: r,
                        left_column: left_column.clone(),
                        right_column: right_column.clone(),
                        predicate: *predicate,
                        join_type: *join_type,
                        right_prefix: right_prefix.clone(),
                    }
                }
                Node::Aggregate { input, group_by, aggregates } => {
                    PlanNode::Aggregate { input: go(input, b), group_by: group_by.clone(), aggregates: aggregates.clone() }
                }
                Node::Project { input, columns } => PlanNode::Project { input: go(input, b), columns: columns.clone() },
                Node::Limit { input, n } => PlanNode::Limit { input: go(input, b), n: *n },
                Node::Sort { input, keys } => PlanNode::Sort { input: go(input, b), keys: keys.clone() },
            };
            b.add(node)
        }
        let mut b = GraphBuilder::new();
        let root_id = go(root, &mut b);
        b.finish(root_id)
    }

    /// Expand to a tree (shared nodes are duplicated). Fails if the graph is malformed.
    pub fn to_tree(&self) -> Result<Node, GraphError> {
        self.check_structure()?;
        fn go(p: &Plan, id: &NodeId) -> Node {
            let n = &p.nodes[id];
            let b = |i: &NodeId| Box::new(go(p, i));
            match n {
                PlanNode::Source { uri, format, schema, statistics } => {
                    Node::Source { uri: uri.clone(), format: *format, schema: schema.clone(), statistics: statistics.clone() }
                }
                PlanNode::Filter { input, predicate } => Node::Filter { input: b(input), predicate: predicate.clone() },
                PlanNode::BboxFilter { input, geometry_column, bbox } => {
                    Node::BboxFilter { input: b(input), geometry_column: geometry_column.clone(), bbox: bbox.clone() }
                }
                PlanNode::Intersects { input, geometry_column, with } => {
                    Node::Intersects { input: b(input), geometry_column: geometry_column.clone(), with: with.clone() }
                }
                PlanNode::Buffer { input, geometry_column, distance, input_crs, output_crs, error_tolerance, output_column } => Node::Buffer {
                    input: b(input),
                    geometry_column: geometry_column.clone(),
                    distance: *distance,
                    input_crs: input_crs.clone(),
                    output_crs: output_crs.clone(),
                    error_tolerance: *error_tolerance,
                    output_column: output_column.clone(),
                },
                PlanNode::SpatialJoin { left, right, left_column, right_column, predicate, join_type, right_prefix } => Node::SpatialJoin {
                    left: b(left),
                    right: b(right),
                    left_column: left_column.clone(),
                    right_column: right_column.clone(),
                    predicate: *predicate,
                    join_type: *join_type,
                    right_prefix: right_prefix.clone(),
                },
                PlanNode::Aggregate { input, group_by, aggregates } => {
                    Node::Aggregate { input: b(input), group_by: group_by.clone(), aggregates: aggregates.clone() }
                }
                PlanNode::Project { input, columns } => Node::Project { input: b(input), columns: columns.clone() },
                PlanNode::Limit { input, n } => Node::Limit { input: b(input), n: *n },
                PlanNode::Sort { input, keys } => Node::Sort { input: b(input), keys: keys.clone() },
            }
        }
        Ok(go(self, &self.root))
    }

    pub fn node(&self, id: &NodeId) -> Option<&PlanNode> {
        self.nodes.get(id)
    }

    pub fn root_node(&self) -> Option<&PlanNode> {
        self.nodes.get(&self.root)
    }

    /// Structural checks: root exists, no missing references, no cycles,
    /// no orphan (unreachable) nodes.
    pub fn check_structure(&self) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&self.root) {
            return Err(GraphError::RootMissing(self.root.clone()));
        }
        for (id, n) in &self.nodes {
            for i in n.inputs() {
                if !self.nodes.contains_key(i) {
                    return Err(GraphError::MissingNode { from: id.clone(), missing: i.clone() });
                }
            }
        }
        let order = self.topological_order()?;
        if order.len() != self.nodes.len() {
            let reach: BTreeSet<&NodeId> = order.iter().collect();
            let orphan = self.nodes.keys().find(|k| !reach.contains(k)).expect("count mismatch implies an orphan");
            return Err(GraphError::Orphan(orphan.clone()));
        }
        Ok(())
    }

    /// Nodes reachable from the root, inputs before consumers (deterministic:
    /// post-order DFS following input order). Detects cycles. Requires that
    /// referenced nodes exist (missing references are reported as errors).
    pub fn topological_order(&self) -> Result<Vec<NodeId>, GraphError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Visiting,
            Done,
        }
        let mut marks: HashMap<&NodeId, Mark> = HashMap::new();
        let mut out: Vec<NodeId> = Vec::new();
        // iterative DFS (plans can be deep)
        let root = self.nodes.get_key_value(&self.root).ok_or_else(|| GraphError::RootMissing(self.root.clone()))?.0;
        let mut stack: Vec<(&NodeId, usize)> = vec![(root, 0)];
        marks.insert(root, Mark::Visiting);
        while let Some((id, next)) = stack.pop() {
            let node = &self.nodes[id];
            let inputs = node.inputs();
            if next < inputs.len() {
                stack.push((id, next + 1));
                let child_id = inputs[next];
                let (child_key, _) = self
                    .nodes
                    .get_key_value(child_id)
                    .ok_or_else(|| GraphError::MissingNode { from: id.clone(), missing: child_id.clone() })?;
                match marks.get(child_key) {
                    Some(Mark::Visiting) => return Err(GraphError::Cycle(child_key.clone())),
                    Some(Mark::Done) => {}
                    None => {
                        marks.insert(child_key, Mark::Visiting);
                        stack.push((child_key, 0));
                    }
                }
            } else {
                marks.insert(id, Mark::Done);
                out.push(id.clone());
            }
        }
        Ok(out)
    }

    /// Output schema of every node (computed once per node, in topological order).
    pub fn schemas(&self) -> Result<BTreeMap<NodeId, SpatialSchema>, IrError> {
        let order = self.topological_order().map_err(|e| IrError::Graph(e.to_string()))?;
        let mut out: BTreeMap<NodeId, SpatialSchema> = BTreeMap::new();
        for id in order {
            let node = &self.nodes[&id];
            let ins: Vec<&SpatialSchema> = node.inputs().iter().map(|i| &out[*i]).collect();
            let s = crate::schema::derive_schema(node, &ins)?;
            out.insert(id, s);
        }
        Ok(out)
    }

    pub fn output_schema(&self) -> Result<SpatialSchema, IrError> {
        let mut all = self.schemas()?;
        Ok(all.remove(&self.root).expect("root is in the topological order"))
    }

    /// Content hash of one node, independent of how ids are spelled.
    fn canonical_hashes(&self) -> Result<BTreeMap<NodeId, String>, GraphError> {
        let order = self.topological_order()?;
        let mut out: BTreeMap<NodeId, String> = BTreeMap::new();
        for id in order {
            let node = &self.nodes[&id];
            let ins: Vec<NodeId> = node.inputs().iter().map(|i| NodeId(out[*i].clone())).collect();
            let canon = node.with_inputs(&ins);
            let mut h = Sha256::new();
            h.update(IR_VERSION.as_bytes());
            h.update(serde_json::to_vec(&canon).expect("IR is always serializable"));
            out.insert(id, h.finalize().iter().map(|b| format!("{b:02x}")).collect());
        }
        Ok(out)
    }

    /// Content hash of the whole plan (root's canonical hash). Equal plans have
    /// equal ids even if node ids are spelled differently.
    pub fn plan_id(&self) -> String {
        self.canonical_hashes().ok().and_then(|mut m| m.remove(&self.root)).unwrap_or_else(|| "invalid-plan".into())
    }

    /// Sources first: (canonical node hash, operation) in topological order.
    pub fn lineage(&self) -> Vec<(String, OpKind)> {
        let Ok(hashes) = self.canonical_hashes() else { return vec![] };
        let Ok(order) = self.topological_order() else { return vec![] };
        order.iter().map(|id| (hashes[id].clone(), self.nodes[id].kind())).collect()
    }

    /// Operation kinds required to execute this plan (for capability matching).
    pub fn required_ops(&self) -> BTreeSet<OpKind> {
        self.nodes.values().map(|n| n.kind()).collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("IR is always serializable")
    }

    /// Parse only (no semantic validation). Accepts IR 0.2 (graph) and IR 0.1
    /// (tree, migrated deterministically). Other versions are rejected.
    pub fn from_json(s: &str) -> Result<Plan, IrError> {
        let v: serde_json::Value = serde_json::from_str(s).map_err(|e| IrError::Parse(e.to_string()))?;
        match v.get("ir_version").and_then(|x| x.as_str()) {
            Some(IR_VERSION) => serde_json::from_value(v).map_err(|e| IrError::Parse(e.to_string())),
            Some(IR_VERSION_TREE) => migrate_v0_1(s),
            Some(other) => Err(IrError::UnsupportedVersion {
                found: other.to_string(),
                supported: format!("{IR_VERSION}, {IR_VERSION_TREE} (migrated)"),
            }),
            None => Err(IrError::Parse("missing ir_version".into())),
        }
    }
}

/// IR 0.1 (tree) -> IR 0.2 (graph). Deterministic: ids are content hashes, equal
/// sub-trees become one shared node. Semantics are unchanged (the tests compare
/// the expanded tree and the schemas).
pub fn migrate_v0_1(json: &str) -> Result<Plan, IrError> {
    let old: TreePlanV01 = serde_json::from_str(json).map_err(|e| IrError::Parse(e.to_string()))?;
    if old.ir_version != IR_VERSION_TREE {
        return Err(IrError::UnsupportedVersion { found: old.ir_version, supported: IR_VERSION_TREE.into() });
    }
    let mut p = Plan::from_tree(&old.root);
    p.hints = old.hints;
    Ok(p)
}
