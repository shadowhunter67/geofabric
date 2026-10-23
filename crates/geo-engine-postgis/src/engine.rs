//! `PostGisEngine`: a peer of `DuckDbEngine` under the same contract (`geo_executor::EngineDescriptor` + `ExecutionEngine`).
//!
//! Order of events in `execute`, which is the architecture invariant of this phase:
//!   validate plan -> AUTHORIZE every source through the context's guard -> residency + lowering (pure) -> cancel check ->
//!   bounded execution on the server -> error mapping -> metrics with provenance.
//! Nothing that touches the database (not even the catalog) happens before authorization, and an unsupported or non-resident plan
//! is refused by the planner (via `source_residency` and the capability matrix) before it ever reaches `execute`.

use crate::catalog::PostGisCatalog;
use crate::connector::{PgConnector, PgError, PgErrorKind, PgQuery};
use crate::lower::{lower, LowerError};
use crate::probe::PostGisProbe;
use geo_executor::{
    limits, tag_geometry, Capabilities, EngineCalibration, EngineDescriptor, EngineError, EngineId, EngineKind, EngineLimits, Estimate,
    ExecutionContext, ExecutionEngine, ExecutionMetrics, ExecutionResult, ExplainStep, Metric,
};
use geo_ir::{validate, OpKind, Plan, PlanNode, SourceFormat};
use arrow_array::Array;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct PostGisEngine {
    catalog: PostGisCatalog,
    connector: Arc<dyn PgConnector>,
    last_probe: Mutex<Option<PostGisProbe>>,
}

impl PostGisEngine {
    pub fn new(catalog: PostGisCatalog, connector: Arc<dyn PgConnector>) -> Self {
        PostGisEngine { catalog, connector, last_probe: Mutex::new(None) }
    }

    pub fn catalog(&self) -> &PostGisCatalog {
        &self.catalog
    }

    /// Ask the server whether it can run our SQL (system catalogs only). Cached as `last_probe` for the UI.
    pub fn probe(&self) -> PostGisProbe {
        let p = match self.connector.probe() {
            Ok(inputs) => PostGisProbe::evaluate(&inputs),
            Err(e) => PostGisProbe::unreachable(&e.message),
        };
        if let Ok(mut g) = self.last_probe.lock() {
            *g = Some(p.clone());
        }
        p
    }

    pub fn last_probe(&self) -> Option<PostGisProbe> {
        self.last_probe.lock().ok().and_then(|g| g.clone())
    }

    /// BENCHMARK EVIDENCE ONLY (never used by the planner, never on a user request path): the server's own account of how it ran `plan`,
    /// `EXPLAIN (ANALYZE, BUFFERS)` as text lines (plan node types, actual rows/loops, shared hit/read blocks, index names, planning and
    /// execution time). The statement is the same parameterized SELECT `execute` would send, authorized and lowered the same way, on the same
    /// read-only role, so EXPLAIN ANALYZE cannot write anything.
    pub fn explain_analyze(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<String>, EngineError> {
        for n in plan.nodes.values() {
            if let PlanNode::Source { uri, .. } = n {
                ctx.guard.authorize(uri).map_err(|_| EngineError::AccessDenied)?;
            }
        }
        let lowered = lower(plan, &self.catalog, ctx.max_rows).map_err(map_lower)?;
        let q = PgQuery { sql: format!("EXPLAIN (ANALYZE, BUFFERS, SETTINGS) {}", lowered.sql), params: lowered.params.clone(), timeout: ctx.timeout };
        let batches = self.connector.query(&q, &ctx.cancel).map_err(|e| map_error(e, ctx.timeout.as_millis() as u64))?;
        let mut lines = Vec::new();
        for b in &batches {
            if let Some(col) = b.columns().first().and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>()) {
                lines.extend((0..col.len()).filter(|i| !col.is_null(*i)).map(|i| col.value(i).to_string()));
            }
        }
        Ok(lines)
    }
}

/// Map a driver error to the engine error taxonomy. The messages carry the words the planner's failure classifier keys on
/// ("timeout", "cancel", "not available", ...) so health and fallback treat PostGIS exactly like any other engine.
fn map_error(e: PgError, timeout_ms: u64) -> EngineError {
    match e.kind {
        PgErrorKind::Timeout => EngineError::Timeout { limit_ms: timeout_ms },
        PgErrorKind::Cancelled => EngineError::Cancelled,
        PgErrorKind::Connection => EngineError::Engine(format!("postgis connection failed (transient): {}", e.message)),
        PgErrorKind::Capability => EngineError::Engine(format!("capability: a PostGIS function or type is not available: {}", e.message)),
        PgErrorKind::Relation => EngineError::Engine(format!("capability: mapped relation or column is not available in the database: {}", e.message)),
        PgErrorKind::Other => EngineError::Engine(format!("postgis error: {}", e.message)),
    }
}

fn map_lower(e: LowerError) -> EngineError {
    match e {
        LowerError::Invalid(m) => EngineError::InvalidPlan(m),
        LowerError::Unsupported(m) => EngineError::Unsupported(m),
        LowerError::NotResident(m) => EngineError::Unsupported(format!("source not resident in PostGIS: {m}")),
    }
}

impl EngineDescriptor for PostGisEngine {
    fn id(&self) -> EngineId {
        EngineId::new(crate::ENGINE_ID)
    }

    /// The capability matrix. An operation missing here makes the planner exclude the engine BEFORE execution; the same list is
    /// asserted against the lowering in the tests so the two cannot drift.
    fn capabilities(&self) -> Capabilities {
        let ops: BTreeSet<OpKind> = [
            OpKind::Source,
            OpKind::Filter,
            OpKind::BboxFilter,
            OpKind::Intersects,
            OpKind::Transform,
            OpKind::SpatialSemiJoin,
            OpKind::Aggregate,
            OpKind::Project,
            OpKind::Limit,
            OpKind::Sort,
        ]
        .into_iter()
        .collect();
        Capabilities {
            kind: EngineKind::Server,
            ops,
            sources: [SourceFormat::GeoParquet].into_iter().collect(),
            limits: EngineLimits { max_input_bytes: None, max_result_rows: Some(limits::DEFAULT_MAX_ROWS) },
            // bbox filtering uses the geometry column and its GiST index (`&&`), not a covering column
            bbox_filter_requires_covering: false,
            location: "PostgreSQL/PostGIS database server".into(),
            notes: vec![
                "only data resident in the database (source locality); importing a file is never a query cost".into(),
                "not supported in v0: Buffer, SpatialJoin (pair-producing), render sampling (HashBucket)".into(),
                "transactions exist in PostgreSQL but are not exposed: the adapter is read-only".into(),
            ],
        }
    }

    fn source_residency(&self, plan: &Plan, _ctx: &ExecutionContext) -> Result<(), String> {
        lower(plan, &self.catalog, 1).map(|_| ()).map_err(|e| e.to_string())
    }

    /// No measurements of this engine exist on this system (no live database was available): the planner uses its stand-in
    /// server model, says so, and never prefers this engine on predicted cost over a measured one (policy R5d).
    fn calibration(&self) -> EngineCalibration {
        EngineCalibration::Unmeasured
    }

    fn estimate(&self, plan: &Plan, _ctx: &ExecutionContext) -> Result<Estimate, EngineError> {
        let mut rows: u64 = 0;
        let mut known = true;
        for n in plan.nodes.values() {
            if let PlanNode::Source { uri, .. } = n {
                match self.catalog.get(uri).and_then(|s| s.row_estimate) {
                    Some(r) => rows = rows.saturating_add(r),
                    None => known = false,
                }
            }
        }
        Ok(Estimate {
            rows_upper_bound: if known { Metric::source_estimated(rows) } else { Metric::unknown() },
            bytes_upper_bound: Metric::unknown(),
        })
    }

    fn explain(&self, plan: &Plan, _ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError> {
        let lowered = lower(plan, &self.catalog, limits::DEFAULT_MAX_ROWS).map_err(map_lower)?;
        let mut steps = vec![ExplainStep {
            label: "residency".into(),
            detail: format!("sources resident in PostGIS: {}", lowered.sources.join(", ")),
        }];
        if !lowered.dropped_columns.is_empty() {
            steps.push(ExplainStep {
                label: "columns".into(),
                detail: format!("not stored in the relation (dropped at the scan): {}", lowered.dropped_columns.join(", ")),
            });
        }
        steps.push(ExplainStep {
            label: "sql".into(),
            detail: format!("{} bind parameter(s), no value in the text:\n{}", lowered.params.len(), lowered.sql),
        });
        Ok(steps)
    }
}

impl ExecutionEngine for PostGisEngine {
    fn execute(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<ExecutionResult, EngineError> {
        let report = validate(plan);
        if !report.is_ok() {
            return Err(EngineError::InvalidPlan(report.errors().map(|e| e.message.clone()).collect::<Vec<_>>().join("; ")));
        }
        // AUTHORIZATION FIRST: every source passes the guard before the catalog is consulted or the database is touched.
        for n in plan.nodes.values() {
            if let PlanNode::Source { uri, .. } = n {
                ctx.guard.authorize(uri).map_err(|_| EngineError::AccessDenied)?;
            }
        }
        let lowered = lower(plan, &self.catalog, ctx.max_rows).map_err(map_lower)?;
        if ctx.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let q = PgQuery { sql: lowered.sql.clone(), params: lowered.params.clone(), timeout: ctx.timeout };
        let t0 = Instant::now();
        let batches = self.connector.query(&q, &ctx.cancel).map_err(|e| map_error(e, ctx.timeout.as_millis() as u64))?;
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let rows: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
        if rows > ctx.max_rows {
            return Err(EngineError::LimitExceeded(format!("result has more than {} rows", ctx.max_rows)));
        }
        let batches = tag_geometry(batches, &lowered.geometry_outputs);
        let mut metrics = ExecutionMetrics::default();
        metrics.elapsed_ms = Metric::engine_observed(elapsed_ms);
        metrics.rows_returned = Metric::engine_observed(rows);
        // Nothing else is invented: PostgreSQL gives no trustworthy bytes-read / peak-memory number through this path (they stay
        // `Unknown`), and the row estimate comes from the catalog, labelled as an estimate.
        let scanned: u64 = lowered.sources.iter().filter_map(|u| self.catalog.get(u).and_then(|s| s.row_estimate)).sum();
        if scanned > 0 {
            metrics.rows_scanned_upper_bound = Metric::source_estimated(scanned);
        }
        metrics.notes.push(format!("engine postgis; {} bind parameter(s)", lowered.params.len()));
        if !lowered.dropped_columns.is_empty() {
            metrics.notes.push(format!("columns not stored in the relation: {}", lowered.dropped_columns.join(", ")));
        }
        Ok(ExecutionResult { batches, metrics })
    }
}
