//! LIVE cross-engine correctness: the same PlanGraph on DuckDB and on a real PostgreSQL/PostGIS, through the real runtime and planner,
//! on the same data. Runs only with `--features postgis-live` and the environment below; otherwise it prints SKIP (a skip is NOT evidence).
//!
//!   GEOFABRIC_POSTGIS_ENABLED=1  GEOFABRIC_POSTGIS_URL=postgres://...  GEOFABRIC_POSTGIS_SOURCES=<catalog json>
//!   GF_XE_BUILDINGS=<logical uri>  GF_XE_ROADS=<logical uri>  GF_XE_ADMIN=<logical uri>   (all three must be resident in PostGIS)
//!   GF_XE_BBOX=x0,y0,x1,y1  (the window every plan uses)   GF_XE_TEXT_COL=class  GF_XE_NUM_COL=height
//!   GF_XE_OUT=<path>  (optional: JSON evidence)
//!
//! What "the same result" means here: same columns, same row count, same ids, same aggregates (floats within 1e-9 relative), the same
//! order where the plan defines one, and geometries equal COORDINATE BY COORDINATE after parsing the WKB (1e-9 degrees; 1e-6 m for
//! projected coordinates) - never "the bytes happened to match". The canonical digest equality is reported as information.
#![cfg(feature = "postgis-live")]

use arrow_array::cast::AsArray;
use arrow_array::{Array, RecordBatch};
use arrow_schema::DataType;
use geo_engine_duckdb::{DuckDbConfig, DuckDbEngine};
use geo_executor::{result_digest, ExecutionContext};
use geo_ir::*;
use geo_runtime::{Dispatch, Runtime};
use geo_types::*;

#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    /// WKB parsed to a flat list: [type, n, coords...] (recursive for rings/parts); compared coordinate by coordinate
    Geom(Vec<f64>),
}

fn wkb(b: &[u8], pos: &mut usize, out: &mut Vec<f64>) {
    let little = b[*pos] == 1;
    *pos += 1;
    let rd32 = |p: &mut usize| {
        let v = if little { u32::from_le_bytes(b[*p..*p + 4].try_into().unwrap()) } else { u32::from_be_bytes(b[*p..*p + 4].try_into().unwrap()) };
        *p += 4;
        v
    };
    let rd64 = |p: &mut usize| {
        let v = if little { f64::from_le_bytes(b[*p..*p + 8].try_into().unwrap()) } else { f64::from_be_bytes(b[*p..*p + 8].try_into().unwrap()) };
        *p += 8;
        v
    };
    let t = rd32(pos) & 0xff; // OGC 2D types 1..7 (ISO flags are not produced by either engine for 2D data)
    out.push(t as f64);
    match t {
        1 => {
            out.push(rd64(pos));
            out.push(rd64(pos));
        }
        2 => {
            let n = rd32(pos);
            out.push(n as f64);
            for _ in 0..n * 2 {
                out.push(rd64(pos));
            }
        }
        3 => {
            let rings = rd32(pos);
            out.push(rings as f64);
            for _ in 0..rings {
                let n = rd32(pos);
                out.push(n as f64);
                for _ in 0..n * 2 {
                    out.push(rd64(pos));
                }
            }
        }
        4..=7 => {
            let parts = rd32(pos);
            out.push(parts as f64);
            for _ in 0..parts {
                wkb(b, pos, out);
            }
        }
        other => panic!("WKB geometry type {other} is not handled by the comparator"),
    }
}

fn rows_of(batches: &[RecordBatch]) -> (Vec<String>, Vec<Vec<Cell>>) {
    let mut names = Vec::new();
    let mut rows = Vec::new();
    for b in batches {
        let schema = b.schema();
        if names.is_empty() {
            names = schema.fields().iter().map(|f| f.name().clone()).collect();
        }
        for r in 0..b.num_rows() {
            let mut row = Vec::new();
            for (ci, col) in b.columns().iter().enumerate() {
                let field = schema.field(ci);
                row.push(if col.is_null(r) {
                    Cell::Null
                } else {
                    match col.data_type() {
                        DataType::Int64 => Cell::Int(col.as_primitive::<arrow_array::types::Int64Type>().value(r)),
                        DataType::Int32 => Cell::Int(col.as_primitive::<arrow_array::types::Int32Type>().value(r) as i64),
                        DataType::Float64 => Cell::Float(col.as_primitive::<arrow_array::types::Float64Type>().value(r)),
                        DataType::Float32 => Cell::Float(col.as_primitive::<arrow_array::types::Float32Type>().value(r) as f64),
                        DataType::Boolean => Cell::Bool(col.as_boolean().value(r)),
                        DataType::Utf8 => Cell::Text(col.as_string::<i32>().value(r).to_string()),
                        DataType::Binary => {
                            assert_eq!(field.metadata().get("ARROW:extension:name").map(String::as_str), Some("geoarrow.wkb"), "binary column {} is not tagged as WKB", field.name());
                            let b = col.as_binary::<i32>().value(r);
                            let (mut out, mut pos) = (Vec::new(), 0usize);
                            wkb(b, &mut pos, &mut out);
                            Cell::Geom(out)
                        }
                        other => panic!("unhandled column type {other}"),
                    }
                });
            }
            rows.push(row);
        }
    }
    (names, rows)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * a.abs().max(b.abs()).max(1.0)
}

fn cell_eq(a: &Cell, b: &Cell, geom_tol: f64) -> bool {
    match (a, b) {
        (Cell::Float(x), Cell::Float(y)) => close(*x, *y, 1e-9),
        (Cell::Geom(x), Cell::Geom(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (p - q).abs() <= geom_tol),
        _ => a == b,
    }
}

struct Env {
    rt: Runtime,
    ctx: ExecutionContext,
    bld: Node,
    rds: Node,
    adm: Node,
    bbox: [f64; 4],
    text: String,
    num: String,
}

fn env_or_skip() -> Option<Env> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(b), Some(r), Some(a), Some(bb)) = (get("GF_XE_BUILDINGS"), get("GF_XE_ROADS"), get("GF_XE_ADMIN"), get("GF_XE_BBOX")) else {
        eprintln!("SKIP: GF_XE_* / GEOFABRIC_POSTGIS_* are not set (no live database): this test proved nothing");
        return None;
    };
    let mut rt = Runtime::empty();
    assert!(rt.register_postgis_from_env().expect("postgis configuration"), "PostGIS is not enabled in the environment");
    rt.register(Box::new(DuckDbEngine::new(DuckDbConfig::default())));
    rt.probe_postgis();
    let status = rt.postgis_status();
    assert_eq!(status["probe"]["ok"], true, "the live probe must pass first: {status}");
    let roots: Vec<std::path::PathBuf> = [&b, &r, &a].iter().map(|u| std::path::Path::new(u.as_str()).parent().unwrap().to_path_buf()).collect();
    let mut ctx = ExecutionContext::new(roots);
    ctx.timeout = std::time::Duration::from_secs(240); // a correctness check must finish; the durations are recorded, not judged
    let n: Vec<f64> = bb.split(',').map(|v| v.trim().parse().unwrap()).collect();
    Some(Env {
        bld: rt.open_source(&b, &ctx).unwrap(),
        rds: rt.open_source(&r, &ctx).unwrap(),
        adm: rt.open_source(&a, &ctx).unwrap(),
        bbox: [n[0], n[1], n[2], n[3]],
        text: get("GF_XE_TEXT_COL").unwrap_or_else(|| "class".into()),
        num: get("GF_XE_NUM_COL").unwrap_or_else(|| "height".into()),
        rt,
        ctx,
    })
}

struct Case {
    name: &'static str,
    plan: Plan,
    /// the plan defines a total order: compare in order; otherwise sort both sides by the first column
    ordered: bool,
    /// compare only the row count (an unordered LIMIT: which rows come back is undefined)
    count_only: bool,
    geom_tol: f64,
}

fn win(e: &Env) -> BoundingBox {
    BoundingBox::new(e.bbox[0], e.bbox[1], e.bbox[2], e.bbox[3], Crs::OgcCrs84).unwrap()
}

fn windowed(e: &Env, src: &Node) -> Node {
    Node::BboxFilter { input: Box::new(src.clone()), geometry_column: "geometry".into(), bbox: win(e) }
}

fn sort_id(input: Node) -> Node {
    Node::Sort { input: Box::new(input), keys: vec![SortKey { column: "id".into(), descending: false }] }
}

fn project(input: Node, cols: &[&str]) -> Node {
    Node::Project { input: Box::new(input), columns: cols.iter().map(|s| s.to_string()).collect() }
}

fn metric(e: &Env, src: &Node) -> Node {
    Node::Transform { input: Box::new(windowed(e, src)), geometry_column: "geometry".into(), input_crs: Crs::OgcCrs84, output_crs: Crs::Epsg(32648), output_column: "geometry_metric".into() }
}

fn cases(e: &Env) -> Vec<Case> {
    let (t, n) = (e.text.as_str(), e.num.as_str());
    let base = |node: Node| Plan::new(node);
    let eq = |c: &str, v: Literal| Expr::Compare { column: c.into(), op: CompareOp::Eq, value: v };
    let cols = ["id", t, n, "geometry"];
    let w = windowed(e, &e.bld);
    let first_text = "school"; // present in the fixture and in the real data
    let in_values: Vec<String> = std::env::var("GF_XE_IN").unwrap_or_else(|_| "house,shop".into()).split(',').map(str::to_string).collect();
    let mut v = vec![
        Case { name: "limit_unordered", plan: base(Node::Limit { input: Box::new(project(w.clone(), &cols)), n: 20 }), ordered: false, count_only: true, geom_tol: 1e-9 },
        Case { name: "bbox_sort_limit", plan: base(Node::Limit { input: Box::new(sort_id(project(w.clone(), &cols))), n: 500 }), ordered: true, count_only: false, geom_tol: 1e-9 },
        Case { name: "filter_eq_text", plan: base(sort_id(project(Node::Filter { input: Box::new(w.clone()), predicate: eq(t, Literal::Str(first_text.into())) }, &cols))), ordered: true, count_only: false, geom_tol: 1e-9 },
        Case {
            name: "filter_in_or_isnull",
            plan: base(sort_id(project(
                Node::Filter { input: Box::new(w.clone()), predicate: Expr::Or { exprs: vec![Expr::In { column: t.into(), values: in_values.iter().map(|v| Literal::Str(v.clone())).collect() }, Expr::IsNull { column: n.into() }] } },
                &cols,
            ))),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "filter_numeric_and_not",
            plan: base(sort_id(project(
                Node::Filter { input: Box::new(w.clone()), predicate: Expr::And { exprs: vec![Expr::Compare { column: n.into(), op: CompareOp::Gt, value: Literal::Float(9.0) }, Expr::Not { expr: Box::new(Expr::IsNull { column: t.into() }) }] } },
                &cols,
            ))),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "sort_desc_nulls_then_id",
            plan: base(Node::Limit {
                input: Box::new(Node::Sort { input: Box::new(project(w.clone(), &cols)), keys: vec![SortKey { column: n.into(), descending: true }, SortKey { column: "id".into(), descending: false }] }),
                n: 40,
            }),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "sort_text_asc_then_id",
            plan: base(Node::Limit {
                input: Box::new(Node::Sort { input: Box::new(project(w.clone(), &cols)), keys: vec![SortKey { column: t.into(), descending: false }, SortKey { column: "id".into(), descending: false }] }),
                n: 40,
            }),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "aggregate_count_star",
            plan: base(Node::Aggregate { input: Box::new(w.clone()), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }] }),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "count_by_with_ties_and_null_group",
            plan: base(Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate { input: Box::new(w.clone()), group_by: vec![t.into()], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }] }),
                    keys: vec![SortKey { column: "count".into(), descending: true }, SortKey { column: t.into(), descending: false }],
                }),
                n: 50,
            }),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "aggregate_min_max_avg_count_by_group",
            plan: base(Node::Sort {
                input: Box::new(Node::Aggregate {
                    input: Box::new(w.clone()),
                    group_by: vec![t.into()],
                    aggregates: vec![
                        AggExpr { func: AggFunc::Min, column: Some(n.into()), alias: "lo".into() },
                        AggExpr { func: AggFunc::Max, column: Some(n.into()), alias: "hi".into() },
                        AggExpr { func: AggFunc::Avg, column: Some(n.into()), alias: "mean".into() },
                        AggExpr { func: AggFunc::Count, column: Some(n.into()), alias: "known".into() },
                    ],
                }),
                keys: vec![SortKey { column: t.into(), descending: false }],
            }),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "intersects_polygon",
            plan: base(sort_id(project(
                Node::Intersects {
                    input: Box::new(w.clone()),
                    geometry_column: "geometry".into(),
                    with: GeometryLiteral {
                        wkt: format!(
                            "POLYGON(({a} {b}, {c} {b}, {c} {d}, {a} {d}, {a} {b}))",
                            a = e.bbox[0] + (e.bbox[2] - e.bbox[0]) * 0.2, b = e.bbox[1] + (e.bbox[3] - e.bbox[1]) * 0.2,
                            c = e.bbox[0] + (e.bbox[2] - e.bbox[0]) * 0.7, d = e.bbox[1] + (e.bbox[3] - e.bbox[1]) * 0.7
                        ),
                        crs: Crs::OgcCrs84,
                    },
                },
                &cols,
            ))),
            ordered: true, count_only: false, geom_tol: 1e-9,
        },
        Case {
            name: "transform_to_utm48_column",
            plan: base(sort_id(project(
                Node::Transform { input: Box::new(w.clone()), geometry_column: "geometry".into(), input_crs: Crs::OgcCrs84, output_crs: Crs::Epsg(32648), output_column: "geometry_metric".into() },
                &["id", "geometry", "geometry_metric"],
            ))),
            ordered: true, count_only: false, geom_tol: 1e-6,
        },
    ];
    // metric semi-join: buildings within 200 m of roads, in the plane of UTM 48N; each building once
    let semi = |dist: f64| Node::SpatialSemiJoin {
        left: Box::new(metric(e, &e.bld)),
        right: Box::new(metric(e, &e.rds)),
        left_column: "geometry_metric".into(),
        right_column: "geometry_metric".into(),
        predicate: SpatialPredicate::WithinDistance { distance: Distance::new(dist, DistanceUnit::Meter).unwrap() },
    };
    v.push(Case { name: "semi_join_metric_200m", plan: base(sort_id(project(semi(200.0), &["id", t, "geometry"]))), ordered: true, count_only: false, geom_tol: 1e-9 });
    v.push(Case { name: "semi_join_metric_50m", plan: base(sort_id(project(semi(50.0), &["id", t, "geometry"]))), ordered: true, count_only: false, geom_tol: 1e-9 });
    // Demo B EXACTLY as the gateway template builds it (templates.rs, "near" / "near_count"): the left window, a metric COPY of the geometry,
    // the right side = the window grown by the distance (degrees, conservative), the main-road class filter, Project to the geometry,
    // then Transform REPLACING the geometry column. The count variant and the ids variant of the same question.
    {
        let dist = 200.0;
        let lat_max = e.bbox[3].abs().max(e.bbox[1].abs()).min(89.0) + dist / 110_574.0;
        let dlat = dist / 110_574.0 * 1.02;
        let dlon = dist / (111_320.0 * lat_max.to_radians().cos().max(0.01)) * 1.02;
        let expanded = BoundingBox::new(e.bbox[0] - dlon, e.bbox[1] - dlat, e.bbox[2] + dlon, e.bbox[3] + dlat, Crs::OgcCrs84).unwrap();
        let classes: Vec<Literal> = std::env::var("GF_XE_ROAD_CLASSES").unwrap_or_else(|_| "motorway,trunk,primary".into()).split(',').map(|c| Literal::Str(c.to_string())).collect();
        let left = Node::Transform { input: Box::new(w.clone()), geometry_column: "geometry".into(), input_crs: Crs::OgcCrs84, output_crs: Crs::Epsg(32648), output_column: "geometry_metric".into() };
        let right = Node::Transform {
            input: Box::new(Node::Project {
                input: Box::new(Node::Filter {
                    input: Box::new(Node::BboxFilter { input: Box::new(e.rds.clone()), geometry_column: "geometry".into(), bbox: expanded }),
                    predicate: Expr::In { column: "class".into(), values: classes },
                }),
                columns: vec!["geometry".into()],
            }),
            geometry_column: "geometry".into(),
            input_crs: Crs::OgcCrs84,
            output_crs: Crs::Epsg(32648),
            output_column: "geometry".into(),
        };
        let semi = Node::SpatialSemiJoin {
            left: Box::new(left),
            right: Box::new(right),
            left_column: "geometry_metric".into(),
            right_column: "geometry".into(),
            predicate: SpatialPredicate::WithinDistance { distance: Distance::new(dist, DistanceUnit::Meter).unwrap() },
        };
        v.push(Case {
            name: "demo_b_count_template_shape",
            plan: base(Node::Aggregate { input: Box::new(semi.clone()), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }] }),
            ordered: true, count_only: false, geom_tol: 1e-9,
        });
        v.push(Case { name: "demo_b_ids_template_shape", plan: base(sort_id(project(semi, &["id", "geometry"]))), ordered: true, count_only: false, geom_tol: 1e-9 });
    }
    v.push(Case {
        name: "semi_join_intersects_admin",
        plan: base(sort_id(project(
            Node::SpatialSemiJoin { left: Box::new(w.clone()), right: Box::new(windowed(e, &e.adm)), left_column: "geometry".into(), right_column: "geometry".into(), predicate: SpatialPredicate::Intersects },
            &["id", t, "geometry"],
        ))),
        ordered: true, count_only: false, geom_tol: 1e-9,
    });
    v
}

fn run(e: &Env, plan: &Plan, engine: &str) -> (Vec<RecordBatch>, f64) {
    let mut p = plan.clone();
    p.hints.preferred_engine = Some(engine.into());
    let t0 = std::time::Instant::now();
    let dispatched = e.rt.dispatch(&p, &e.ctx).unwrap_or_else(|err| panic!("{engine}: {err}"));
    let wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
    match dispatched {
        Dispatch::Executed(out) => {
            let why: Vec<String> = out.physical.candidates.iter().map(|c| format!("{}: feasible={} {}", c.engine, c.feasible, c.rejected_because.join("; "))).collect();
            assert_eq!(out.physical.engine.0, engine, "the planner did not run the requested engine. candidates: {why:?}; policy: {:?}", out.physical.assessment.policy);
            (out.result.batches, wall_ms)
        }
        Dispatch::Client { .. } => panic!("unexpected client dispatch"),
    }
}

/// GF_XE_DUMP=<path>: write the lowered statement and its typed parameters of every case (for EXPLAIN and benchmarks run by hand).
fn dump_sql(e: &Env) {
    let Ok(path) = std::env::var("GF_XE_DUMP") else { return };
    let cfg = geo_engine_postgis::PostGisConfig::from_env().expect("config");
    let catalog = geo_engine_postgis::PostGisCatalog::new(cfg.sources).expect("catalog");
    let mut out = Vec::new();
    for c in cases(e) {
        match geo_engine_postgis::lower(&c.plan, &catalog, 1_000_000) {
            Ok(l) => {
                let params: Vec<serde_json::Value> = l
                    .params
                    .iter()
                    .map(|p| match p {
                        geo_engine_postgis::Param::Int(v) => serde_json::json!({"t": "bigint", "v": v}),
                        geo_engine_postgis::Param::Float(v) => serde_json::json!({"t": "double precision", "v": v}),
                        geo_engine_postgis::Param::Text(v) => serde_json::json!({"t": "text", "v": v}),
                        geo_engine_postgis::Param::Bool(v) => serde_json::json!({"t": "boolean", "v": v}),
                    })
                    .collect();
                out.push(serde_json::json!({"case": c.name, "sql": l.sql, "params": params}));
            }
            Err(err) => out.push(serde_json::json!({"case": c.name, "error": err.to_string()})),
        }
    }
    std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
}

#[test]
fn the_same_plan_gives_the_same_result_on_duckdb_and_postgis() {
    let Some(e) = env_or_skip() else { return };
    dump_sql(&e);
    let mut evidence = Vec::new();
    let mut failures = Vec::new();
    let skip: Vec<String> = std::env::var("GF_XE_SKIP").unwrap_or_default().split(',').map(str::to_string).collect();
    for c in cases(&e).into_iter().filter(|c| !skip.iter().any(|s| s == c.name)) {
        let ((da, ms_a), (db, ms_b)) = (run(&e, &c.plan, "duckdb"), run(&e, &c.plan, "postgis"));
        let (names_a, mut ra) = rows_of(&da);
        let (names_b, mut rb) = rows_of(&db);
        let (digest_a, digest_b) = (result_digest(&da), result_digest(&db));
        let mut problems = Vec::new();
        // Demo B: the PostGIS id set must equal the INDEPENDENT oracle's (shapely + pyproj), compared by hash, not through DuckDB
        let mut oracle_sha: Option<bool> = None;
        if c.name == "demo_b_ids_template_shape" {
            if let Ok(want) = std::env::var("GF_XE_ORACLE_SHA") {
                use sha2::{Digest, Sha256};
                let mut ids: Vec<String> = rb.iter().map(|r| match &r[0] { Cell::Text(t) => t.clone(), Cell::Int(i) => i.to_string(), other => format!("{other:?}") }).collect();
                ids.sort();
                let got: String = Sha256::digest(ids.join("\n").as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
                oracle_sha = Some(got == want);
                if got != want {
                    problems.push(format!("PostGIS Demo B id set hash {got} differs from the independent oracle {want}"));
                }
            }
        }
        if names_a != names_b && !(ra.is_empty() && rb.is_empty()) {
            problems.push(format!("columns differ: {names_a:?} vs {names_b:?}"));
        }
        if ra.len() != rb.len() {
            problems.push(format!("row counts differ: duckdb {} vs postgis {}", ra.len(), rb.len()));
        } else if !c.count_only {
            if !c.ordered {
                let key = |r: &Vec<Cell>| format!("{:?}", r.first());
                ra.sort_by_key(key);
                rb.sort_by_key(key);
            }
            for (i, (x, y)) in ra.iter().zip(&rb).enumerate() {
                if x.len() != y.len() || !x.iter().zip(y).all(|(p, q)| cell_eq(p, q, c.geom_tol)) {
                    problems.push(format!("row {i} differs:\n  duckdb  {x:?}\n  postgis {y:?}"));
                    break;
                }
            }
        }
        eprintln!("{:<42} rows={:<6} {}  digest_equal={}  duckdb={:.0} ms  postgis={:.0} ms", c.name, ra.len(), if problems.is_empty() { "IDENTICAL" } else { "MISMATCH" }, digest_a == digest_b, ms_a, ms_b);
        for p in &problems {
            eprintln!("    problem: {}", p.chars().take(900).collect::<String>());
        }
        evidence.push(serde_json::json!({"case": c.name, "rows_duckdb": ra.len(), "rows_postgis": rb.len(), "identical": problems.is_empty(), "oracle_ids_sha_match": oracle_sha, "canonical_digest_equal": digest_a == digest_b, "wall_ms_duckdb": ms_a, "wall_ms_postgis": ms_b, "digest_duckdb": digest_a.1, "digest_postgis": digest_b.1, "problems": problems}));
        if !problems.is_empty() {
            failures.push(format!("{}: {}", c.name, problems.join(" | ")));
        }
    }
    if let Ok(path) = std::env::var("GF_XE_OUT") {
        std::fs::write(path, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    }
    assert!(failures.is_empty(), "cross-engine MISMATCH (stop, fix correctness, re-run):\n{}", failures.join("\n"));
}


/// Benchmark (GF_BENCH=1): the same plans on both engines, FIRST execution and WARM repetitions, correctness checked on every run's row
/// count. Definitions (stated, not claimed to be more than they are):
///   first = the first execution of the case in this process. For PostGIS the server is restarted just before the process starts, so
///           its shared buffers are cold (the OS file cache is NOT cleared and cannot be on this machine: "cold database cache, warm OS
///           cache"); DuckDB's first run includes its per-process state (extension, file metadata). Later cases benefit from tables
///           earlier cases warmed, so `first` is "first touch after restart", not "from disk".
///   warm  = median of the repetitions that follow (5 when the first run took < 2 s, 3 when < 10 s, else 1).
/// Wall clock around `Runtime::dispatch`: plan + authorization + execution + Arrow result, the same for both engines.
#[test]
fn benchmark_duckdb_vs_postgis() {
    if std::env::var("GF_BENCH").ok().as_deref() != Some("1") {
        eprintln!("SKIP: set GF_BENCH=1 to run the benchmark");
        return;
    }
    let Some(e) = env_or_skip() else { return };
    let skip: Vec<String> = std::env::var("GF_XE_SKIP").unwrap_or_default().split(',').map(str::to_string).collect();
    let mut out = Vec::new();
    for c in cases(&e).into_iter().filter(|c| !skip.iter().any(|s| s == c.name)) {
        let mut one = serde_json::json!({"case": c.name});
        let mut rows_seen = Vec::new();
        for engine in ["duckdb", "postgis"] {
            let (b, first) = run(&e, &c.plan, engine);
            let rows: usize = b.iter().map(|x| x.num_rows()).sum();
            rows_seen.push(rows);
            let reps = if first < 2000.0 { 5 } else if first < 10_000.0 { 3 } else { 1 };
            let mut warm = Vec::new();
            for _ in 0..reps {
                let (b, ms) = run(&e, &c.plan, engine);
                assert_eq!(b.iter().map(|x| x.num_rows()).sum::<usize>(), rows, "{} {engine}: row count changed between runs", c.name);
                warm.push(ms);
            }
            warm.sort_by(|a, b| a.partial_cmp(b).unwrap());
            one[engine] = serde_json::json!({"first_ms": first, "warm_ms": warm, "warm_median_ms": warm[warm.len() / 2], "rows": rows});
        }
        assert!(c.count_only || rows_seen[0] == rows_seen[1], "{}: engines disagree on the row count", c.name);
        eprintln!("{:<42} first d={:.0} p={:.0} | warm d={:.0} p={:.0} ms", c.name, one["duckdb"]["first_ms"].as_f64().unwrap(), one["postgis"]["first_ms"].as_f64().unwrap(), one["duckdb"]["warm_median_ms"].as_f64().unwrap(), one["postgis"]["warm_median_ms"].as_f64().unwrap());
        out.push(one);
    }
    if let Ok(path) = std::env::var("GF_BENCH_OUT") {
        std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }
}
