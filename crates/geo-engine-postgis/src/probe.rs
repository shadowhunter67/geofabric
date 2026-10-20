//! Runtime probe for a PostGIS server: can it run the SQL this adapter generates?
//!
//! The probe queries system catalogs only (`version()`, `postgis_full_version()`, `pg_proc`) - never a user relation - so it can
//! run before any source is authorized. Evaluating the facts is a pure function (tested without a server).

use serde::Serialize;

/// Functions the lowering emits. A server missing one cannot run (some) plans: the check names it.
pub const REQUIRED_FUNCTIONS: &[&str] =
    &["st_makeenvelope", "st_asbinary", "st_intersects", "st_dwithin", "st_transform", "st_geomfromtext", "st_srid"];

pub const MIN_POSTGRES_MAJOR: u32 = 14;
pub const MIN_POSTGIS: (u32, u32) = (3, 3);

/// Raw facts a connector reads from the server.
#[derive(Debug, Clone, Default)]
pub struct ProbeInputs {
    /// `SELECT version()`, e.g. `PostgreSQL 16.9, compiled by ...`
    pub postgres_version: String,
    /// `SELECT postgis_full_version()` or empty when the extension is not installed.
    pub postgis_full_version: String,
    /// Lower-cased names from `pg_proc` among [`REQUIRED_FUNCTIONS`] that exist.
    pub functions_found: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostGisProbe {
    pub reachable: bool,
    pub postgres_version: Option<String>,
    pub postgis_version: Option<String>,
    pub ok: bool,
    pub failed_checks: Vec<String>,
}

impl PostGisProbe {
    pub fn unreachable(reason: &str) -> PostGisProbe {
        PostGisProbe { reachable: false, postgres_version: None, postgis_version: None, ok: false, failed_checks: vec![format!("not reachable: {reason}")] }
    }

    pub fn evaluate(i: &ProbeInputs) -> PostGisProbe {
        let mut failed = Vec::new();
        let pg = parse_postgres_major(&i.postgres_version);
        match pg {
            Some((major, _)) if major >= MIN_POSTGRES_MAJOR => {}
            Some((major, _)) => failed.push(format!("PostgreSQL {major} is older than the minimum {MIN_POSTGRES_MAJOR}")),
            None => failed.push("cannot read the PostgreSQL version".into()),
        }
        let gis = parse_postgis_version(&i.postgis_full_version);
        match gis {
            Some(v) if v >= MIN_POSTGIS => {}
            Some((a, b, ..)) => failed.push(format!("PostGIS {a}.{b} is older than the minimum {}.{}", MIN_POSTGIS.0, MIN_POSTGIS.1)),
            None => failed.push("the PostGIS extension is not installed in this database".into()),
        }
        for f in REQUIRED_FUNCTIONS {
            if !i.functions_found.iter().any(|x| x.eq_ignore_ascii_case(f)) {
                failed.push(format!("function {f} is not available"));
            }
        }
        PostGisProbe {
            reachable: true,
            postgres_version: pg.map(|(a, b)| format!("{a}.{b}")),
            postgis_version: gis.map(|(a, b)| format!("{a}.{b}")),
            ok: failed.is_empty(),
            failed_checks: failed,
        }
    }
}

/// `PostgreSQL 16.9 on x86_64-...` -> (16, 9)
fn parse_postgres_major(v: &str) -> Option<(u32, u32)> {
    let rest = v.trim().strip_prefix("PostgreSQL")?.trim_start();
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let mut parts = num.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// `POSTGIS="3.6.2 ..." [EXTENSION] PGSQL="160" GEOS="3.13.0-CAPI-1.19.0" ...` -> (3, 6)
fn parse_postgis_version(v: &str) -> Option<(u32, u32)> {
    let start = v.find("POSTGIS=\"")? + "POSTGIS=\"".len();
    let num: String = v[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let mut parts = num.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> ProbeInputs {
        ProbeInputs {
            postgres_version: "PostgreSQL 16.9, compiled by Visual C++ build 1944, 64-bit".into(),
            postgis_full_version: "POSTGIS=\"3.6.2 e5a2d4d\" [EXTENSION] PGSQL=\"160\" GEOS=\"3.13.0-CAPI-1.19.0\" PROJ=\"9.5.1\"".into(),
            functions_found: REQUIRED_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_complete_server_passes_and_reports_versions() {
        let p = PostGisProbe::evaluate(&good());
        assert!(p.ok && p.failed_checks.is_empty(), "{p:?}");
        assert_eq!((p.postgres_version.as_deref(), p.postgis_version.as_deref()), (Some("16.9"), Some("3.6")));
    }

    #[test]
    fn every_missing_piece_is_named() {
        let mut i = good();
        i.postgres_version = "PostgreSQL 12.4".into();
        i.postgis_full_version = String::new();
        i.functions_found.retain(|f| f != "st_dwithin");
        let p = PostGisProbe::evaluate(&i);
        assert!(!p.ok);
        let all = p.failed_checks.join(" | ");
        assert!(all.contains("older than the minimum 14") && all.contains("not installed") && all.contains("st_dwithin"), "{all}");
        let mut old = good();
        old.postgis_full_version = "POSTGIS=\"3.1.4 abc\" [EXTENSION]".into();
        assert!(PostGisProbe::evaluate(&old).failed_checks[0].contains("older than the minimum 3.3"));
    }

    #[test]
    fn unreachable_is_its_own_state() {
        let p = PostGisProbe::unreachable("connection refused");
        assert!(!p.reachable && !p.ok && p.failed_checks[0].contains("connection refused"));
    }
}
