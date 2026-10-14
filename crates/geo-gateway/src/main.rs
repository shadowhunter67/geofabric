//! geo-gateway binary: `geo-gateway --data <dir> [--port 8787] [--host 127.0.0.1]
//! [--web web/dist] [--timeout-secs 30] [--network LOCAL|FAST_LAN|GOOD_BROADBAND|MOBILE|HIGH_LATENCY]
//! [--simulate-network <profile>] [--telemetry <file.jsonl>] [--no-learn]`

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
    let mut rt = tokio::task::spawn_blocking(Runtime::with_duckdb_and_browser).await.unwrap();
    let mut pcfg = rt.planner_config().clone();
    if args.iter().any(|a| a == "--no-browser") {
        pcfg.allow_browser = false;
    }
    if let Ok(v) = arg(&args, "--browser-max-mb", "").parse::<u64>() {
        pcfg.browser_max_input_bytes = Some(v << 20);
    }
    if let Ok(v) = arg(&args, "--browser-max-rows", "").parse::<u64>() {
        pcfg.browser_max_result_rows = v;
    }
    rt.set_planner_config(pcfg);
    // planner knowledge: past telemetry (calibration), whether to keep learning, the network profile
    let planner_state = rt.state().clone();
    if let Some(profile) = geo_planner::NetworkProfile::by_name(&arg(&args, "--network", "LOCAL")) {
        planner_state.set_network(profile);
    }
    let tele = arg(&args, "--telemetry", "");
    if !tele.is_empty() {
        let path = std::path::PathBuf::from(&tele);
        if path.exists() {
            match planner_state.load_telemetry(&path) {
                Ok(n) => eprintln!("loaded {n} telemetry observations from {tele}"),
                Err(e) => eprintln!("telemetry {tele}: {e}"),
            }
        }
        planner_state.set_sink(Some(path));
    }
    if args.iter().any(|a| a == "--no-learn") {
        planner_state.set_learning(false);
        eprintln!("planner calibration is frozen (--no-learn)");
    }
    let mut app_state = AppState::new(rt, data_dir, config.clone());
    let sim = arg(&args, "--simulate-network", "");
    if !sim.is_empty() {
        let profile = geo_planner::NetworkProfile::by_name(&sim).unwrap_or_else(|| panic!("unknown network profile '{sim}'"));
        eprintln!("SIMULATING network profile {} (rtt {} ms, {} Mbps down): every /api response is delayed accordingly", profile.name, profile.rtt_ms, profile.down_mbps);
        app_state = app_state.with_simulated_network(profile);
    }
    let state = Arc::new(app_state);
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
