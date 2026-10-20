//! Configuration. PostGIS is optional: no configuration (or `enabled = false`) means no engine, no startup error, no probe.

use crate::catalog::PostGisSource;
use std::time::Duration;

/// Connection + catalog settings. The URL carries credentials and is therefore never printed (`Debug` redacts it).
#[derive(Clone)]
pub struct PostGisConfig {
    pub enabled: bool,
    /// `postgres://user:password@127.0.0.1:5433/geofabric` - a local development URL. Never logged.
    pub url: Option<String>,
    /// The statement timeout is the execution timeout of the context; this is only the connect timeout.
    pub connect_timeout: Duration,
    /// Upper bound of simultaneous connections (a bounded pool, never one connection per query).
    pub pool_size: usize,
    pub sources: Vec<PostGisSource>,
}

impl Default for PostGisConfig {
    fn default() -> Self {
        PostGisConfig { enabled: false, url: None, connect_timeout: Duration::from_secs(5), pool_size: 4, sources: Vec::new() }
    }
}

impl std::fmt::Debug for PostGisConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostGisConfig")
            .field("enabled", &self.enabled)
            .field("url", &self.url.as_ref().map(|_| "<redacted>"))
            .field("connect_timeout", &self.connect_timeout)
            .field("pool_size", &self.pool_size)
            .field("sources", &self.sources.len())
            .finish()
    }
}

/// Environment variables (the documented convention, `docs/postgis.md`).
pub const ENV_ENABLED: &str = "GEOFABRIC_POSTGIS_ENABLED";
pub const ENV_URL: &str = "GEOFABRIC_POSTGIS_URL";
pub const ENV_SOURCES: &str = "GEOFABRIC_POSTGIS_SOURCES";
pub const ENV_POOL: &str = "GEOFABRIC_POSTGIS_POOL";

impl PostGisConfig {
    /// Read the configuration from the environment. A missing or false `GEOFABRIC_POSTGIS_ENABLED` yields a disabled config
    /// (the caller registers nothing). A malformed sources file is an error (a half-configured database must not look healthy).
    pub fn from_env() -> Result<PostGisConfig, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let enabled = matches!(get(ENV_ENABLED).as_deref(), Some("1") | Some("true") | Some("TRUE") | Some("yes"));
        if !enabled {
            return Ok(PostGisConfig::default());
        }
        let url = get(ENV_URL).ok_or_else(|| format!("{ENV_ENABLED} is set but {ENV_URL} is missing"))?;
        let sources = match get(ENV_SOURCES) {
            Some(path) => PostGisSource::load_file(std::path::Path::new(&path))?,
            None => Vec::new(),
        };
        let pool_size = get(ENV_POOL).and_then(|v| v.parse::<usize>().ok()).unwrap_or(4).clamp(1, 32);
        Ok(PostGisConfig { enabled: true, url: Some(url), connect_timeout: Duration::from_secs(5), pool_size, sources })
    }

    pub fn is_usable(&self) -> bool {
        self.enabled && self.url.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_and_the_url_is_never_printed() {
        let c = PostGisConfig::default();
        assert!(!c.is_usable());
        let c = PostGisConfig { enabled: true, url: Some("postgres://u:secret-pw@127.0.0.1:5433/db".into()), ..Default::default() };
        let shown = format!("{c:?}");
        assert!(!shown.contains("secret-pw") && !shown.contains("postgres://"), "{shown}");
        assert!(shown.contains("<redacted>"));
    }
}
