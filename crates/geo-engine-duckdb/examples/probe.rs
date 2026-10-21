//! Build/link probe for ADR-006: prints capabilities and the DuckDB library version.
use geo_executor::EngineDescriptor;
fn main() {
    let e = geo_engine_duckdb::DuckDbEngine::new(Default::default());
    let c = e.capabilities();
    println!("engine={} spatial={} spatial_error={:?} ops={:?}", e.id(), e.spatial_available(), e.spatial_error(), c.ops);
    let conn = duckdb::Connection::open_in_memory().unwrap();
    let v: String = conn.query_row("select version()", [], |r| r.get(0)).unwrap();
    println!("duckdb {v}");
}
