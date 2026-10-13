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

mod sql;

use duckdb::arrow::datatypes::{Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::Connection;
use geo_executor::*;
use geo_ir::{validate, OpKind, Plan, SourceFormat};
use geo_source_geoparquet::analyze_plan;
use geo_types::Crs;
use std::collections::{BTreeSet, HashMap};
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

fn load_spatial(conn: &Connection, allow_install: bool) -> Result<(), String> {
    if conn.execute_batch("LOAD spatial").is_ok() {
        return Ok(());
    }
    if !allow_install {
        return Err("spatial extension not installed and install disabled".into());
    }
    conn.execute_batch("INSTALL spatial; LOAD spatial").map_err(|e| e.to_string())
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
        let e = |x: duckdb::Error| EngineError::Engine(x.to_string());
        let conn = Connection::open_in_memory().map_err(e)?;
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
        .map_err(e)?;
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
            ops.extend([OpKind::Intersects, OpKind::Buffer, OpKind::SpatialJoin]);
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
            let e = |x: duckdb::Error| EngineError::Engine(x.to_string());
            conn.execute_batch(&format!(
                "PRAGMA enable_profiling = 'json'; PRAGMA profiling_output = {};",
                sql::q_str(&sql::sql_path(&profile_file))
            ))
            .map_err(e)?;

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
                let mut stmt = conn.prepare(&compiled.sql).map_err(e)?;
                let batches: Vec<_> = stmt.query_arrow([]).map_err(e)?.collect();
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
            None => metrics.notes.push("DuckDB profile unavailable".into()),
        }
        Ok(ExecutionResult { batches, metrics })
    }
}

/// Attach GeoArrow extension metadata (name + CRS from the IR schema) to WKB geometry columns.
fn tag_geometry(batches: Vec<RecordBatch>, geoms: &[(String, Crs)]) -> Vec<RecordBatch> {
    if geoms.is_empty() {
        return batches;
    }
    batches
        .into_iter()
        .map(|b| {
            let fields: Vec<Arc<Field>> = b
                .schema()
                .fields()
                .iter()
                .map(|f| match geoms.iter().find(|(n, _)| n == f.name()) {
                    Some((_, crs)) => {
                        let mut md: HashMap<String, String> = f.metadata().clone();
                        md.insert("ARROW:extension:name".into(), "geoarrow.wkb".into());
                        let ext = match crs {
                            Crs::Epsg(c) => serde_json::json!({"crs": format!("EPSG:{c}"), "crs_type": "authority_code"}),
                            Crs::OgcCrs84 => serde_json::json!({"crs": "OGC:CRS84", "crs_type": "authority_code"}),
                            Crs::Undefined => serde_json::json!({"crs_type": "undefined"}),
                            Crs::Unknown(s) => serde_json::json!({"crs": s, "crs_type": "unknown"}),
                        };
                        md.insert("ARROW:extension:metadata".into(), ext.to_string());
                        Arc::new(f.as_ref().clone().with_metadata(md))
                    }
                    None => f.clone(),
                })
                .collect();
            RecordBatch::try_new(Arc::new(Schema::new(fields)), b.columns().to_vec()).unwrap_or(b)
        })
        .collect()
}
