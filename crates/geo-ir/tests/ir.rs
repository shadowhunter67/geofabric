use geo_ir::*;
use geo_types::*;

fn schema(crs: Crs) -> SpatialSchema {
    SpatialSchema {
        fields: vec![
            Field { name: "id".into(), data_type: FieldType::Int64 },
            Field { name: "category".into(), data_type: FieldType::Utf8 },
            Field { name: "height".into(), data_type: FieldType::Float64 },
            Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) },
        ],
        geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Polygon, crs }],
    }
}

fn source(crs: Crs) -> Node {
    Node::Source {
        uri: "buildings.parquet".into(),
        format: SourceFormat::GeoParquet,
        schema: schema(crs),
        statistics: SourceStats::default(),
    }
}

fn buffer(input: Node, d: Distance, crs: Crs) -> Node {
    Node::Buffer {
        input: Box::new(input),
        geometry_column: "geometry".into(),
        distance: d,
        input_crs: crs.clone(),
        output_crs: crs,
        error_tolerance: None,
        output_column: "geometry".into(),
    }
}

#[test]
fn roundtrip_and_version() {
    let bbox = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap();
    let plan = Plan::new(Node::Limit {
        input: Box::new(Node::BboxFilter {
            input: Box::new(source(Crs::Epsg(4326))),
            geometry_column: "geometry".into(),
            bbox,
        }),
        n: 10,
    });
    let json = plan.to_json();
    assert!(json.contains("\"ir_version\": \"0.3\""));
    assert_eq!(Plan::from_json(&json).unwrap(), plan);
    assert_eq!(plan.to_json(), json, "deterministic");
    assert!(validate(&plan).is_ok());
}

#[test]
fn rejects_bad_input() {
    assert!(matches!(Plan::from_json("{"), Err(IrError::Parse(_))));
    let plan = Plan::new(source(Crs::Epsg(4326)));
    let bad = plan.to_json().replace("\"0.3\"", "\"9.9\"");
    assert!(matches!(Plan::from_json(&bad), Err(IrError::UnsupportedVersion { .. })));
    let unknown_field = plan.to_json().replace("\"hints\"", "\"bogus\"");
    assert!(Plan::from_json(&unknown_field).is_err());
}

#[test]
fn unknown_crs_parses_but_metric_buffer_rejected() {
    let crs = Crs::Unknown("LOCAL_CS[...]".into());
    let plan = Plan::new(buffer(source(crs), Distance::new(100.0, DistanceUnit::Meter).unwrap(), Crs::Unknown("LOCAL_CS[...]".into())));
    let reparsed = Plan::from_json(&plan.to_json()).unwrap(); // parse OK
    let rep = validate(&reparsed);
    assert!(!rep.is_ok());
    assert!(rep.has_code("unknown_crs"));
}

#[test]
fn metric_buffer_on_geographic_rejected_projected_ok() {
    let d = Distance::new(100.0, DistanceUnit::Meter).unwrap();
    let geo = Plan::new(buffer(source(Crs::Epsg(4326)), d, Crs::Epsg(4326)));
    assert!(validate(&geo).has_code("metric_on_geographic"));
    let proj = Plan::new(buffer(source(Crs::Epsg(3857)), d, Crs::Epsg(3857)));
    assert!(validate(&proj).is_ok());
    // degree distance on geographic: allowed with a warning, never treated as meters
    let deg = Plan::new(buffer(source(Crs::Epsg(4326)), Distance::new(0.1, DistanceUnit::Degree).unwrap(), Crs::Epsg(4326)));
    let rep = validate(&deg);
    assert!(rep.is_ok() && rep.warnings().count() == 1);
    // unknown geographic-ness (valid EPSG code outside the v0 table) is refused, not guessed
    let odd = Plan::new(buffer(source(Crs::Epsg(2154)), d, Crs::Epsg(2154)));
    assert!(validate(&odd).has_code("crs_kind_unknown"));
}

#[test]
fn bbox_and_intersects_require_matching_crs() {
    let bb = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(3857)).unwrap();
    let plan = Plan::new(Node::BboxFilter { input: Box::new(source(Crs::Epsg(4326))), geometry_column: "geometry".into(), bbox: bb });
    assert!(validate(&plan).has_code("crs_mismatch"));
    let plan = Plan::new(Node::Intersects {
        input: Box::new(source(Crs::Epsg(4326))),
        geometry_column: "geometry".into(),
        with: GeometryLiteral { wkt: "POINT(0 0)".into(), crs: Crs::Unknown("x".into()) },
    });
    assert!(validate(&plan).has_code("unknown_crs"));
}

#[test]
fn declared_input_crs_must_match_column() {
    let d = Distance::new(1.0, DistanceUnit::Meter).unwrap();
    let plan = Plan::new(Node::Buffer {
        input: Box::new(source(Crs::Epsg(3857))),
        geometry_column: "geometry".into(),
        distance: d,
        input_crs: Crs::Epsg(32648),
        output_crs: Crs::Epsg(32648),
        error_tolerance: None,
        output_column: "geometry".into(),
    });
    assert!(validate(&plan).has_code("crs_mismatch"));
}

#[test]
fn schema_derivation_aggregate_and_project() {
    let agg = Node::Aggregate {
        input: Box::new(source(Crs::Epsg(4326))),
        group_by: vec!["category".into()],
        aggregates: vec![
            AggExpr { func: AggFunc::Count, column: None, alias: "n".into() },
            AggExpr { func: AggFunc::Avg, column: Some("height".into()), alias: "avg_h".into() },
        ],
    };
    let s = agg.output_schema().unwrap();
    assert_eq!(s.fields.len(), 3);
    assert!(s.geometry_columns.is_empty());
    assert!(validate(&Plan::new(agg)).is_ok());

    let bad = Node::Project { input: Box::new(source(Crs::Epsg(4326))), columns: vec!["nope".into()] };
    assert!(validate(&Plan::new(bad)).has_code("column_not_found"));
    let bad_sum = Node::Aggregate {
        input: Box::new(source(Crs::Epsg(4326))),
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Sum, column: Some("category".into()), alias: "s".into() }],
    };
    assert!(!validate(&Plan::new(bad_sum)).is_ok());
    let group_geom = Node::Aggregate { input: Box::new(source(Crs::Epsg(4326))), group_by: vec!["geometry".into()], aggregates: vec![] };
    assert!(!validate(&Plan::new(group_geom)).is_ok());
}

#[test]
fn spatial_join_requires_explicit_prefix_and_same_crs() {
    let mk = |rc| Node::SpatialJoin {
        left: Box::new(source(Crs::Epsg(4326))),
        right: Box::new(source(rc)),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "r_".into(),
    };
    assert!(validate(&Plan::new(mk(Crs::Epsg(4326)))).is_ok());
    assert!(validate(&Plan::new(mk(Crs::Epsg(3857)))).has_code("crs_mismatch"));
    let clash = Node::SpatialJoin {
        left: Box::new(source(Crs::Epsg(4326))),
        right: Box::new(source(Crs::Epsg(4326))),
        left_column: "geometry".into(),
        right_column: "geometry".into(),
        predicate: SpatialPredicate::Intersects,
        join_type: JoinType::Inner,
        right_prefix: "".into(),
    };
    assert!(!validate(&Plan::new(clash)).is_ok());
}

#[test]
fn node_ids_are_content_hashes_and_lineage_is_post_order() {
    let a = source(Crs::Epsg(4326));
    let b = Node::Limit { input: Box::new(a.clone()), n: 5 };
    assert_eq!(a.node_id(), a.clone().node_id());
    assert_ne!(a.node_id(), b.node_id());
    let l = b.lineage();
    assert_eq!(l.iter().map(|x| x.1).collect::<Vec<_>>(), vec![OpKind::Source, OpKind::Limit]);
    assert_eq!(Plan::new(b).required_ops().len(), 2);
}

#[test]
fn ir_json_has_no_engine_artifacts() {
    let j = Plan::new(source(Crs::Epsg(4326))).to_json().to_lowercase();
    for w in ["duckdb", "st_intersects", "select "] {
        assert!(!j.contains(w), "IR must stay engine-neutral: found {w}");
    }
}

#[test]
fn undefined_crs_rejects_metric_buffer_and_bbox() {
    let d = Distance::new(100.0, DistanceUnit::Meter).unwrap();
    let plan = Plan::new(buffer(source(Crs::Undefined), d, Crs::Undefined));
    assert!(validate(&plan).has_code("unknown_crs"), "metric op on Undefined CRS must be a validation error");
    let bb = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap();
    let plan = Plan::new(Node::BboxFilter { input: Box::new(source(Crs::Undefined)), geometry_column: "geometry".into(), bbox: bb });
    assert!(validate(&plan).has_code("unknown_crs"));
}

#[test]
fn crs84_is_known_but_never_implicitly_epsg4326() {
    let bb = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap();
    let plan = Plan::new(Node::BboxFilter { input: Box::new(source(Crs::OgcCrs84)), geometry_column: "geometry".into(), bbox: bb });
    assert!(validate(&plan).has_code("crs_mismatch"));
    let bb84 = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::OgcCrs84).unwrap();
    let plan = Plan::new(Node::BboxFilter { input: Box::new(source(Crs::OgcCrs84)), geometry_column: "geometry".into(), bbox: bb84 });
    assert!(validate(&plan).is_ok());
    // degree buffer on CRS84 is geographic -> allowed with a warning; metric is rejected
    let d = Distance::new(100.0, DistanceUnit::Meter).unwrap();
    assert!(validate(&Plan::new(buffer(source(Crs::OgcCrs84), d, Crs::OgcCrs84))).has_code("metric_on_geographic"));
}
