//! Descriptor of the browser execution engine (DuckDB-Wasm).
//!
//! The engine itself lives in the web client and implements the same contract
//! in TypeScript (`web/src/engine/`): same plan, same canonical result digest.
//! Here we only describe it to the planner — capabilities, limits, estimates and
//! a human-readable explanation of what the client will do.
//!
//! First version supports a deliberately small, spatially *cheap* subset:
//! Source, Filter, BboxFilter (through the bbox covering column — the browser has
//! no spatial extension), Aggregate, Project, Limit, Sort. No Intersects, Buffer or
//! SpatialJoin.

pub mod lower;

use geo_executor::{
    Capabilities, EngineDescriptor, EngineError, EngineId, EngineKind, EngineLimits, Estimate,
    ExecutionContext, ExplainStep, Metric,
};
use geo_ir::{OpKind, Plan, SourceFormat};
use geo_source_geoparquet::analyze_plan;
use std::collections::BTreeSet;

pub const ENGINE_ID: &str = "duckdb-wasm";

#[derive(Debug, Clone)]
pub struct BrowserEngineConfig {
    /// Engine-level hard limits (the planner config may add stricter ones).
    pub limits: EngineLimits,
}

impl Default for BrowserEngineConfig {
    fn default() -> Self {
        BrowserEngineConfig { limits: EngineLimits { max_input_bytes: None, max_result_rows: Some(500_000) } }
    }
}

#[derive(Default)]
pub struct DuckDbWasmDescriptor {
    pub config: BrowserEngineConfig,
}

impl EngineDescriptor for DuckDbWasmDescriptor {
    fn id(&self) -> EngineId {
        EngineId::new(ENGINE_ID)
    }

    fn capabilities(&self) -> Capabilities {
        let ops: BTreeSet<OpKind> =
            [OpKind::Source, OpKind::Filter, OpKind::BboxFilter, OpKind::Aggregate, OpKind::Project, OpKind::Limit, OpKind::Sort]
                .into_iter()
                .collect();
        Capabilities {
            kind: EngineKind::Browser,
            ops,
            sources: [SourceFormat::GeoParquet].into_iter().collect(),
            limits: self.config.limits.clone(),
            bbox_filter_requires_covering: true,
            location: "user's browser tab (DuckDB-Wasm)".into(),
            notes: vec![
                "no spatial extension: Intersects / Buffer / SpatialJoin are not supported".into(),
                "Parquet byte ranges are fetched over HTTP from the gateway (authorized sources only)".into(),
            ],
        }
    }

    fn estimate(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Estimate, EngineError> {
        let scans = analyze_plan(plan, ctx.guard.as_ref()).map_err(|e| match e {
            geo_source_geoparquet::SourceError::AccessDenied => EngineError::AccessDenied,
            other => EngineError::Engine(other.to_string()),
        })?;
        Ok(Estimate {
            rows_upper_bound: Metric::source_estimated(scans.iter().map(|s| s.prune.kept_rows).sum()),
            bytes_upper_bound: Metric::source_estimated(scans.iter().map(|s| s.prune.kept_bytes).sum()),
        })
    }

    fn explain(&self, plan: &Plan, ctx: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError> {
        let est = self.estimate(plan, ctx)?;
        Ok(vec![
            ExplainStep {
                label: "Planner selected the browser engine".into(),
                detail: format!(
                    "input upper bound {} bytes, within the browser memory guard",
                    est.bytes_upper_bound.value.unwrap_or(0)
                ),
            },
            ExplainStep {
                label: "Fetch GeoParquet byte ranges".into(),
                detail: "HTTP range requests to the gateway file endpoint (only authorized sources); DuckDB-Wasm reads the row groups it needs".into(),
            },
            ExplainStep {
                label: "Compile logical plan -> DuckDB-Wasm SQL (client side)".into(),
                detail: "the client translates the same PlanGraph; bbox filters use the bbox covering column".into(),
            },
            ExplainStep { label: "Execute in DuckDB-Wasm".into(), detail: "in a Web Worker; cancelled/timed out from the main thread".into() },
            ExplainStep { label: "Result".into(), detail: "Arrow table in the browser; geometry as WKB; rendered by MapLibre".into() },
        ])
    }
}
