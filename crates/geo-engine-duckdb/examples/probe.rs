//! Build/link probe for ADR-006: prints capabilities and the DuckDB library version.
use geo_executor::ExecutionEngine;
fn main() {
    let e = geo_engine_duckdb::DuckDbEngine::new(Default::default());
    let c = e.capabilities();
    println!("engine={} spatial={} ops={:?}", e.id(), e.spatial_available(), c.ops);
    let conn = duckdb::Connection::open_in_memory().unwrap();
    let v: String = conn.query_row("select version()", [], |r| r.get(0)).unwrap();
    println!("duckdb {v}");
}
