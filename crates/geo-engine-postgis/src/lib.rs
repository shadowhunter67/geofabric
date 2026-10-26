//! PostGIS execution adapter (ADR-013): a PostgreSQL/PostGIS database as an execution engine of the same contract as DuckDB.
//!
//! Layers (each testable without a database):
//! * [`config`]   - optional configuration; absent or disabled = the engine simply does not exist;
//! * [`catalog`]  - which logical sources are resident in the database (source locality) and their physical relation;
//! * [`crs`]      - explicit GeoFabric CRS -> SRID mapping (lon/lat axis order stated, never implied);
//! * [`lower`]    - PlanGraph -> parameterized PostgreSQL (identifiers from the catalog only, values as bind parameters);
//! * [`connector`]- the only thing that talks to a server (trait; the real driver is the `live` feature);
//! * [`engine`]   - descriptor + executor: authorization first, residency check, lowering, bounded execution, error mapping;
//! * [`probe`]    - what a server must offer to run our SQL (versions, functions).
//!
//! Nothing in this crate runs unless the operator configures PostGIS; GeoFabric works normally without it.

pub mod catalog;
pub mod config;
pub mod cost;
pub mod connector;
pub mod crs;
pub mod engine;
#[cfg(feature = "live")]
pub mod live;
pub mod lower;
pub mod probe;
pub mod watch;

pub use catalog::{CoveringCols, PgColType, PostGisCatalog, PostGisSource};
pub use config::PostGisConfig;
pub use connector::{Param, PgConnector, PgError, PgErrorKind, PgQuery, UnavailableConnector};
pub use engine::PostGisEngine;
pub use lower::{lower, Lowered};
pub use probe::{PostGisProbe, ProbeInputs};

/// Engine id the planner, health, telemetry and the UI use.
pub const ENGINE_ID: &str = "postgis";

/// Build the engine from configuration. `None` = PostGIS is not configured/enabled (the normal case: GeoFabric runs without it).
/// With the `live` feature the real driver is used; without it a configured engine exists but reports why it cannot connect.
pub fn engine_from_config(cfg: &PostGisConfig) -> Result<Option<PostGisEngine>, String> {
    if !cfg.is_usable() {
        return Ok(None);
    }
    let catalog = PostGisCatalog::new(cfg.sources.clone())?;
    #[cfg(feature = "live")]
    let connector: std::sync::Arc<dyn PgConnector> = {
        let url = cfg.url.as_deref().ok_or("PostGIS URL missing")?;
        std::sync::Arc::new(live::LiveConnector::new(url, cfg.pool_size, cfg.connect_timeout)?)
    };
    #[cfg(not(feature = "live"))]
    let connector: std::sync::Arc<dyn PgConnector> =
        std::sync::Arc::new(UnavailableConnector { reason: "PostGIS is configured but this build has no PostgreSQL driver (build with the `postgis-live` feature)".into() });
    Ok(Some(PostGisEngine::new(catalog, connector)))
}
