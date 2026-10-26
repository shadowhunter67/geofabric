//! The real PostgreSQL driver (`--features live`). **UNVERIFIED AGAINST A LIVE SERVER**: written and compile-checked in a phase in
//! which no PostgreSQL/PostGIS was available (the user declined the download). Everything above it (lowering, engine logic,
//! error mapping, probe evaluation) is tested with a mock; this file is the one piece whose behaviour on a real server is
//! unproven. The integration tests in `tests/live.rs` are skipped unless `GEOFABRIC_POSTGIS_URL` is set.
//!
//! Properties by construction:
//! * bounded pool (`pool_size` connections, callers wait up to the query timeout, never an unbounded number of connections);
//! * `SET statement_timeout` on the session = the execution timeout of the context (the server stops the statement itself);
//! * cancellation = a watcher thread issues PostgreSQL's cancel request (`pg_cancel_backend` semantics through the wire
//!   protocol) the moment the context is cancelled or the deadline passes - the server stops working, not just the HTTP response;
//! * no TLS (`NoTls`): local development only, as documented; credentials come from the configuration URL and are never logged;
//! * every value is a bind parameter; the statement text is the lowered SQL.

use crate::connector::{Param, PgConnector, PgError, PgErrorKind, PgQuery};
use crate::probe::{ProbeInputs, REQUIRED_FUNCTIONS};
use arrow_array::builder::{BinaryBuilder, BooleanBuilder, Float64Builder, Int64Builder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use geo_executor::CancelToken;
use postgres::types::{ToSql, Type};
use postgres::{Client, NoTls};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

struct PoolState {
    idle: Vec<Client>,
    total: usize,
}

pub struct LiveConnector {
    config: postgres::Config,
    size: usize,
    state: Mutex<PoolState>,
    available: Condvar,
}

fn to_pg_error(e: &postgres::Error) -> PgError {
    match e.as_db_error() {
        Some(db) => PgError::from_sqlstate(db.code().code(), db.message()),
        // no database error = the connection itself failed or broke
        None => PgError::new(PgErrorKind::Connection, e.to_string()),
    }
}

impl LiveConnector {
    pub fn new(url: &str, pool_size: usize, connect_timeout: Duration) -> Result<Self, String> {
        let mut config: postgres::Config = url.parse().map_err(|_| "malformed PostGIS URL".to_string())?;
        config.connect_timeout(connect_timeout);
        Ok(LiveConnector { config, size: pool_size.clamp(1, 32), state: Mutex::new(PoolState { idle: Vec::new(), total: 0 }), available: Condvar::new() })
    }

    fn acquire(&self, wait: Duration) -> Result<Client, PgError> {
        let deadline = Instant::now() + wait;
        let mut st = self.state.lock().map_err(|_| PgError::new(PgErrorKind::Other, "pool poisoned"))?;
        loop {
            if let Some(c) = st.idle.pop() {
                return Ok(c);
            }
            if st.total < self.size {
                st.total += 1;
                drop(st);
                return self.config.connect(NoTls).map_err(|e| {
                    if let Ok(mut s) = self.state.lock() {
                        s.total -= 1;
                    }
                    self.available.notify_one();
                    to_pg_error(&e)
                });
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(PgError::new(PgErrorKind::Timeout, "no database connection became free before the timeout (pool exhausted)"));
            }
            st = self.available.wait_timeout(st, left).map_err(|_| PgError::new(PgErrorKind::Other, "pool poisoned"))?.0;
        }
    }

    fn release(&self, c: Client, healthy: bool) {
        if let Ok(mut st) = self.state.lock() {
            if healthy && !c.is_closed() {
                st.idle.push(c);
            } else {
                st.total -= 1;
            }
        }
        self.available.notify_one();
    }
}

fn to_arrow(rows: &[postgres::Row]) -> Result<Vec<RecordBatch>, PgError> {
    let Some(first) = rows.first() else { return Ok(vec![]) };
    let mut fields = Vec::new();
    let mut arrays: Vec<ArrayRef> = Vec::new();
    for (i, col) in first.columns().iter().enumerate() {
        let name = col.name();
        macro_rules! build {
            ($builder:expr, $t:ty, $dt:expr) => {{
                let mut b = $builder;
                for r in rows {
                    match r.try_get::<_, Option<$t>>(i).map_err(|e| to_pg_error(&e))? {
                        Some(v) => b.append_value(v),
                        None => b.append_null(),
                    }
                }
                fields.push(Field::new(name, $dt, true));
                arrays.push(Arc::new(b.finish()));
            }};
        }
        let ty = col.type_().clone();
        if ty == Type::INT8 {
            build!(Int64Builder::new(), i64, DataType::Int64)
        } else if ty == Type::INT4 {
            let mut b = Int64Builder::new();
            for r in rows {
                match r.try_get::<_, Option<i32>>(i).map_err(|e| to_pg_error(&e))? {
                    Some(v) => b.append_value(v as i64),
                    None => b.append_null(),
                }
            }
            fields.push(Field::new(name, DataType::Int64, true));
            arrays.push(Arc::new(b.finish()));
        } else if ty == Type::INT2 {
            let mut b = Int64Builder::new();
            for r in rows {
                match r.try_get::<_, Option<i16>>(i).map_err(|e| to_pg_error(&e))? {
                    Some(v) => b.append_value(v as i64),
                    None => b.append_null(),
                }
            }
            fields.push(Field::new(name, DataType::Int64, true));
            arrays.push(Arc::new(b.finish()));
        } else if ty == Type::FLOAT8 {
            build!(Float64Builder::new(), f64, DataType::Float64)
        } else if ty == Type::FLOAT4 {
            let mut b = Float64Builder::new();
            for r in rows {
                match r.try_get::<_, Option<f32>>(i).map_err(|e| to_pg_error(&e))? {
                    Some(v) => b.append_value(v as f64),
                    None => b.append_null(),
                }
            }
            fields.push(Field::new(name, DataType::Float64, true));
            arrays.push(Arc::new(b.finish()));
        } else if ty == Type::BOOL {
            build!(BooleanBuilder::new(), bool, DataType::Boolean)
        } else if ty == Type::TEXT || ty == Type::VARCHAR || ty == Type::BPCHAR || ty == Type::NAME {
            build!(StringBuilder::new(), String, DataType::Utf8)
        } else if ty == Type::BYTEA {
            build!(BinaryBuilder::new(), Vec<u8>, DataType::Binary)
        } else {
            return Err(PgError::new(PgErrorKind::Other, format!("unsupported PostgreSQL column type {} for column {name}", ty.name())));
        }
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map(|b| vec![b]).map_err(|e| PgError::new(PgErrorKind::Other, e.to_string()))
}

impl PgConnector for LiveConnector {
    fn query(&self, q: &PgQuery, cancel: &CancelToken) -> Result<Vec<RecordBatch>, PgError> {
        let mut client = self.acquire(q.timeout)?;
        let ms = q.timeout.as_millis().max(1);
        if let Err(e) = client.batch_execute(&format!("SET statement_timeout = {ms}")) {
            self.release(client, false);
            return Err(to_pg_error(&e));
        }
        // watcher: cancel the statement ON THE SERVER when the context is cancelled or the deadline passes
        let token = client.cancel_token();
        // The watcher (crate::watch) waits on a channel, never on a sleep loop: when the statement finishes it wakes AT ONCE. A sleep(15 ms) + join()
        // here once added up to a Windows timer tick to every query (ADR-015 addendum); watch.rs carries the structural test that keeps it out.
        let watcher = crate::watch::spawn(Instant::now() + q.timeout, cancel.clone(), Duration::from_millis(15), move || {
            let _ = token.cancel_query(NoTls);
        });
        let values: Vec<Box<dyn ToSql + Sync>> = q
            .params
            .iter()
            .map(|p| -> Box<dyn ToSql + Sync> {
                match p {
                    Param::Int(v) => Box::new(*v),
                    Param::Float(v) => Box::new(*v),
                    Param::Text(v) => Box::new(v.clone()),
                    Param::Bool(v) => Box::new(*v),
                }
            })
            .collect();
        let refs: Vec<&(dyn ToSql + Sync)> = values.iter().map(|b| b.as_ref()).collect();
        let result = client.query(q.sql.as_str(), &refs);
        let timed_out = watcher.finish();
        match result {
            Ok(rows) => {
                self.release(client, true);
                to_arrow(&rows)
            }
            Err(e) => {
                let mut err = to_pg_error(&e);
                if timed_out && err.kind == PgErrorKind::Cancelled {
                    err.kind = PgErrorKind::Timeout;
                }
                let healthy = err.kind != PgErrorKind::Connection;
                self.release(client, healthy);
                Err(err)
            }
        }
    }

    fn probe(&self) -> Result<ProbeInputs, PgError> {
        let mut client = self.acquire(Duration::from_secs(5))?;
        let version: String = client.query_one("SELECT version()", &[]).map_err(|e| to_pg_error(&e))?.get(0);
        // postgis_full_version() raises "function does not exist" when the extension is absent: that is an answer, not an error
        let postgis = match client.query_one("SELECT postgis_full_version()", &[]) {
            Ok(r) => r.get::<_, String>(0),
            Err(e) => match e.as_db_error().map(|d| d.code().code().to_string()) {
                Some(c) if c == "42883" => String::new(),
                _ => return Err(to_pg_error(&e)),
            },
        };
        let wanted: Vec<String> = REQUIRED_FUNCTIONS.iter().map(|s| s.to_string()).collect();
        let found = client
            .query("SELECT DISTINCT lower(proname) FROM pg_proc WHERE lower(proname) = ANY($1)", &[&wanted])
            .map_err(|e| to_pg_error(&e))?
            .iter()
            .map(|r| r.get::<_, String>(0))
            .collect();
        self.release(client, true);
        Ok(ProbeInputs { postgres_version: version, postgis_full_version: postgis, functions_found: found })
    }
}
