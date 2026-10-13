//! DuckDB execution adapter (RFC-002, ADR-004).
//!
//! In-process DuckDB with the `spatial` extension. The connection is
//! sandboxed: the extension is loaded first, then external access is
//! disabled except for the allow-listed source roots (+ a private temp dir
//! for the profiler output).

mod sql;

use duckdb::arrow::datatypes::{Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::Connection;
use geo_types::Crs;
use std::collections::HashMap;
use std::sync::Arc;
use geo_executor::*;
use geo_ir::{validate, OpKind, Plan, SourceFormat};
use geo_source_geoparquet::analyze_plan;
use std::collections::BTreeSet;
use std::time::Instant;

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
        Ok(sql::compile(&plan.root, ctx, self.spatial)?.sql)
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

    fn open(&self, ctx: &ExecutionContext, profile_dir: &std::path::Path) -> Result<Connection, EngineError> {
        let e = |x: duckdb::Error| EngineError::Engine(x.to_string());
        let conn = Connection::open_in_memory().map_err(e)?;
        if self.spatial {
            load_spatial(&conn, self.config.allow_extension_install).map_err(EngineError::Engine)?;
        }
        let mut dirs: Vec<String> = Vec::new();
        for r in &ctx.allowed_roots {
            if let Ok(c) = std::fs::canonicalize(r) {
                dirs.push(sql::q_str(&sql::sql_path(&c)));
            }
        }
        dirs.push(sql::q_str(&sql::sql_path(profile_dir)));
        conn.execute_batch(&format!(
            "SET threads = {}; SET memory_limit = '{}MB'; \
             SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
             SET allowed_directories = [{}]; SET enable_external_access = false;",
            ctx.threads.max(1),
            ctx.memory_limit_mb.max(64),
            dirs.join(", ")
        ))
        .map_err(e)?;
        Ok(conn)
    }
}

impl ExecutionEngine for DuckDbEngine {
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
            ops,
            sources: [SourceFormat::GeoParquet].into_iter().collect(),
            location: "in-process DuckDB (local file system sources)".into(),
            notes,
        }
    }

    fn estimate(&self, plan: &Plan) -> Result<Estimate, EngineError> {
        let scans = analyze_plan(plan).map_err(|e| EngineError::Engine(e.to_string()))?;
        Ok(Estimate {
            rows_upper_bound: Some(scans.iter().map(|s| s.prune.kept_rows).sum()),
            bytes_to_read: Some(scans.iter().map(|s| s.prune.kept_bytes).sum()),
            confidence: Confidence::Metadata,
        })
    }

    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError> {
        self.check(plan)?;
        let scans = analyze_plan(plan).map_err(|e| EngineError::Engine(e.to_string()))?;
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
                label: "Spatial prune".into(),
                detail: format!(
                    "{} constraint(s) applied; skip {}/{} row groups via {:?}; ~{} of {} bytes remain",
                    s.constraints_applied, s.prune.skipped, s.prune.total_row_groups, s.prune.method,
                    s.prune.kept_bytes, s.prune.total_bytes
                ),
            });
        }
        steps.push(ExplainStep {
            label: "Compile logical plan -> DuckDB SQL".into(),
            detail: self.compile_sql(plan, ctx)?,
        });
        steps.push(ExplainStep { label: "Execute".into(), detail: "DuckDB in-process, sandboxed file access".into() });
        steps.push(ExplainStep { label: "Result".into(), detail: "Arrow record batches (geometry as geoarrow.wkb)".into() });
        Ok(steps)
    }

    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError> {
        self.check(plan)?;
        let scans = analyze_plan(plan).map_err(|e| EngineError::Engine(e.to_string()))?;
        let compiled = sql::compile(&plan.root, ctx, self.spatial)?;

        let profile_dir = std::env::temp_dir().join(format!(
            "geofabric-duckdb-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        std::fs::create_dir_all(&profile_dir).map_err(|e| EngineError::Engine(e.to_string()))?;
        let profile_dir = std::fs::canonicalize(&profile_dir).map_err(|e| EngineError::Engine(e.to_string()))?;
        let profile_file = profile_dir.join("profile.json");

        let result = (|| {
            let conn = self.open(ctx, &profile_dir)?;
            let e = |x: duckdb::Error| EngineError::Engine(x.to_string());
            conn.execute_batch(&format!(
                "PRAGMA enable_profiling = 'json'; PRAGMA profiling_output = {};",
                sql::q_str(&sql::sql_path(&profile_file))
            ))
            .map_err(e)?;
            let start = Instant::now();
            let mut stmt = conn.prepare(&compiled.sql).map_err(e)?;
            let batches: Vec<_> = stmt.query_arrow([]).map_err(e)?.collect();
            let elapsed = start.elapsed();
            Ok::<_, EngineError>((batches, elapsed))
        })();

        let profile = std::fs::read_to_string(&profile_file).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let _ = std::fs::remove_file(&profile_file);
        let _ = std::fs::remove_dir(&profile_dir);
        let (batches, elapsed) = result?;

        let batches = tag_geometry(batches, &compiled.geometry_columns);
        let rows: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
        if rows > ctx.max_rows {
            return Err(EngineError::LimitExceeded(format!("result exceeds max_rows={}", ctx.max_rows)));
        }
        let mut metrics = ExecutionMetrics {
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
            rows_returned: rows,
            rows_scanned: Some(scans.iter().map(|s| s.prune.kept_rows).sum()),
            row_groups_total: Some(scans.iter().map(|s| s.prune.total_row_groups as u64).sum()),
            row_groups_skipped: Some(scans.iter().map(|s| s.prune.skipped as u64).sum()),
            scan_metrics_source:
                "rows_scanned/row_groups_*: geoparquet statistics analysis (upper bound, not measured by DuckDB); \
                 bytes_read/peak_memory: DuckDB profiler"
                    .into(),
            ..Default::default()
        };
        match profile {
            Some(p) => {
                metrics.bytes_read = p["total_bytes_read"].as_u64();
                metrics.peak_memory_bytes = p["system_peak_buffer_memory"].as_u64();
            }
            None => metrics.notes.push("DuckDB profile unavailable".into()),
        }
        metrics.notes.push(
            "bytes_read is DuckDB profiler total_bytes_read: it UNDER-counts (a full column scan of a 98 MB file reported ~56 KB); use it only for relative comparison, not as true I/O"
                .into(),
        );
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
