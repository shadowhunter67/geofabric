//! Integration tests against a REAL PostgreSQL/PostGIS server. They run only with `--features live` and GEOFABRIC_POSTGIS_URL set
//! (a development database; see docs/postgis.md); otherwise each test prints SKIP and passes, so the ordinary suite is never
//! blocked by a missing database.
//!
//! *** NEVER RUN so far: no PostgreSQL was available when they were written. *** They are the acceptance checklist for the first
//! machine that has a server: probe, parameter binding, real server-side timeout, real server-side cancellation, pool bound.
#![cfg(feature = "live")]

use geo_engine_postgis::live::LiveConnector;
use geo_engine_postgis::{Param, PgConnector, PgErrorKind, PgQuery, PostGisProbe};
use postgres as _;
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

/// How many statements of this role that contain `needle` are ACTIVE on the server right now (excluding the observer itself).
/// Observed through a separate plain connection: the brief asks for server-side proof, not "the HTTP call ended".
fn active_statements(needle: &str) -> i64 {
    let url = std::env::var("GEOFABRIC_POSTGIS_URL").unwrap();
    let mut c = postgres::Client::connect(&url, postgres::NoTls).expect("observer connection");
    c.query_one(
        "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() AND state = 'active' AND query LIKE $1",
        &[&format!("%{needle}%")],
    )
    .expect("pg_stat_activity")
    .get(0)
}

fn wait_until_gone(needle: &str) -> bool {
    for _ in 0..40 {
        if active_statements(needle) == 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn the_server_enforces_the_statement_timeout() {
    let Some(c) = connector() else { return };
    let t0 = Instant::now();
    let e = c.query(&q("SELECT 1::bigint AS x FROM pg_sleep(10) /*gf-timeout-probe*/", vec![], 300), &CancelToken::default()).unwrap_err();
    assert_eq!(e.kind, PgErrorKind::Timeout, "{e:?}");
    assert!(t0.elapsed() < Duration::from_secs(3), "the statement must stop at the deadline, not run to completion");
    // server-side proof: the statement is no longer running, and the pooled connection is not poisoned
    assert!(wait_until_gone("gf-timeout-probe"), "a zombie statement is still active on the server");
    let ok = c.query(&q("SELECT 2::bigint AS x", vec![], 5000), &CancelToken::default()).expect("the connection must still work after a timeout");
    assert_eq!(ok[0].num_rows(), 1);
}

#[test]
fn cancelling_the_context_cancels_the_statement_on_the_server() {
    let Some(c) = connector() else { return };
    let cancel = CancelToken::default();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c2.cancel();
    });
    let t0 = Instant::now();
    let runner = std::thread::scope(|sc| {
        let h = sc.spawn(|| c.query(&q("SELECT 1::bigint AS x FROM pg_sleep(10) /*gf-cancel-probe*/", vec![], 30_000), &cancel));
        // while it runs, the server really has the statement
        std::thread::sleep(Duration::from_millis(150));
        let during = active_statements("gf-cancel-probe");
        (h.join().unwrap(), during)
    });
    let (res, during) = runner;
    assert_eq!(during, 1, "the statement was running on the server before the cancellation");
    assert_eq!(res.unwrap_err().kind, PgErrorKind::Cancelled);
    assert!(t0.elapsed() < Duration::from_secs(3));
    assert!(wait_until_gone("gf-cancel-probe"), "a zombie statement is still active on the server after cancellation");
    assert!(c.query(&q("SELECT 3::bigint AS x", vec![], 5000), &CancelToken::default()).is_ok(), "the connection must be reusable (or replaced) after a cancellation");
}

#[test]
fn the_pool_is_bounded_and_connections_are_reused() {
    let Some(c) = connector() else { return };
    let c = Arc::new(c);
    let peak = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observer = {
        let (peak, stop) = (peak.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                peak.fetch_max(active_statements("gf-pool-probe"), std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    let hs: Vec<_> = (0..6)
        .map(|_| {
            let c = c.clone();
            std::thread::spawn(move || c.query(&q("SELECT 1::bigint AS x FROM pg_sleep(0.3) /*gf-pool-probe*/", vec![], 10_000), &CancelToken::default()))
        })
        .collect();
    let results: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    observer.join().unwrap();
    for r in &results {
        assert!(r.is_ok(), "{:?}", r.as_ref().err());
    }
    let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
    assert!(peak <= 2, "the pool of 2 must never run more than 2 statements at once, saw {peak}");
    assert!(peak >= 1, "the observer never saw the statements: the check proved nothing");
}

/// Hostile strings travel as BOUND values: they round-trip byte for byte and never become SQL (a temp table survives them).
#[test]
fn hostile_and_unicode_strings_are_data_never_sql() {
    let Some(c) = connector() else { return };
    let samples = ["x'; DROP TABLE fx_buildings; --", "O'Brien", "\"quoted\"", r"back\slash", "Đường Nguyễn Huệ — Quận 1 ✓", "', (SELECT 1)) --", "%_like", ""];
    for s in samples {
        let rows = c.query(&q("SELECT $1::text AS t, length($1::text)::bigint AS n", vec![Param::Text(s.to_string())], 5000), &CancelToken::default()).expect("query");
        let t = rows[0].column(0).as_any().downcast_ref::<arrow_array::StringArray>().expect("text").value(0).to_string();
        assert_eq!(t, s, "string changed in transit");
    }
    // the fixture table is still there after all of them
    let rows = c.query(&q("SELECT count(*)::bigint AS n FROM geofabric.fx_buildings", vec![], 5000), &CancelToken::default()).expect("fixture table intact");
    assert_eq!(rows[0].num_rows(), 1);
}

/// Failure taxonomy against the real server: each failure lands in its own kind, with the SQLSTATE kept.
#[test]
fn failures_are_classified_by_what_actually_happened() {
    let Some(c) = connector() else { return };
    let ct = CancelToken::default();
    let e = c.query(&q("SELECT * FROM no_such_relation_zz", vec![], 5000), &ct).unwrap_err();
    assert_eq!((e.kind, e.sqlstate.as_deref()), (PgErrorKind::Relation, Some("42P01")), "{e:?}");
    let e = c.query(&q("SELECT no_such_column_zz FROM geofabric.fx_buildings", vec![], 5000), &ct).unwrap_err();
    assert_eq!((e.kind, e.sqlstate.as_deref()), (PgErrorKind::Relation, Some("42703")), "{e:?}");
    let e = c.query(&q("SELECT no_such_function_zz(1)", vec![], 5000), &ct).unwrap_err();
    assert_eq!(e.kind, PgErrorKind::Capability, "{e:?}");
    let e = c.query(&q("SELECT 1/0", vec![], 5000), &ct).unwrap_err();
    assert_eq!(e.kind, PgErrorKind::Other, "{e:?}");
    // a refused connection (nothing listens on port 1) is a Connection failure, and the message must not carry the credential
    let dead = LiveConnector::new("postgresql://nobody:SECRET-PW-123@127.0.0.1:1/none", 1, Duration::from_secs(2)).expect("url parses");
    let e = dead.query(&q("SELECT 1", vec![], 2000), &ct).unwrap_err();
    assert_eq!(e.kind, PgErrorKind::Connection, "{e:?}");
    assert!(!format!("{e:?}").contains("SECRET-PW-123"), "the error leaked the password");
}

/// The runtime account is SELECT-only: a write is refused by the SERVER (42501), whatever the SQL generator would ever emit.
/// Set GEOFABRIC_POSTGIS_RO_URL to the runtime (read-only) account's URL; without it this test is a SKIP and proves nothing.
#[test]
fn the_runtime_account_cannot_write() {
    let Some(url) = std::env::var("GEOFABRIC_POSTGIS_RO_URL").ok().filter(|v| !v.is_empty()) else {
        eprintln!("SKIP: GEOFABRIC_POSTGIS_RO_URL is not set");
        return;
    };
    let c = LiveConnector::new(&url, 1, Duration::from_secs(5)).expect("url");
    let ct = CancelToken::default();
    c.query(&q("SELECT count(*)::bigint AS n FROM geofabric.fx_buildings", vec![], 5000), &ct).expect("the read-only account can read");
    for sql in ["CREATE TABLE gf_should_not_exist (a int)", "DELETE FROM geofabric.fx_buildings", "UPDATE geofabric.fx_buildings SET id = id", "DROP TABLE geofabric.fx_buildings"] {
        let e = c.query(&q(sql, vec![], 5000), &ct).unwrap_err();
        assert_eq!(e.sqlstate.as_deref(), Some("42501"), "{sql}: {e:?}");
    }
}
