//! PlanGraph (IR 0.3): DAG structure, validation, migration from 0.1 (tree); 0.2 plans are read unchanged.

use geo_ir::*;
use geo_types::*;
use std::collections::BTreeMap;

fn schema() -> SpatialSchema {
    SpatialSchema {
        fields: vec![
            Field { name: "id".into(), data_type: FieldType::Int64 },
            Field { name: "category".into(), data_type: FieldType::Utf8 },
            Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) },
        ],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Polygon, crs: Crs::Epsg(4326) }],
    }
}

fn src(b: &mut GraphBuilder, uri: &str) -> NodeId {
    b.add(PlanNode::Source { uri: uri.into(), format: SourceFormat::GeoParquet, schema: schema(), statistics: SourceStats::default() })
}

fn bbox() -> BoundingBox {
    BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap()
}

fn count(input: NodeId, b: &mut GraphBuilder) -> NodeId {
    b.add(PlanNode::Aggregate {
        input,
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
    })
}

fn join(l: NodeId, r: NodeId, b: &mut GraphBuilder) -> NodeId {
    b.add(PlanNode::SpatialJoin {
        left: l,
        right: r,
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "r_".into(),
    })
}

fn pos(order: &[NodeId], id: &NodeId) -> usize {
    order.iter().position(|x| x == id).expect("node in order")
}

#[test]
fn valid_linear_dag() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let f = b.add(PlanNode::BboxFilter { input: s.clone(), geometry_column: "geometry".into(), bbox: bbox() });
    let l = b.add(PlanNode::Limit { input: f.clone(), n: 5 });
    let plan = b.finish(l.clone());
    assert!(validate(&plan).is_ok());
    assert_eq!(plan.nodes.len(), 3);
    let order = plan.topological_order().unwrap();
    assert_eq!(order, vec![s, f, l]);
}

#[test]
fn branching_dag_with_shared_source_and_shared_intermediate() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet"); // shared source
    let f = b.add(PlanNode::BboxFilter { input: s.clone(), geometry_column: "geometry".into(), bbox: bbox() }); // shared intermediate
    let lim = b.add(PlanNode::Limit { input: f.clone(), n: 10 });
    let j = join(f.clone(), lim.clone(), &mut b); // f is consumed twice: directly and via lim
    let root = count(j, &mut b);
    let plan = b.finish(root.clone());
    assert!(validate(&plan).is_ok(), "{:?}", validate(&plan).issues);
    // 1 source, 1 bbox filter, 1 limit, 1 join, 1 aggregate: the source and the filter exist once
    assert_eq!(plan.nodes.len(), 5);
    let sources = plan.nodes.values().filter(|n| n.kind() == OpKind::Source).count();
    assert_eq!(sources, 1);
    let order = plan.topological_order().unwrap();
    assert_eq!(order.len(), 5, "each node exactly once");
    for (id, n) in &plan.nodes {
        for i in n.inputs() {
            assert!(pos(&order, i) < pos(&order, id), "input {i} must precede {id}");
        }
    }
    assert_eq!(order.last().unwrap(), &root);
    // deterministic order
    assert_eq!(order, plan.topological_order().unwrap());
}

#[test]
fn builder_collapses_equal_subplans_into_one_node() {
    let mut b = GraphBuilder::new();
    let s1 = src(&mut b, "a.parquet");
    let s2 = src(&mut b, "a.parquet");
    let s3 = src(&mut b, "other.parquet");
    assert_eq!(s1, s2, "content-addressed ids");
    assert_ne!(s1, s3);
}

#[test]
fn tree_to_graph_shares_duplicated_subtrees() {
    let mk = || Node::Source { uri: "a.parquet".into(), format: SourceFormat::GeoParquet, schema: schema(), statistics: SourceStats::default() };
    let tree = Node::Aggregate {
        input: Box::new(Node::SpatialJoin {
            left: Box::new(mk()),
            right: Box::new(mk()),
            left_column: "geometry".into(),
            right_column: "geometry".into(),
            predicate: SpatialPredicate::Intersects,
            join_type: JoinType::Inner,
            right_prefix: "r_".into(),
        }),
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
    };
    let plan = Plan::from_tree(&tree);
    assert_eq!(plan.nodes.len(), 3, "tree has 4 nodes, the DAG shares the source");
    assert_eq!(plan.to_tree().unwrap(), tree, "expanding the DAG gives the tree back");
}

#[test]
fn cycle_is_rejected() {
    let mut nodes = BTreeMap::new();
    nodes.insert(NodeId::new("a"), PlanNode::Limit { input: NodeId::new("b"), n: 1 });
    nodes.insert(NodeId::new("b"), PlanNode::Limit { input: NodeId::new("a"), n: 1 });
    let plan = Plan { ir_version: IR_VERSION.into(), nodes, root: NodeId::new("a"), hints: Hints::default() };
    assert!(matches!(plan.topological_order(), Err(GraphError::Cycle(_))));
    assert!(matches!(plan.check_structure(), Err(GraphError::Cycle(_))));
    let rep = validate(&plan);
    assert!(rep.has_code("graph_cycle"), "{:?}", rep.issues);
    // a self-loop too
    let mut nodes = BTreeMap::new();
    nodes.insert(NodeId::new("a"), PlanNode::Limit { input: NodeId::new("a"), n: 1 });
    let plan = Plan { ir_version: IR_VERSION.into(), nodes, root: NodeId::new("a"), hints: Hints::default() };
    assert!(validate(&plan).has_code("graph_cycle"));
}

#[test]
fn missing_node_reference_is_rejected() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let mut plan = b.finish(s);
    let l = NodeId::new("limit");
    plan.nodes.insert(l.clone(), PlanNode::Limit { input: NodeId::new("ghost"), n: 3 });
    plan.root = l;
    assert!(matches!(plan.check_structure(), Err(GraphError::MissingNode { .. })));
    assert!(validate(&plan).has_code("graph_missing_node"));
    // missing root
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let mut plan = b.finish(s);
    plan.root = NodeId::new("nope");
    assert!(validate(&plan).has_code("graph_root_missing"));
}

#[test]
fn orphan_node_is_detected() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let _unused = src(&mut b, "unused.parquet");
    let plan = b.finish(s);
    assert!(matches!(plan.check_structure(), Err(GraphError::Orphan(_))));
    assert!(validate(&plan).has_code("graph_orphan"));
}

#[test]
fn serialization_roundtrip_is_stable_and_versioned() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let j = join(s.clone(), s, &mut b);
    let root = count(j, &mut b);
    let plan = b.finish(root);
    let json = plan.to_json();
    assert!(json.contains("\"ir_version\": \"0.3\""));
    let back = Plan::from_json(&json).unwrap();
    assert_eq!(back, plan);
    assert_eq!(back.to_json(), json, "stable serialization");
    // unknown versions are rejected
    let bad = json.replace("\"0.3\"", "\"0.9\"");
    assert!(matches!(Plan::from_json(&bad), Err(IrError::UnsupportedVersion { .. })));
}

#[test]
fn plan_id_ignores_id_spelling_but_not_content() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let l = b.add(PlanNode::Limit { input: s, n: 5 });
    let plan = b.finish(l);

    // rename every node id
    let rename: BTreeMap<NodeId, NodeId> = plan.nodes.keys().enumerate().map(|(i, k)| (k.clone(), NodeId::new(format!("x{i}")))).collect();
    let mut nodes = BTreeMap::new();
    for (k, n) in &plan.nodes {
        let n2 = match n {
            PlanNode::Limit { input, n } => PlanNode::Limit { input: rename[input].clone(), n: *n },
            other => other.clone(),
        };
        nodes.insert(rename[k].clone(), n2);
    }
    let renamed = Plan { ir_version: IR_VERSION.into(), nodes, root: rename[&plan.root].clone(), hints: Hints::default() };
    assert_eq!(renamed.plan_id(), plan.plan_id());

    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a.parquet");
    let l = b.add(PlanNode::Limit { input: s, n: 6 });
    assert_ne!(b.finish(l).plan_id(), plan.plan_id());
}

// ---------- migration 0.1 (tree) -> 0.2 (graph) ----------

fn fixtures() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0_1");
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(p).unwrap()))
        .collect();
    v.sort();
    assert!(v.len() >= 5, "fixtures present");
    v
}

#[test]
fn old_fixtures_still_load_and_migrate_to_the_current_version() {
    for (name, json) in fixtures() {
        assert!(json.contains("\"ir_version\": \"0.1\""), "{name} is a genuine 0.1 file");
        let plan = Plan::from_json(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(plan.ir_version, IR_VERSION, "{name}");
        plan.check_structure().unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn migration_is_deterministic() {
    for (name, json) in fixtures() {
        let a = migrate_v0_1(&json).unwrap();
        let b = migrate_v0_1(&json).unwrap();
        assert_eq!(a.to_json(), b.to_json(), "{name}");
        assert_eq!(a.plan_id(), b.plan_id());
        // idempotent through 0.2 serialization
        assert_eq!(Plan::from_json(&a.to_json()).unwrap(), a, "{name}");
    }
}

#[test]
fn migration_preserves_semantics() {
    for (name, json) in fixtures() {
        let old: TreePlanV01 = serde_json::from_str(&json).unwrap();
        let plan = Plan::from_json(&json).unwrap();
        // 1. the expanded graph equals the original tree
        assert_eq!(plan.to_tree().unwrap(), old.root, "{name}: tree equivalence");
        // 2. same output schema as derived from the old tree
        assert_eq!(plan.output_schema().unwrap(), old.root.output_schema().unwrap(), "{name}: schema");
        // 3. same required operations and lineage ops (as a set / sequence of kinds)
        assert_eq!(plan.required_ops(), old.root.lineage().into_iter().map(|(_, k)| k).collect(), "{name}: ops");
        // 4. validation verdicts
        let rep = validate(&plan);
        assert!(rep.is_ok(), "{name}: {:?}", rep.issues);
    }
}

#[test]
fn self_join_fixture_shares_the_source_after_migration() {
    let (_, json) = fixtures().into_iter().find(|(n, _)| n == "self_join.json").unwrap();
    let plan = Plan::from_json(&json).unwrap();
    let sources = plan.nodes.values().filter(|n| n.kind() == OpKind::Source).count();
    assert_eq!(sources, 1, "0.1 duplicated the source; 0.2 shares it");
    assert_eq!(plan.nodes.len(), 3);
}

#[test]
fn hints_survive_migration() {
    let (_, json) = fixtures().into_iter().find(|(n, _)| n == "linear.json").unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["hints"] = serde_json::json!({"preferred_engine": "duckdb"});
    let plan = Plan::from_json(&v.to_string()).unwrap();
    assert_eq!(plan.hints.preferred_engine.as_deref(), Some("duckdb"));
}
