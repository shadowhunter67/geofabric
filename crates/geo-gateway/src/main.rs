//! geo-gateway binary: `geo-gateway --data <dir> [--port 8787] [--host 127.0.0.1]
//! [--web web/dist] [--timeout-secs 30]`

use geo_gateway::{api_router, AppState, GatewayConfig};
use geo_runtime::Runtime;
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::{ServeDir, ServeFile};

fn arg(args: &[String], name: &str, default: &str) -> String {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = arg(&args, "--data", "benchmarks/data");
    let port = arg(&args, "--port", "8787");
    let host = arg(&args, "--host", "127.0.0.1");
    let web = arg(&args, "--web", "web/dist");
    let mut config = GatewayConfig::default();
    if let Ok(secs) = arg(&args, "--timeout-secs", "").parse::<u64>() {
        config.query_timeout = Duration::from_secs(secs);
    }
    let data_dir = std::fs::canonicalize(&data).unwrap_or_else(|e| panic!("data dir '{data}': {e}"));
    let rt = tokio::task::spawn_blocking(Runtime::with_duckdb).await.unwrap();
    let state = Arc::new(AppState::new(rt, data_dir, config.clone()));
    let index = ServeFile::new(format!("{web}/index.html"));
    let app = api_router(state).fallback_service(ServeDir::new(&web).fallback(index));
    let addr = format!("{host}:{port}");
    let l = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("geo-gateway listening on http://{addr} (data: {data}, web: {web}, query timeout: {:?})", config.query_timeout);
    axum::serve(l, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}
