//! The statement watcher: cancels a running statement ON THE SERVER when the context is cancelled or the deadline passes, and goes away at once when the
//! statement finishes.
//!
//! "At once" is the point of this module being separate (pure `std`, no database): a first version polled with `sleep(15 ms)` and `join()`ed the thread
//! after every statement, which added up to one Windows timer tick (~15 ms) to EVERY PostGIS query - 55 % of all measured PostGIS times sat on 16 ms and
//! every earlier cost coefficient was wrong in its fast range (ADR-015 addendum). The structural test below makes that regression impossible to
//! reintroduce quietly: it gives the watcher a poll interval of many seconds and requires `finish` to return in a fraction of one.

use geo_executor::CancelToken;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct Watcher {
    stop: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
    timed_out: Arc<AtomicBool>,
}

/// Start watching. `on_fire` runs (once) when `cancel` is set or `deadline` passes - for PostgreSQL it sends the cancel request.
pub fn spawn(deadline: Instant, cancel: CancelToken, poll: Duration, on_fire: impl FnOnce() + Send + 'static) -> Watcher {
    let (stop, rx) = channel::<()>();
    let timed_out = Arc::new(AtomicBool::new(false));
    let flag = timed_out.clone();
    let handle = std::thread::spawn(move || loop {
        let late = Instant::now() >= deadline;
        if cancel.is_cancelled() || late {
            flag.store(late && !cancel.is_cancelled(), Ordering::SeqCst);
            on_fire();
            return;
        }
        match rx.recv_timeout(poll) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return, // the sender was dropped: the statement finished
        }
    });
    Watcher { stop: Some(stop), handle: Some(handle), timed_out }
}

impl Watcher {
    /// The statement is done: wake the watcher immediately and report whether it fired because of the DEADLINE (not a cancel).
    pub fn finish(mut self) -> bool {
        drop(self.stop.take());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.timed_out.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn finishing_never_waits_for_the_poll_interval() {
        // a poll interval that would make the old sleep+join design take 5 s; the channel design must return in a small fraction of it
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        let w = spawn(Instant::now() + Duration::from_secs(600), CancelToken::default(), Duration::from_secs(5), move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(30)); // let the watcher reach its wait
        let t0 = Instant::now();
        let timed_out = w.finish();
        let took = t0.elapsed();
        assert!(took < Duration::from_millis(500), "finish() took {took:?}: the watcher is waiting for its poll interval again (the +15 ms per query regression)");
        assert!(!timed_out);
        assert_eq!(fired.load(Ordering::SeqCst), 0, "a finished statement is not cancelled");
    }

    #[test]
    fn the_deadline_fires_once_and_is_reported_as_a_timeout() {
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        let w = spawn(Instant::now() + Duration::from_millis(40), CancelToken::default(), Duration::from_millis(5), move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(w.finish(), "a deadline is reported as a timeout");
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_cancel_fires_once_and_is_not_a_timeout() {
        let cancel = CancelToken::default();
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        let w = spawn(Instant::now() + Duration::from_secs(600), cancel.clone(), Duration::from_millis(5), move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        cancel.cancel();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!w.finish(), "a user cancel is not a timeout");
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }
}
