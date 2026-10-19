//! IR 0.3 additions: SpatialSemiJoin, Transform, WithinDistance, HashBucket: validation, stable serialization, topological
//! behaviour, schema, version compatibility (0.2 plans are read unchanged).

use geo_ir::*;
use geo_types::*;

fn schema(crs: Crs) -> SpatialSchema {
    SpatialSchema {
        fields: vec![
            Field { name: "id".into(), data_type: FieldType::Int64 },
            Field { name: "name".into(), data_type: FieldType::Utf8 },
            Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) },
        ],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Polygon, crs }],
    }
}

fn src(b: &mut GraphBuilder, uri: &str, crs: Crs) -> NodeId {
    b.add(PlanNode::Source { uri: uri.into(), format: SourceFormat::GeoParquet, schema: schema(crs), statistics: SourceStats::default() })
}

fn within(m: f64, unit: DistanceUnit) -> SpatialPredicate {
    SpatialPredicate::WithinDistance { distance: Distance::new(m, unit).unwrap() }
}

fn semi(l: NodeId, r: NodeId, lc: &str, p: SpatialPredicate, b: &mut GraphBuilder) -> NodeId {
    b.add(PlanNode::SpatialSemiJoin { left: l, right: r, left_column: lc.into(), right_column: "geometry".into(), predicate: p })
}

fn metric_plan() -> Plan {
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "buildings.parquet", Crs::OgcCrs84);
    let r = src(&mut b, "roads.parquet", Crs::OgcCrs84);
    let lt = b.add(PlanNode::Transform {
        input: l,
        geometry_column: "geometry".into(),
        input_crs: Crs::OgcCrs84,
        output_crs: Crs::Epsg(32648),
        output_column: "geometry_metric".into(),
    });
    let rt = b.add(PlanNode::Transform {
        input: r,
        geometry_column: "geometry".into(),
        input_crs: Crs::OgcCrs84,
        output_crs: Crs::Epsg(32648),
        output_column: "geometry".into(),
    });
    let s = semi(lt, rt, "geometry_metric", within(200.0, DistanceUnit::Meter), &mut b);
    b.finish(s)
}

#[test]
fn the_metric_plan_is_valid_and_its_schema_is_the_left_schema_plus_the_metric_copy() {
    let plan = metric_plan();
    let report = validate(&plan);
    assert!(report.is_ok(), "{:?}", report.issues);
    let out = plan.output_schema().unwrap();
    // semi join: no right column leaks in; the metric copy is a geometry column in the metric CRS, the original stays in CRS84
    assert_eq!(out.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["id", "name", "geometry", "geometry_metric"]);
    assert_eq!(out.geometry("geometry").unwrap().crs, Crs::OgcCrs84);
    assert_eq!(out.geometry("geometry_metric").unwrap().crs, Crs::Epsg(32648));
}

#[test]
fn topological_order_puts_both_inputs_before_the_semi_join() {
    let plan = metric_plan();
    let order = plan.topological_order().unwrap();
    let root = order.last().unwrap();
    assert_eq!(plan.node(root).unwrap().kind(), OpKind::SpatialSemiJoin);
    let n_transform = order.iter().filter(|id| plan.node(id).unwrap().kind() == OpKind::Transform).count();
    assert_eq!(n_transform, 2);
    assert_eq!(plan.node(root).unwrap().inputs().len(), 2);
}

#[test]
fn serialization_is_stable_and_round_trips() {
    let a = metric_plan();
    let b = metric_plan();
    assert_eq!(a.to_json(), b.to_json());
    assert_eq!(a.plan_id(), b.plan_id());
    let back = Plan::from_json(&a.to_json()).unwrap();
    assert_eq!(back, a);
    let json = a.to_json();
    assert!(json.contains("\"ir_version\": \"0.3\""));
    assert!(json.contains("\"op\": \"spatial_semi_join\""));
    assert!(json.contains("\"within_distance\""));
    assert!(json.contains("\"unit\": \"meter\""));
    // the tree form carries the same plan
    let tree = a.to_tree().unwrap();
    assert_eq!(Plan::from_tree(&tree).plan_id(), a.plan_id());
}

#[test]
fn a_different_distance_or_unit_is_a_different_plan() {
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::Epsg(32648));
    let r = src(&mut b, "b", Crs::Epsg(32648));
    let m200 = semi(l.clone(), r.clone(), "geometry", within(200.0, DistanceUnit::Meter), &mut b);
    let p200 = b.finish(m200);
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::Epsg(32648));
    let r = src(&mut b, "b", Crs::Epsg(32648));
    let km = semi(l, r, "geometry", within(0.2, DistanceUnit::Kilometer), &mut b);
    let p_km = b.finish(km);
    assert!(validate(&p200).is_ok() && validate(&p_km).is_ok());
    assert_ne!(p200.plan_id(), p_km.plan_id(), "units are part of the plan: no silent normalisation");
}

#[test]
fn metric_predicates_are_refused_without_a_metric_crs_or_with_a_degree_distance() {
    // lon/lat columns
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::OgcCrs84);
    let r = src(&mut b, "b", Crs::OgcCrs84);
    let s = semi(l, r, "geometry", within(200.0, DistanceUnit::Meter), &mut b);
    let report = validate(&b.finish(s));
    assert!(report.issues.iter().any(|i| i.code == "not_metric_crs"), "{:?}", report.issues);
    // Web Mercator
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::Epsg(3857));
    let r = src(&mut b, "b", Crs::Epsg(3857));
    let s = semi(l, r, "geometry", within(200.0, DistanceUnit::Meter), &mut b);
    assert!(validate(&b.finish(s)).issues.iter().any(|i| i.code == "not_metric_crs"));
    // degrees
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::Epsg(32648));
    let r = src(&mut b, "b", Crs::Epsg(32648));
    let s = semi(l, r, "geometry", within(0.001, DistanceUnit::Degree), &mut b);
    assert!(validate(&b.finish(s)).issues.iter().any(|i| i.code == "angular_distance_predicate"));
    // different CRS on the two sides
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::Epsg(32648));
    let r = src(&mut b, "b", Crs::Epsg(32649));
    let s = semi(l, r, "geometry", within(200.0, DistanceUnit::Meter), &mut b);
    assert!(validate(&b.finish(s)).issues.iter().any(|i| i.code == "crs_mismatch"));
    // an unknown or undefined CRS is never guessed
    for crs in [Crs::Undefined, Crs::Unknown("custom".into())] {
        let mut b = GraphBuilder::new();
        let l = src(&mut b, "a", crs.clone());
        let r = src(&mut b, "b", crs);
        let s = semi(l, r, "geometry", SpatialPredicate::Intersects, &mut b);
        assert!(validate(&b.finish(s)).issues.iter().any(|i| i.code == "unknown_crs"));
    }
}

#[test]
fn transform_needs_the_declared_input_crs_and_known_crs() {
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::OgcCrs84);
    let t = b.add(PlanNode::Transform {
        input: l,
        geometry_column: "geometry".into(),
        input_crs: Crs::Epsg(4326), // wrong: the column is OGC:CRS84 (a different CRS: axis order)
        output_crs: Crs::Epsg(32648),
        output_column: "g2".into(),
    });
    assert!(validate(&b.finish(t)).issues.iter().any(|i| i.code == "crs_mismatch"));
    let mut b = GraphBuilder::new();
    let l = src(&mut b, "a", Crs::OgcCrs84);
    let t = b.add(PlanNode::Transform {
        input: l,
        geometry_column: "geometry".into(),
        input_crs: Crs::OgcCrs84,
        output_crs: Crs::Unknown("custom".into()),
        output_column: "g2".into(),
    });
    assert!(validate(&b.finish(t)).issues.iter().any(|i| i.code == "unknown_crs"));
}

#[test]
fn hash_bucket_validates_and_serializes() {
    let mk = |modulus: u32, remainder: u32| {
        let mut b = GraphBuilder::new();
        let s = src(&mut b, "a", Crs::Epsg(4326));
        let f = b.add(PlanNode::Filter { input: s, predicate: Expr::HashBucket { column: "id".into(), modulus, remainder } });
        b.finish(f)
    };
    assert!(validate(&mk(4, 0)).is_ok());
    assert!(validate(&mk(0, 0)).issues.iter().any(|i| i.code == "bad_hash_bucket"));
    assert!(validate(&mk(4, 4)).issues.iter().any(|i| i.code == "bad_hash_bucket"));
    let json = mk(8, 0).to_json();
    assert!(json.contains("\"kind\": \"hash_bucket\"") && json.contains("\"modulus\": 8"));
}

#[test]
fn ir_0_2_plans_are_read_unchanged() {
    let mut b = GraphBuilder::new();
    let s = src(&mut b, "a", Crs::Epsg(4326));
    let l = b.add(PlanNode::Limit { input: s, n: 3 });
    let plan = b.finish(l);
    let old = plan.to_json().replace("\"ir_version\": \"0.3\"", "\"ir_version\": \"0.2\"");
    let read = Plan::from_json(&old).unwrap();
    assert_eq!(read.ir_version, IR_VERSION);
    assert!(validate(&read).is_ok());
    assert_eq!(read.plan_id(), plan.plan_id());
    let future = plan.to_json().replace("\"ir_version\": \"0.3\"", "\"ir_version\": \"0.9\"");
    assert!(Plan::from_json(&future).is_err());
}
