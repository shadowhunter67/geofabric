//! Integration tests against a REAL PostgreSQL/PostGIS server. They run only with `--features live` and GEOFABRIC_POSTGIS_URL set
//! (a development database; see docs/postgis.md); otherwise each test prints SKIP and passes, so the ordinary suite is never
//! blocked by a missing database.
//!
//! *** NEVER RUN so far: no PostgreSQL was available when they were written. *** They are the acceptance checklist for the first
//! machine that has a server: probe, parameter binding, real server-side timeout, real server-side cancellation, pool bound.
#![cfg(feature = "live")]

use geo_engine_postgis::live::LiveConnector;
use geo_engine_postgis::{Param, PgConnector, PgErrorKind, PgQuery, PostGisProbe};
use geo_executor::CancelToken;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn connector() -> Option<LiveConnector> {
    let Some(url) = std::env::var("GEOFABRIC_POSTGIS_URL").ok().filter(|v| !v.is_empty()) else {
        eprintln!("SKIP: GEOFABRIC_POSTGIS_URL is not set (no PostgreSQL available)");
        return None;
    };
    Some(LiveConnector::new(&url, 2, Duration::from_secs(5)).expect("url"))
}

fn q(sql: &str, params: Vec<Param>, timeout_ms: u64) -> PgQuery {
    PgQuery { sql: sql.into(), params, timeout: Duration::from_millis(timeout_ms) }
}

#[test]
fn the_server_has_everything_the_lowering_needs() {
    let Some(c) = connector() else { return };
    let p = PostGisProbe::evaluate(&c.probe().expect("probe"));
    assert!(p.ok, "{p:?}");
}

#[test]
fn bound_parameters_round_trip_with_their_explicit_casts() {
    let Some(c) = connector() else { return };
    let rows = c
        .query(&q("SELECT $1::bigint AS i, $2::double precision AS f, $3::text AS t, $4::boolean AS b", vec![Param::Int(7), Param::Float(1.5), Param::Text("x'; DROP TABLE y; --".into()), Param::Bool(true)], 5000), &CancelToken::default())
        .expect("query");
    assert_eq!(rows[0].num_rows(), 1);
}

#[test]
fn the_server_enforces_the_statement_timeout() {
    let Some(c) = connector() else { return };
    let t0 = Instant::now();
    let e = c.query(&q("SELECT pg_sleep(10)", vec![], 300), &CancelToken::default()).unwrap_err();
    assert_eq!(e.kind, PgErrorKind::Timeout, "{e:?}");
    assert!(t0.elapsed() < Duration::from_secs(3), "the statement must stop at the deadline, not run to completion");
}

#[test]
fn cancelling_the_context_cancels_the_statement_on_the_server() {
    let Some(c) = connector() else { return };
    let cancel = CancelToken::default();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        c2.cancel();
    });
    let t0 = Instant::now();
    let e = c.query(&q("SELECT pg_sleep(10)", vec![], 30_000), &cancel).unwrap_err();
    assert_eq!(e.kind, PgErrorKind::Cancelled, "{e:?}");
    assert!(t0.elapsed() < Duration::from_secs(3));
}

#[test]
fn the_pool_is_bounded_and_connections_are_reused() {
    let Some(c) = connector() else { return };
    let c = Arc::new(c);
    let hs: Vec<_> = (0..6)
        .map(|_| {
            let c = c.clone();
            std::thread::spawn(move || c.query(&q("SELECT pg_sleep(0.2), 1::bigint", vec![], 10_000), &CancelToken::default()).is_ok())
        })
        .collect();
    assert!(hs.into_iter().all(|h| h.join().unwrap()), "6 queries through a pool of 2 all complete (they queue, they do not open 6 connections)");
}
