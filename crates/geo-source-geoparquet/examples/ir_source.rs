//! Print the IR Source node (`open_source` / `to_ir_source`) of GeoParquet files: `ir_source <file>...` -> `{"<file>": <Source node>, ...}`.
//! Used by `data/scripts/make_public_demo.py` so the static site publishes, for each layer, exactly the Source node the gateway would build from the
//! same file (schema, CRS, statistics) instead of re-deriving it in another language.
use geo_source_geoparquet::GeoParquetSource;
use geo_types::{RootedAccessGuard, SourceAccessGuard};
use std::path::Path;

fn main() {
    let mut out = serde_json::Map::new();
    for arg in std::env::args().skip(1) {
        let path = std::fs::canonicalize(Path::new(&arg)).unwrap_or_else(|e| panic!("{arg}: {e}"));
        let guard = RootedAccessGuard::new([path.parent().unwrap().to_path_buf()]);
        let authorized = guard.authorize(path.to_str().unwrap()).unwrap_or_else(|_| panic!("{arg}: not authorized"));
        let node = GeoParquetSource::open_authorized(&authorized).unwrap_or_else(|e| panic!("{arg}: {e}")).to_ir_source();
        out.insert(arg, serde_json::to_value(&node).unwrap());
    }
    println!("{}", serde_json::to_string(&out).unwrap());
}
