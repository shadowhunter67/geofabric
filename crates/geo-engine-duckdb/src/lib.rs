//! DuckDB execution adapter (RFC-002, ADR-004).
//!
//! In-process DuckDB with the `spatial` extension. The connection is
//! sandboxed: the extension is loaded first, then external access is
//! disabled except for the files that passed the access guard (+ a private
//! temp dir for the profiler output).
//!
//! Timeout / cancellation: a watchdog thread calls DuckDB's
//! `InterruptHandle::interrupt()` when `ctx.timeout` elapses or `ctx.cancel`
//! is set, so the query really stops (not just the caller's wait).

mod cost;
mod sql;

use duckdb::arrow::record_batch::RecordBatch;
use duckdb::Connection;
use geo_executor::*;
use geo_ir::{validate, OpKind, Plan, SourceFormat};
use geo_source_geoparquet::analyze_plan;
use geo_types::Crs;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const ENGINE_ID: &str = "duckdb";

#[derive(Debug, Clone)]
pub struct DuckDbConfig {
    /// If the spatial extension is not installed locally, try to download it.
    pub allow_extension_install: bool,
}

impl Default for DuckDbConfig {
    fn default() -> Self {
        DuckDbConfig { allow_extension_install: true }
    }
}

pub struct DuckDbEngine {
    config: DuckDbConfig,
    spatial: bool,
    spatial_error: Option<String>,
}

/// Serializes the extension load/install inside this process: on a machine that has never installed `spatial`, several engines
/// created at once would all run `INSTALL` against the same extension directory and one of them fails with an opaque error.
/// Number of queries this process has started (1 = the very first one): an opaque first-query failure is a different problem from a later one.
static QUERY_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Environment that matters when a DuckDB statement fails opaquely (CI flake investigation). No secrets: paths, ids and versions only.
fn failure_context(stage: &str, threads: usize, query_no: usize, sql: &str) -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "<unset>".into());
    format!(
        "[diag stage={stage} query_no={query_no} ({}) thread={:?} test_threads={} pid={} ts_ms={} engine_threads={threads} TEMP={} TMP={} cwd={:?} DUCKDB_LIB_DIR={} sql_head={:?}]",
        if query_no <= 1 { "FIRST query of this process" } else { "later query" },
        std::thread::current().name().unwrap_or("?"),
        env("RUST_TEST_THREADS"),
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
        env("TEMP"),
        env("TMP"),
        std::env::current_dir().ok(),
        env("DUCKDB_LIB_DIR"),
        sql.chars().take(120).collect::<String>(),
    )
}

static EXTENSION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn load_spatial(conn: &Connection, allow_install: bool) -> Result<(), String> {
    let _guard = EXTENSION_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if conn.execute_batch("LOAD spatial").is_ok() {
        return Ok(());
    }
    if !allow_install {
        return Err("spatial extension not installed and install disabled".into());
    }
    // Separate statements, so a failure says which step failed (the batch form reports an opaque "Error code 1"). On a machine that
    // has never installed the extension the very first LOAD right after the download was seen to fail on a hosted Windows runner
    // while the next one succeeded, so the LOAD is retried a few times before giving up.
    conn.execute_batch("INSTALL spatial").map_err(|e| format!("INSTALL spatial failed: {e}"))?;
    let mut last = String::new();
    for attempt in 0..4 {
        match conn.execute_batch("LOAD spatial") {
            Ok(()) => return Ok(()),
            Err(e) => last = format!("LOAD spatial failed after INSTALL (attempt {}): {e}", attempt + 1),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(last)
}

impl DuckDbEngine {
    pub fn new(config: DuckDbConfig) -> Self {
        let probe = Connection::open_in_memory()
            .map_err(|e| e.to_string())
            .and_then(|c| load_spatial(&c, config.allow_extension_install));
        let (spatial, spatial_error) = match probe {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        DuckDbEngine { config, spatial, spatial_error }
    }

    pub fn spatial_available(&self) -> bool {
        self.spatial
    }

    /// Why the spatial extension could not be loaded (None when it is available): shown by the probe and the Developer view.
    pub fn spatial_error(&self) -> Option<&str> {
        self.spatial_error.as_deref()
    }

    /// The SQL this adapter would run for `plan` (for explain / debugging).
    pub fn compile_sql(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<String, EngineError> {
        Ok(sql::compile(plan, ctx, self.spatial)?.sql)
    }

    fn check(&self, plan: &Plan) -> Result<(), EngineError> {
        let rep = validate(plan);
        if !rep.is_ok() {
            let msgs: Vec<String> = rep.errors().map(|i| format!("[{}] {}", i.code, i.message)).collect();
            return Err(EngineError::InvalidPlan(msgs.join("; ")));
        }
        let missing = self.capabilities().missing_ops(plan);
        if !missing.is_empty() {
            return Err(EngineError::Unsupported(format!(
                "operations not supported: {}",
                missing.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ")
            )));
        }
        Ok(())
    }

    fn open(
        &self,
        ctx: &ExecutionContext,
        sources: &[geo_types::AuthorizedSource],
        profile_dir: &std::path::Path,
    ) -> Result<Connection, EngineError> {
        let conn = Connection::open_in_memory().map_err(|x| EngineError::Engine(format!("open in-memory connection: {x}")))?;
        if self.spatial {
            load_spatial(&conn, self.config.allow_extension_install).map_err(EngineError::Engine)?;
        }
        // Only the authorized files (and the private profile dir) stay readable.
        let files: Vec<String> = sources.iter().map(|a| sql::q_str(&sql::sql_path(a.path()))).collect();
        let dirs = [sql::q_str(&sql::sql_path(profile_dir))];
        conn.execute_batch(&format!(
            "SET threads = {}; SET memory_limit = '{}MB'; \
             SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
             SET allowed_paths = [{}]; SET allowed_directories = [{}]; SET enable_external_access = false;",
            ctx.threads.max(1),
            ctx.memory_limit_mb.max(64),
            files.join(", "),
            dirs.join(", ")
        ))
        .map_err(|x| EngineError::Engine(format!("session setup (SET threads/memory_limit/allowed_paths/...): {x}")))?;
        Ok(conn)
    }
}

fn scan_analysis(plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<geo_source_geoparquet::SourceScan>, EngineError> {
    analyze_plan(plan, ctx.guard.as_ref()).map_err(|e| match e {
        geo_source_geoparquet::SourceError::AccessDenied => EngineError::AccessDenied,
        other => EngineError::Engine(other.to_string()),
    })
}

impl EngineDescriptor for DuckDbEngine {
    fn cost_estimate(&self, _plan: &Plan, features: &geo_executor::WorkloadFeatures) -> Option<geo_executor::EngineCostEstimate> {
        cost::estimate(features)
    }

    fn id(&self) -> EngineId {
        EngineId::new(ENGINE_ID)
    }

    fn capabilities(&self) -> Capabilities {
        let mut ops: BTreeSet<OpKind> = [
            OpKind::Source,
            OpKind::Filter,
            OpKind::BboxFilter,
            OpKind::Aggregate,
            OpKind::Project,
            OpKind::Limit,
            OpKind::Sort,
        ]
        .into_iter()
        .collect();
        let mut notes = vec!["bbox_filter without a bbox covering column needs the spatial extension".to_string()];
        if self.spatial {
            ops.extend([OpKind::Intersects, OpKind::Buffer, OpKind::Transform, OpKind::SpatialJoin, OpKind::SpatialSemiJoin]);
        } else {
            notes.push(format!(
                "spatial extension unavailable ({}): intersects/buffer/spatial_join disabled",
                self.spatial_error.clone().unwrap_or_default()
            ));
        }
        Capabilities {
            kind: EngineKind::Server,
            ops,
            sources: [SourceFormat::GeoParquet].into_iter().collect(),
            limits: EngineLimits::default(),
            bbox_filter_requires_covering: !self.spatial,
            location: "in-process DuckDB (local file system sources)".into(),
            notes,
        }
    }

    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Estimate, EngineError> {
        let scans = scan_analysis(plan, ctx)?;
        Ok(Estimate {
            rows_upper_bound: Metric::source_estimated(scans.iter().map(|s| s.prune.kept_rows).sum()),
            bytes_upper_bound: Metric::source_estimated(scans.iter().map(|s| s.prune.kept_bytes).sum()),
        })
    }

    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError> {
        self.check(plan)?;
        let scans = scan_analysis(plan, ctx)?;
        let mut steps = Vec::new();
        for s in &scans {
            steps.push(ExplainStep {
                label: "Read GeoParquet metadata".into(),
                detail: format!(
                    "{}: {} rows, {} row groups, CRS {}",
                    s.uri, s.total_rows, s.prune.total_row_groups, s.crs
                ),
            });
            steps.push(ExplainStep {
                label: "Spatial prune (estimated from statistics)".into(),
                detail: format!(
                    "{} constraint(s) applied; would skip {}/{} row groups via {:?}; upper bound {} of {} bytes remain",
                    s.constraints_applied,
                    s.prune.skipped,
                    s.prune.total_row_groups,
                    s.prune.method,
                    s.prune.kept_bytes,
                    s.prune.total_bytes
                ),
            });
        }
        steps.push(ExplainStep {
            label: "Compile logical plan -> DuckDB SQL".into(),
            detail: self.compile_sql(plan, ctx)?,
        });
        steps.push(ExplainStep {
            label: "Execute".into(),
            detail: format!(
                "DuckDB in-process reads Parquet itself; sandboxed to authorized files; timeout {} ms",
                ctx.timeout.as_millis()
            ),
        });
        steps.push(ExplainStep {
            label: "Result".into(),
            detail: "geometry returned as WKB; GeoFabric re-attaches GeoArrow metadata + CRS from the IR; Arrow record batches"
                .into(),
        });
        Ok(steps)
    }
}

impl ExecutionEngine for DuckDbEngine {
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError> {
        self.check(plan)?;
        if ctx.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let scans = scan_analysis(plan, ctx)?;
        let compiled = sql::compile(plan, ctx, self.spatial)?;

        let profile_dir = std::env::temp_dir().join(format!(
            "geofabric-duckdb-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        std::fs::create_dir_all(&profile_dir).map_err(|e| EngineError::Engine(e.to_string()))?;
        let profile_dir = std::fs::canonicalize(&profile_dir).map_err(|e| EngineError::Engine(e.to_string()))?;
        let profile_file = profile_dir.join("profile.json");
        let fired = Arc::new(AtomicU8::new(0)); // 1 = timeout, 2 = cancelled

        let result = (|| {
            let conn = self.open(ctx, &compiled.sources, &profile_dir)?;
            // The DuckDB profiler is OPT-IN (GEOFABRIC_DUCKDB_PROFILE=1; the benchmark tool sets it). Root cause of the CI flake (2026-10-22):
            // with the profiler's JSON output on, several engines running in one process failed sporadically with an opaque "Error code 1"
            // (4 of 15 runs on a hosted runner, same tests with it off: 0 of 15). Peak memory is therefore Unknown unless it is enabled.
            if profiling_enabled(std::env::var("GEOFABRIC_DUCKDB_PROFILE").ok().as_deref()) {
                conn.execute_batch(&format!(
                    "PRAGMA enable_profiling = 'json'; PRAGMA profiling_output = {};",
                    sql::q_str(&sql::sql_path(&profile_file))
                ))
                .map_err(|x| EngineError::Engine(format!("profiling setup: {x}")))?;
            }

            // Watchdog: really interrupts the running query.
            let handle = conn.interrupt_handle();
            let (stop_tx, stop_rx) = channel::<()>();
            let watcher = {
                let fired = fired.clone();
                let cancel = ctx.cancel.clone();
                let timeout = ctx.timeout;
                std::thread::spawn(move || {
                    let start = Instant::now();
                    loop {
                        match stop_rx.recv_timeout(Duration::from_millis(10)) {
                            Err(RecvTimeoutError::Timeout) => {}
                            _ => return,
                        }
                        if fired.load(Ordering::SeqCst) == 0 {
                            if cancel.is_cancelled() {
                                fired.store(2, Ordering::SeqCst);
                            } else if start.elapsed() >= timeout {
                                fired.store(1, Ordering::SeqCst);
                            }
                        }
                        if fired.load(Ordering::SeqCst) != 0 {
                            handle.interrupt(); // repeated each tick until the query unwinds
                        }
                    }
                })
            };
            let start = Instant::now();
            let run = (|| {
                let query_no = QUERY_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
                let mut stmt = conn.prepare(&compiled.sql).map_err(|x| EngineError::Engine(format!("prepare: {x} {}", failure_context("prepare", ctx.threads, query_no, &compiled.sql))))?;
                let batches: Vec<_> = match stmt.query_arrow([]) {
                    Ok(arrow) => arrow.collect(),
                    Err(first) => {
                        // a statement that WE interrupted (timeout / cancel) is not re-run to learn why it failed: that second execution of an
                        // expensive plan, with the watchdog still interrupting it every tick, only prolonged the unwind (and was the one place a
                        // timed-out query ran twice). The caller maps `fired` to Timeout / Cancelled anyway.
                        if fired.load(Ordering::SeqCst) != 0 {
                            return Err(EngineError::Engine(format!("interrupted: {first}")));
                        }
                        // the arrow path reports a failed statement as an opaque "Error code N" without DuckDB's message; the row API
                        // keeps the message, so re-run the failing statement through it purely to say what actually went wrong
                        let detail = conn.prepare(&compiled.sql).and_then(|mut s| s.query([]).map(|_| ())).err().map(|d| d.to_string());
                        return Err(match detail {
                            Some(d) if d != first.to_string() => EngineError::Engine(format!("{first} ({d}) {}", failure_context("execute", ctx.threads, query_no, &compiled.sql))),
                            _ => EngineError::Engine(format!("{first} {}", failure_context("execute", ctx.threads, query_no, &compiled.sql))),
                        });
                    }
                };
                Ok::<_, EngineError>(batches)
            })();
            let elapsed = start.elapsed();
            drop(stop_tx);
            let _ = watcher.join();
            let batches = match run {
                Ok(b) => b,
                Err(err) => {
                    return Err(match fired.load(Ordering::SeqCst) {
                        1 => EngineError::Timeout { limit_ms: ctx.timeout.as_millis() as u64 },
                        2 => EngineError::Cancelled,
                        _ => err,
                    })
                }
            };
            Ok::<_, EngineError>((batches, elapsed))
        })();

        let profile = std::fs::read_to_string(&profile_file)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let _ = std::fs::remove_file(&profile_file);
        let _ = std::fs::remove_dir(&profile_dir);
        let (batches, elapsed) = result?;

        let batches = tag_geometry(batches, &compiled.geometry_columns);
        let rows: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
        if rows > ctx.max_rows {
            return Err(EngineError::LimitExceeded(format!("result exceeds max_rows={}", ctx.max_rows)));
        }
        let mut metrics = ExecutionMetrics {
            elapsed_ms: Metric::engine_observed(elapsed.as_secs_f64() * 1000.0),
            rows_returned: Metric::engine_observed(rows),
            row_groups_total: Metric::source_observed(scans.iter().map(|s| s.prune.total_row_groups as u64).sum()),
            row_groups_skipped: Metric::source_estimated(scans.iter().map(|s| s.prune.skipped as u64).sum()),
            rows_scanned_upper_bound: Metric::source_estimated(scans.iter().map(|s| s.prune.kept_rows).sum()),
            estimated_bytes_upper_bound: Metric::source_estimated(scans.iter().map(|s| s.prune.kept_bytes).sum()),
            bytes_read: Metric::unknown(),
            peak_memory_bytes: Metric::unknown(),
            notes: vec![
                "bytes_read is Unknown: DuckDB's profiler total_bytes_read under-counts (56 KB reported for a column scan of a 98 MB file), so it is deliberately not reported".into(),
                "row_groups_skipped is derived from Parquet statistics by GeoFabric; DuckDB does not report its own pruning".into(),
            ],
        };
        match profile.and_then(|p| p["system_peak_buffer_memory"].as_u64()) {
            Some(v) => {
                metrics.peak_memory_bytes = Metric::engine_observed(v);
                metrics.notes.push("peak_memory_bytes is DuckDB's buffer-manager peak only (not process RSS)".into());
            }
            None => metrics.notes.push("peak_memory_bytes is Unknown: the DuckDB profiler is off by default (it made concurrent in-process engines fail sporadically; GEOFABRIC_DUCKDB_PROFILE=1 enables it)".into()),
        }
        Ok(ExecutionResult { batches, metrics })
    }
}

/// The profiling contract: the DuckDB profiler is on ONLY when `GEOFABRIC_DUCKDB_PROFILE` is exactly `"1"` (unset, empty, `0`, `true`... are off).
/// A pure function so the contract is unit-tested without mutating the process environment (tests run in parallel).
fn profiling_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

#[cfg(test)]
mod profiling_contract {
    use super::profiling_enabled;

    #[test]
    fn profiler_is_opt_in_and_only_for_exactly_one() {
        assert!(!profiling_enabled(None), "default must be off: the profiler made concurrent in-process engines fail sporadically");
        for off in ["", "0", "true", "yes", "on", " 1", "1 "] {
            assert!(!profiling_enabled(Some(off)), "{off:?} must not enable the profiler");
        }
        assert!(profiling_enabled(Some("1")));
    }
}

/// Attach GeoArrow extension metadata (shared with every other engine: `geo_executor::tag_geometry`).
fn tag_geometry(batches: Vec<RecordBatch>, geoms: &[(String, Crs)]) -> Vec<RecordBatch> {
    geo_executor::tag_geometry(batches, geoms)
}
