//! The only thing in this crate that talks to a PostgreSQL server. A trait, so the engine logic (authorization order, lowering,
//! limits, cancellation, error mapping) is testable with a mock and the real driver (`live`) is one replaceable piece.

use crate::probe::ProbeInputs;
use arrow_array::RecordBatch;
use geo_executor::CancelToken;
use std::time::Duration;

/// A bind parameter. The lowered SQL casts every placeholder explicitly (`$1::bigint`, `$2::double precision`, `$3::text`,
/// `$4::boolean`), so the server never has to infer a type from a column and a value never becomes SQL text.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct PgQuery {
    pub sql: String,
    pub params: Vec<Param>,
    /// Server-side statement timeout AND the client-side deadline: the same number as every other engine's execution timeout.
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgErrorKind {
    /// Cannot reach / keep a connection (refused, reset, too many connections, authentication).
    Connection,
    /// `canceling statement due to statement timeout`.
    Timeout,
    /// Cancelled by us (`canceling statement due to user request`).
    Cancelled,
    /// A function, type or extension the SQL needs is not there.
    Capability,
    /// The mapped relation or column does not exist (catalog and database disagree).
    Relation,
    Other,
}

#[derive(Debug, Clone)]
pub struct PgError {
    pub kind: PgErrorKind,
    pub sqlstate: Option<String>,
    pub message: String,
}

impl PgError {
    pub fn new(kind: PgErrorKind, message: impl Into<String>) -> Self {
        PgError { kind, sqlstate: None, message: message.into() }
    }

    /// Classify by SQLSTATE (stable across PostgreSQL versions and locales), not by message text.
    pub fn from_sqlstate(state: &str, message: &str) -> PgError {
        let kind = match state {
            "57014" => {
                if message.to_ascii_lowercase().contains("statement timeout") {
                    PgErrorKind::Timeout
                } else {
                    PgErrorKind::Cancelled
                }
            }
            s if s.starts_with("08") => PgErrorKind::Connection,
            "57P01" | "57P02" | "57P03" | "53300" | "28000" | "28P01" => PgErrorKind::Connection,
            "42883" | "42704" | "0A000" => PgErrorKind::Capability,
            "42P01" | "42703" | "3F000" | "42501" => PgErrorKind::Relation,
            _ => PgErrorKind::Other,
        };
        PgError { kind, sqlstate: Some(state.to_string()), message: message.to_string() }
    }
}

pub trait PgConnector: Send + Sync {
    /// Run one parameterized statement and return its rows as Arrow (geometry columns as WKB `Binary`).
    /// Implementations MUST honour `timeout` on the server (statement timeout) and MUST cancel the running statement on the
    /// server - not just stop waiting - when `cancel` is set.
    fn query(&self, q: &PgQuery, cancel: &CancelToken) -> Result<Vec<RecordBatch>, PgError>;
    /// Facts the runtime probe needs (versions, which required functions exist). Touches system catalogs only, never user data.
    fn probe(&self) -> Result<ProbeInputs, PgError>;
}

/// Stands in when PostGIS is configured but the real driver is not compiled in (`live` feature off): every call fails as a
/// connection problem with the reason, so the engine is visible, unhealthy and explained - never silently absent.
pub struct UnavailableConnector {
    pub reason: String,
}

impl PgConnector for UnavailableConnector {
    fn query(&self, _: &PgQuery, _: &CancelToken) -> Result<Vec<RecordBatch>, PgError> {
        Err(PgError::new(PgErrorKind::Connection, self.reason.clone()))
    }
    fn probe(&self) -> Result<ProbeInputs, PgError> {
        Err(PgError::new(PgErrorKind::Connection, self.reason.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlstates_map_to_the_failure_kinds_the_planner_understands() {
        assert_eq!(PgError::from_sqlstate("57014", "canceling statement due to statement timeout").kind, PgErrorKind::Timeout);
        assert_eq!(PgError::from_sqlstate("57014", "canceling statement due to user request").kind, PgErrorKind::Cancelled);
        assert_eq!(PgError::from_sqlstate("08006", "connection failure").kind, PgErrorKind::Connection);
        assert_eq!(PgError::from_sqlstate("57P01", "terminating connection due to administrator command").kind, PgErrorKind::Connection);
        assert_eq!(PgError::from_sqlstate("42883", "function st_dwithin(...) does not exist").kind, PgErrorKind::Capability);
        assert_eq!(PgError::from_sqlstate("42P01", "relation \"x\" does not exist").kind, PgErrorKind::Relation);
        assert_eq!(PgError::from_sqlstate("23505", "duplicate key").kind, PgErrorKind::Other);
    }
}
