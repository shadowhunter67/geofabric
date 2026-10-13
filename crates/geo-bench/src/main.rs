//! Benchmark runner. Usage:
//!   cargo run --release -p geo-bench -- --data benchmarks/data --out benchmarks/results \
//!       --sizes 10mb,100mb --reps 3
//!
//! No timing thresholds are asserted: the output is for *relative* comparison
//! (pruning effectiveness, bytes read, spatial vs shuffled ordering).

use geo_executor::ExecutionContext;
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
    rows_scanned_upper_bound: Option<u64>,
    rows_returned: u64,
    row_groups_total: Option<u64>,
    row_groups_skipped: Option<u64>,
    bytes_read_duckdb: Option<u64>,
    predicted_bytes_after_prune: Option<u64>,
    time_ms_median: f64,
    time_ms_min: f64,
    peak_buffer_memory_bytes: Option<u64>,
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

fn arg(args: &[String], name: &str, default: &str) -> String {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = PathBuf::from(arg(&args, "--data", "benchmarks/data"));
    let out = PathBuf::from(arg(&args, "--out", "benchmarks/results"));
    let sizes: Vec<String> = arg(&args, "--sizes", "10mb,100mb").split(',').map(String::from).collect();
    let reps: usize = arg(&args, "--reps", "3").parse().expect("--reps");
    let tag = arg(&args, "--tag", "run");

    let rt = Runtime::with_duckdb();
    let mut ctx = ExecutionContext::new(vec![data.clone()]);
    ctx.max_rows = 5_000_000;
    ctx.memory_limit_mb = 4096;
    let mut rows: Vec<Row> = Vec::new();

    for size in &sizes {
        for dist in ["clustered", "uniform"] {
            for ordering in ["spatial", "shuffled"] {
                let name = format!("{dist}_{size}_{ordering}");
                let path = data.join(format!("{name}.parquet"));
                if !path.exists() {
                    eprintln!("skip {name}: missing");
                    continue;
                }
                let src = rt.open_source(&path).expect("open source");
                let file_size = std::fs::metadata(&path).unwrap().len();
                let n_rows = match &src {
                    Node::Source { statistics, .. } => statistics.row_count.unwrap_or(0),
                    _ => 0,
                };
                for (qname, plan) in queries(&src) {
                    let mut times = Vec::new();
                    let mut last = None;
                    for _ in 0..(reps + 1) {
                        // first iteration is a warm-up (OS file cache) and is discarded
                        match rt.run(&plan, &ctx) {
                            Ok(o) => {
                                times.push(o.result.metrics.elapsed_ms);
                                last = Some(o);
                            }
                            Err(e) => {
                                eprintln!("{name} {qname}: {e}");
                                break;
                            }
                        }
                    }
                    let Some(o) = last else { continue };
                    let mut t: Vec<f64> = times.into_iter().skip(1).collect();
                    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let m = &o.result.metrics;
                    let row = Row {
                        dataset: name.clone(),
                        size_bytes: file_size,
                        rows_in_file: n_rows,
                        distribution: dist.into(),
                        ordering: ordering.into(),
                        query: qname.into(),
                        rows_scanned_upper_bound: m.rows_scanned,
                        rows_returned: m.rows_returned,
                        row_groups_total: m.row_groups_total,
                        row_groups_skipped: m.row_groups_skipped,
                        bytes_read_duckdb: m.bytes_read,
                        predicted_bytes_after_prune: o.physical.estimate.bytes_to_read,
                        time_ms_median: t.get(t.len() / 2).copied().unwrap_or(f64::NAN),
                        time_ms_min: t.first().copied().unwrap_or(f64::NAN),
                        peak_buffer_memory_bytes: m.peak_memory_bytes,
                        reps: t.len(),
                    };
                    eprintln!(
                        "{:<28} {:<34} ret={:<8} rg_skip={:?}/{:?} bytes={:?} {:.1}ms",
                        row.dataset, row.query, row.rows_returned, row.row_groups_skipped, row.row_groups_total,
                        row.bytes_read_duckdb, row.time_ms_median
                    );
                    rows.push(row);
                }
            }
        }
    }

    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join(format!("bench-{tag}.json")), serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    let mut md = String::from(
        "| dataset | size MB | distribution | ordering | query | rows scanned (<=) | rows returned | row groups skipped | bytes read (DuckDB) | predicted bytes | median ms | peak buf MB |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in &rows {
        md += &format!(
            "| {} | {:.1} | {} | {} | {} | {} | {} | {}/{} | {} | {} | {:.1} | {} |\n",
            r.dataset,
            r.size_bytes as f64 / 1e6,
            r.distribution,
            r.ordering,
            r.query,
            r.rows_scanned_upper_bound.map(|v| v.to_string()).unwrap_or_default(),
            r.rows_returned,
            r.row_groups_skipped.unwrap_or(0),
            r.row_groups_total.unwrap_or(0),
            r.bytes_read_duckdb.map(|v| v.to_string()).unwrap_or_default(),
            r.predicted_bytes_after_prune.map(|v| v.to_string()).unwrap_or_default(),
            r.time_ms_median,
            r.peak_buffer_memory_bytes.map(|v| format!("{:.1}", v as f64 / 1e6)).unwrap_or_default(),
        );
    }
    std::fs::write(out.join(format!("bench-{tag}.md")), md).unwrap();
    eprintln!("wrote {}/bench-{tag}.{{json,md}}", out.display());
}
