//! Compatibility wrapper: the browser-dialect lowering now lives in `geo-lowering` (RFC-004), where its semantics are shared with the other
//! engines and where it also builds for WebAssembly (the page runs that same code; there is no TypeScript compiler of the plan any more).
//! This module keeps the old `lower_browser` signature and error type for the planner-side crate and the golden fixtures.

use geo_executor::EngineError;
use geo_ir::Plan;
use geo_lowering::LowerError;
use std::collections::BTreeMap;

pub use geo_lowering::browser::BrowserSource;

fn to_engine(e: LowerError) -> EngineError {
    match e {
        LowerError::Invalid(m) => EngineError::InvalidPlan(m),
        LowerError::Unsupported(m) => EngineError::Unsupported(m),
    }
}

/// Lower `plan` for DuckDB-Wasm. Returns the SQL and the source URIs it reads; source `i` is file `gf_src_{i}.parquet`.
pub fn lower_browser(plan: &Plan, sources: &BTreeMap<String, BrowserSource>, max_rows: u64) -> Result<(String, Vec<String>), EngineError> {
    geo_lowering::browser::lower_browser(plan, sources, max_rows).map_err(to_engine)
}
