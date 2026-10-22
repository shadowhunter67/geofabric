//! Minimal DuckDB start-up repro (CI flake investigation, 2026-10-22). It does only what a GeoFabric query needs, with no GeoFabric code in the way:
//! open an in-memory DuckDB, load the `spatial` extension, build a deterministic Parquet fixture, apply the same session settings the engine
//! applies, run the first simple query, the first spatial query and a second one, over many iterations and (optionally) many threads.
//! Every failure prints the stage, iteration, thread, and the environment that matters (TEMP/TMP, cwd, DuckDB library/extension dir, version).
//! Exit code 1 if anything failed.
//!
//!   cargo run -q -p geo-engine-duckdb --example duckdb_repro --no-default-features -- [--iters N] [--threads T] [--lock] [--arrow-first]
//!
//! `--lock` serializes the extension LOAD/INSTALL like the engine does; `--arrow-first` makes the very first statement of a connection a
//! `query_arrow` (the engine's path) instead of `execute_batch`.
use duckdb::Connection;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

static EXT_LOCK: Mutex<()> = Mutex::new(());

fn arg(args: &[String], name: &str, default: usize) -> usize {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn context(thread: usize, iter: usize, stage: &str, threads: usize) -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "<unset>".into());
    format!(
        "stage={stage} iter={iter} thread={thread}/{threads} pid={} ts_ms={} TEMP={} TMP={} cwd={:?} HOME={} DUCKDB_LIB_DIR={}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
        env("TEMP"),
        env("TMP"),
        std::env::current_dir().ok(),
        env("USERPROFILE"),
        env("DUCKDB_LIB_DIR"),
    )
}

fn run_batches(conn: &Connection, sql: &str) -> Result<usize, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("prepare: {e}"))?;
    let rows: usize = stmt.query_arrow([]).map_err(|e| format!("query_arrow: {e}"))?.map(|b| b.num_rows()).sum();
    Ok(rows)
}

fn one(thread: usize, iter: usize, threads: usize, lock: bool, arrow_first: bool, fixture: &str) -> Result<(), (String, String)> {
    let fail = |stage: &str, e: String| (stage.to_string(), format!("{e} | {}", context(thread, iter, stage, threads)));
    let conn = Connection::open_in_memory().map_err(|e| fail("open", e.to_string()))?;
    if arrow_first {
        run_batches(&conn, "SELECT 1 AS one").map_err(|e| fail("first_simple_query(arrow)", e))?;
    }
    {
        let _g = if lock { Some(EXT_LOCK.lock().unwrap_or_else(|p| p.into_inner())) } else { None };
        if conn.execute_batch("LOAD spatial").is_err() {
            conn.execute_batch("INSTALL spatial").map_err(|e| fail("install_spatial", e.to_string()))?;
            conn.execute_batch("LOAD spatial").map_err(|e| fail("load_spatial_after_install", e.to_string()))?;
        }
    }
    if !arrow_first {
        run_batches(&conn, "SELECT 1 AS one").map_err(|e| fail("first_simple_query", e))?;
    }
    // the engine's session settings
    let dir = std::path::Path::new(fixture).parent().unwrap().to_string_lossy().replace('\\', "/");
    conn.execute_batch(&format!(
        "SET threads = 2; SET memory_limit = '1024MB'; SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         SET allowed_directories = ['{dir}']; SET allowed_paths = ['{}']; SET enable_external_access = false;",
        fixture.replace('\\', "/")
    ))
    .map_err(|e| fail("session_setup", e.to_string()))?;
    let q = format!("SELECT id, ST_AsWKB(geometry) AS g FROM read_parquet('{}') WHERE id % 7 = 0 ORDER BY id LIMIT 50", fixture.replace('\\', "/"));
    let n1 = run_batches(&conn, &q).map_err(|e| fail("first_spatial_query", e))?;
    let n2 = run_batches(&conn, &q).map_err(|e| fail("second_spatial_query", e))?;
    if n1 == 0 || n2 == 0 {
        return Err(fail("result_check", format!("empty result n1={n1} n2={n2}")));
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let iters = arg(&args, "--iters", 20);
    let threads = arg(&args, "--threads", 1);
    let lock = args.iter().any(|a| a == "--lock");
    let arrow_first = args.iter().any(|a| a == "--arrow-first");

    let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    let dir = tmp.join(format!("duckdb-repro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = dir.join("fixture.parquet").to_string_lossy().to_string();

    // deterministic fixture, written by DuckDB itself
    {
        let c = Connection::open_in_memory().expect("open fixture connection");
        let _ = c.execute_batch("INSTALL spatial");
        c.execute_batch("LOAD spatial").expect("LOAD spatial (fixture)");
        c.execute_batch(&format!(
            "COPY (SELECT i AS id, ST_Point(i, i) AS geometry FROM range(1000) t(i)) TO '{}' (FORMAT parquet)",
            fixture.replace('\\', "/")
        ))
        .expect("write fixture");
    }
    println!(
        "duckdb_repro: iters={iters} threads={threads} lock={lock} arrow_first={arrow_first} fixture={fixture}\n  {}",
        context(0, 0, "start", threads)
    );
    let version: String = Connection::open_in_memory().unwrap().query_row("select version()", [], |r| r.get(0)).unwrap_or_default();
    println!("  duckdb {version}");

    let failures = Arc::new(AtomicUsize::new(0));
    let first_error = Arc::new(Mutex::new(Vec::<String>::new()));
    let started = std::time::Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let (failures, first_error, fixture) = (failures.clone(), first_error.clone(), fixture.clone());
            std::thread::spawn(move || {
                for i in 0..iters {
                    if let Err((stage, msg)) = one(t, i, threads, lock, arrow_first, &fixture) {
                        failures.fetch_add(1, Ordering::SeqCst);
                        let mut v = first_error.lock().unwrap();
                        if v.len() < 8 {
                            v.push(format!("FAIL {stage}: {msg}"));
                        }
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let n = failures.load(Ordering::SeqCst);
    for e in first_error.lock().unwrap().iter() {
        println!("{e}");
    }
    println!("duckdb_repro: {} connections, {} failures, {:.1}s", iters * threads, n, started.elapsed().as_secs_f64());
    let _ = std::fs::remove_file(&fixture);
    let _ = std::fs::remove_dir(&dir);
    if n > 0 {
        std::process::exit(1);
    }
}
