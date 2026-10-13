//! Engine-level timeout / cancellation: the DuckDB query must really stop.

use geo_executor::{EngineError, ExecutionContext};
use geo_ir::*;
use geo_runtime::{Runtime, RuntimeError};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data")
}

fn heavy(rt: &Runtime, ctx: &ExecutionContext) -> Option<Plan> {
    let p = data_dir().join("clustered_100mb_spatial.parquet");
    if !p.exists() {
        eprintln!("SKIP: 100mb data missing");
        return None;
    }
    let src = || rt.open_source(p.to_str().unwrap(), ctx).unwrap();
    Some(Plan::new(Node::Aggregate {
        input: Box::new(Node::SpatialJoin {
            left: Box::new(src()),
            right: Box::new(src()),
            left_column: "geometry".into(),
            right_column: "geometry".into(),
            predicate: SpatialPredicate::Intersects,
            join_type: JoinType::Inner,
            right_prefix: "r_".into(),
        }),
        group_by: vec![],
        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n".into() }],
    }))
}

#[test]
fn timeout_interrupts_the_query_and_the_runtime_stays_usable() {
    let rt = Runtime::with_duckdb();
    let mut ctx = ExecutionContext::new(vec![data_dir()]);
    ctx.timeout = Duration::from_millis(300);
    let Some(plan) = heavy(&rt, &ctx) else { return };
    let t0 = Instant::now();
    match rt.run(&plan, &ctx) {
        Err(RuntimeError::Execution(EngineError::Timeout { limit_ms })) => assert_eq!(limit_ms, 300),
        other => panic!("expected Timeout, got {:?}", other.map(|_| ())),
    }
    assert!(t0.elapsed() < Duration::from_secs(8), "took {:?}", t0.elapsed());
    // same runtime, fresh context: works
    let ctx2 = ExecutionContext::new(vec![data_dir()]);
    let small = Plan::new(Node::Limit { input: Box::new(rt.open_source(data_dir().join("clustered_10mb_spatial.parquet").to_str().unwrap(), &ctx2).unwrap()), n: 3 });
    assert_eq!(rt.run(&small, &ctx2).unwrap().result.metrics.rows_returned.value, Some(3));
}

#[test]
fn cancel_token_interrupts_the_query() {
    let rt = Runtime::with_duckdb();
    let mut ctx = ExecutionContext::new(vec![data_dir()]);
    ctx.timeout = Duration::from_secs(120);
    let Some(plan) = heavy(&rt, &ctx) else { return };
    let cancel = ctx.cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel.cancel();
    });
    let t0 = Instant::now();
    match rt.run(&plan, &ctx) {
        Err(RuntimeError::Execution(EngineError::Cancelled)) => {}
        other => panic!("expected Cancelled, got {:?}", other.map(|_| ())),
    }
    assert!(t0.elapsed() < Duration::from_secs(8), "took {:?}", t0.elapsed());
}

#[test]
fn already_cancelled_context_does_not_start() {
    let rt = Runtime::with_duckdb();
    let ctx = ExecutionContext::new(vec![data_dir()]);
    let Some(plan) = heavy(&rt, &ctx) else { return };
    ctx.cancel.cancel();
    assert!(matches!(rt.run(&plan, &ctx), Err(RuntimeError::Execution(EngineError::Cancelled))));
}
