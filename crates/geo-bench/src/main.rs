//! Benchmark runner. Usage:
//!   cargo run --release -p geo-bench -- --data benchmarks/data --out benchmarks/results \
//!       --sizes 10mb,100mb --reps 3
//!
//! No timing thresholds are asserted: the output is for *relative* comparison
//! (pruning effectiveness, spatial vs shuffled ordering).
//!
//! Correctness gate: every run records `result_count` + `result_hash` and is
//! compared with the independent oracle output `expected-<size>.json`
//! (`benchmarks/oracle.py`: pyarrow + numpy, no DuckDB). A timing row is only
//! valid when `validation == pass`; any FAIL makes the process exit non-zero.
//! Metric provenance is part of the column names: `*_estimated` values are
//! statistics-derived bounds, not measurements; actual bytes read is unknown
//! and is not reported.

use geo_executor::{result_digest, ExecutionContext};
use geo_ir::*;
use geo_runtime::Runtime;
use geo_types::*;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct Row {
    dataset: String,
    size_bytes: u64,
    rows_in_file: u64,
    distribution: String,
    ordering: String,
    query: String,
    result_count: u64,
    result_hash: String,
    expected_count: Option<u64>,
    /// pass | FAIL | no-oracle
    validation: String,
    rows_returned_engine_observed: u64,
    row_groups_total_source_observed: Option<u64>,
    row_groups_skipped_source_estimated: Option<u64>,
    rows_scanned_upper_bound_source_estimated: Option<u64>,
    estimated_bytes_upper_bound_source_estimated: Option<u64>,
    /// Always null: no trustworthy measurement of real I/O exists.
    bytes_read: Option<u64>,
    time_ms_median_engine_observed: f64,
    time_ms_min_engine_observed: f64,
    peak_buffer_memory_bytes_engine_observed: Option<u64>,
    reps: usize,
}

fn bb(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
    BoundingBox::new(x0, y0, x1, y1, Crs::Epsg(4326)).unwrap()
}

fn bbox(src: Node, b: BoundingBox) -> Node {
    Node::BboxFilter { input: Box::new(src), geometry_column: "geometry".into(), bbox: b }
}

fn count(input: Node) -> Node {
    Node::Aggregate {
        input: Box::new(input),
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
    }
}

fn queries(src: &Node) -> Vec<(&'static str, Plan)> {
    let w1 = bb(104.0, 12.0, 105.0, 13.0); // ~0.6% of extent
    let w2 = bb(100.0, 8.0, 105.0, 16.0); // 25% of extent
    let poly = "POLYGON((104 12, 105 12, 105 13, 104 13, 104 12))";
    vec![
        ("bbox_count_small(0.6%)", Plan::new(count(bbox(src.clone(), w1.clone())))),
        ("bbox_count_large(25%)", Plan::new(count(bbox(src.clone(), w2)))),
        (
            "attr_filter_fullscan",
            Plan::new(count(Node::Filter {
                input: Box::new(src.clone()),
                predicate: Expr::And {
                    exprs: vec![
                        Expr::Compare { column: "category".into(), op: CompareOp::Eq, value: Literal::Str("school".into()) },
                        Expr::Compare { column: "height".into(), op: CompareOp::Gt, value: Literal::Float(20.0) },
                    ],
                },
            })),
        ),
        (
            "intersects_count_small",
            Plan::new(count(Node::Intersects {
                input: Box::new(src.clone()),
                geometry_column: "geometry".into(),
                with: GeometryLiteral { wkt: poly.into(), crs: Crs::Epsg(4326) },
            })),
        ),
        (
            "aggregate_by_category_small",
            Plan::new(Node::Sort {
                input: Box::new(Node::Aggregate {
                    input: Box::new(bbox(src.clone(), w1.clone())),
                    group_by: vec!["category".into()],
                    aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "building_count".into() }],
                }),
                keys: vec![SortKey { column: "building_count".into(), descending: true }],
            }),
        ),
        (
            "spatial_join_small",
            Plan::new(count(Node::SpatialJoin {
                left: Box::new(bbox(src.clone(), bb(104.0, 12.0, 104.3, 12.3))),
                right: Box::new(bbox(src.clone(), bb(104.2, 12.2, 104.5, 12.5))),
                left_column: "geometry".into(),
                right_column: "geometry".into(),
                predicate: SpatialPredicate::Intersects,
                join_type: JoinType::Inner,
                right_prefix: "r_".into(),
            })),
        ),
        (
            "return_rows_small(id,geometry)",
            Plan::new(Node::Project {
                input: Box::new(bbox(src.clone(), w1)),
                columns: vec!["id".into(), "geometry".into()],
            }),
        ),
    ]
}

fn bb84(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
    BoundingBox::new(x0, y0, x1, y1, Crs::OgcCrs84).unwrap()
}

/// Queries over real Overture buildings (CRS84, nested schema, mostly-NULL attributes).
/// Windows are around Ho Chi Minh City. Definitions must match benchmarks/oracle_overture.py.
fn overture_queries(src: &Node) -> Vec<(&'static str, Plan)> {
    let small = || bb84(106.60, 10.70, 106.80, 10.90);
    let large = || bb84(105.0, 9.5, 107.5, 11.5);
    let cnt = count;
    let bbox84 = |b: BoundingBox| Node::BboxFilter { input: Box::new(src.clone()), geometry_column: "geometry".into(), bbox: b };
    let tri = "POLYGON((106.62 10.72, 106.78 10.74, 106.70 10.88, 106.62 10.72))";
    vec![
        ("ov_bbox_count_small", Plan::new(cnt(bbox84(small())))),
        ("ov_bbox_count_large", Plan::new(cnt(bbox84(large())))),
        (
            "ov_attr_filter_fullscan",
            Plan::new(cnt(Node::Filter {
                input: Box::new(src.clone()),
                predicate: Expr::Compare { column: "version".into(), op: CompareOp::Ge, value: Literal::Int(4) },
            })),
        ),
        (
            "ov_intersects_polygon",
            Plan::new(cnt(Node::Intersects {
                input: Box::new(src.clone()),
                geometry_column: "geometry".into(),
                with: GeometryLiteral { wkt: tri.into(), crs: Crs::OgcCrs84 },
            })),
        ),
        (
            "ov_aggregate_version_small",
            Plan::new(Node::Sort {
                input: Box::new(Node::Aggregate {
                    input: Box::new(bbox84(small())),
                    group_by: vec!["version".into()],
                    aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
                }),
                keys: vec![SortKey { column: "version".into(), descending: false }],
            }),
        ),
        (
            "ov_spatial_join_small",
            Plan::new(cnt(Node::SpatialJoin {
                left: Box::new(Node::BboxFilter { input: Box::new(src.clone()), geometry_column: "geometry".into(), bbox: bb84(106.700, 10.770, 106.710, 10.780) }),
                right: Box::new(Node::BboxFilter { input: Box::new(src.clone()), geometry_column: "geometry".into(), bbox: bb84(106.705, 10.775, 106.715, 10.785) }),
                left_column: "geometry".into(),
                right_column: "geometry".into(),
                predicate: SpatialPredicate::Intersects,
                join_type: JoinType::Inner,
                right_prefix: "r_".into(),
            })),
        ),
        (
            "ov_return_rows_small(id,version)",
            Plan::new(Node::Project { input: Box::new(bbox84(bb84(106.690, 10.760, 106.730, 10.800))), columns: vec!["id".into(), "version".into()] }),
        ),
    ]
}

fn arg(args: &[String], name: &str, default: &str) -> String {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

fn main() {
    // the benchmark runs queries one at a time, so the DuckDB profiler (opt-in, see geo-engine-duckdb) is safe and gives the peak-memory metric
    std::env::set_var("GEOFABRIC_DUCKDB_PROFILE", "1");
    let args: Vec<String> = std::env::args().collect();
    let data = PathBuf::from(arg(&args, "--data", "benchmarks/data"));
    let out = PathBuf::from(arg(&args, "--out", "benchmarks/results"));
    let sizes: Vec<String> = arg(&args, "--sizes", "10mb,100mb").split(',').map(String::from).collect();
    let reps: usize = arg(&args, "--reps", "3").parse().expect("--reps");
    let tag = arg(&args, "--tag", "run");
    let suite = arg(&args, "--suite", "synthetic"); // synthetic | overture

    let rt = Runtime::with_duckdb();
    let mut ctx = ExecutionContext::new(vec![data.clone()]);
    ctx.max_rows = 5_000_000;
    ctx.memory_limit_mb = 4096;
    ctx.timeout = std::time::Duration::from_secs(600);
    let mut rows: Vec<Row> = Vec::new();
    let mut failures = 0usize;

    let mut expected: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    let expected_keys: Vec<String> = if suite == "overture" { vec!["overture".to_string()] } else { sizes.clone() };
    for size in &expected_keys {
        let p = out.join(format!("expected-{size}.json"));
        match std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            Some(serde_json::Value::Object(m)) => expected.extend(m),
            _ => eprintln!("warning: no oracle file {} (run benchmarks/oracle.py): rows will be 'no-oracle'", p.display()),
        }
    }

    // (name, distribution, ordering, size-label) for each dataset of the chosen suite
    let mut jobs: Vec<(String, String, String)> = Vec::new();
    if suite == "overture" {
        if let Ok(rd) = std::fs::read_dir(&data) {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .filter(|n| n.starts_with("overture_"))
                .collect();
            names.sort();
            for n in names {
                jobs.push((n, "real".into(), "overture-order".into()));
            }
        }
    } else {
        for size in &sizes {
            for dist in ["clustered", "uniform"] {
                for ordering in ["spatial", "shuffled"] {
                    jobs.push((format!("{dist}_{size}_{ordering}"), dist.to_string(), ordering.to_string()));
                }
            }
        }
    }
    {
        for (name, dist, ordering) in &jobs {
            {
                let (dist, ordering) = (dist.as_str(), ordering.as_str());
                let path = data.join(format!("{name}.parquet"));
                if !path.exists() {
                    eprintln!("skip {name}: missing");
                    continue;
                }
                let src = rt.open_source(path.to_str().unwrap(), &ctx).expect("open source");
                let file_size = std::fs::metadata(&path).unwrap().len();
                let n_rows = match &src {
                    Node::Source { statistics, .. } => statistics.row_count.unwrap_or(0),
                    _ => 0,
                };
                let qs = if suite == "overture" { overture_queries(&src) } else { queries(&src) };
                for (qname, plan) in qs {
                    let mut times = Vec::new();
                    let mut last = None;
                    for _ in 0..(reps + 1) {
                        // first iteration is a warm-up (OS file cache) and is discarded
                        match rt.run(&plan, &ctx) {
                            Ok(o) => {
                                times.push(o.result.metrics.elapsed_ms.value.unwrap_or(f64::NAN));
                                last = Some(o);
                            }
                            Err(e) => {
                                eprintln!("{name} {qname}: {e}");
                                break;
                            }
                        }
                    }
                    let Some(o) = last else {
                        failures += 1;
                        continue;
                    };
                    let mut t: Vec<f64> = times.into_iter().skip(1).collect();
                    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let m = &o.result.metrics;
                    let (count, hash) = result_digest(&o.result.batches);
                    let exp = expected.get(name).and_then(|d| d.get(qname));
                    let exp_count = exp.and_then(|e| e["count"].as_u64());
                    let validation = match exp {
                        None => "no-oracle",
                        Some(e) if e["count"].as_u64() == Some(count) && e["hash"].as_str() == Some(hash.as_str()) => "pass",
                        Some(_) => "FAIL",
                    };
                    if validation == "FAIL" {
                        failures += 1;
                        eprintln!("!! VALIDATION FAIL {name} {qname}: got count={count} hash={}, expected count={:?}", &hash[..12], exp_count);
                    }
                    let row = Row {
                        dataset: name.clone(),
                        size_bytes: file_size,
                        rows_in_file: n_rows,
                        distribution: dist.into(),
                        ordering: ordering.into(),
                        query: qname.into(),
                        result_count: count,
                        result_hash: hash,
                        expected_count: exp_count,
                        validation: validation.into(),
                        rows_returned_engine_observed: m.rows_returned.value.unwrap_or(0),
                        row_groups_total_source_observed: m.row_groups_total.value,
                        row_groups_skipped_source_estimated: m.row_groups_skipped.value,
                        rows_scanned_upper_bound_source_estimated: m.rows_scanned_upper_bound.value,
                        estimated_bytes_upper_bound_source_estimated: m.estimated_bytes_upper_bound.value,
                        bytes_read: m.bytes_read.value,
                        time_ms_median_engine_observed: t.get(t.len() / 2).copied().unwrap_or(f64::NAN),
                        time_ms_min_engine_observed: t.first().copied().unwrap_or(f64::NAN),
                        peak_buffer_memory_bytes_engine_observed: m.peak_memory_bytes.value,
                        reps: t.len(),
                    };
                    eprintln!(
                        "{:<28} {:<32} {:<9} n={:<8} rg_skip~{:?}/{:?} {:.1}ms",
                        row.dataset,
                        row.query,
                        row.validation,
                        row.result_count,
                        row.row_groups_skipped_source_estimated,
                        row.row_groups_total_source_observed,
                        row.time_ms_median_engine_observed
                    );
                    rows.push(row);
                }
            }
        }
    }

    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join(format!("bench-{tag}.json")), serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    let mut md = String::from(
        "| dataset | size MB | dist | order | query | result count | result hash | validation | rg skipped (est.) / total (obs.) | rows scanned <= (est.) | bytes <= (est.) | bytes read | median ms (obs.) | peak buf MB (obs.) |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in &rows {
        md += &format!(
            "| {} | {:.1} | {} | {} | {} | {} | {} | {} | {}/{} | {} | {} | unknown | {:.1} | {} |\n",
            r.dataset,
            r.size_bytes as f64 / 1e6,
            r.distribution,
            r.ordering,
            r.query,
            r.result_count,
            &r.result_hash[..12],
            r.validation,
            r.row_groups_skipped_source_estimated.unwrap_or(0),
            r.row_groups_total_source_observed.unwrap_or(0),
            r.rows_scanned_upper_bound_source_estimated.map(|v| v.to_string()).unwrap_or_default(),
            r.estimated_bytes_upper_bound_source_estimated.map(|v| v.to_string()).unwrap_or_default(),
            r.time_ms_median_engine_observed,
            r.peak_buffer_memory_bytes_engine_observed.map(|v| format!("{:.1}", v as f64 / 1e6)).unwrap_or_default(),
        );
    }
    std::fs::write(out.join(format!("bench-{tag}.md")), md).unwrap();
    eprintln!("wrote {}/bench-{tag}.{{json,md}}; validation failures: {failures}", out.display());
    if failures > 0 {
        std::process::exit(1);
    }
}
