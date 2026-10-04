//! Counters for profiling the render loop (feature `frame-times`).
//!
//! The window's renderer prints one line per frame (`crate::renderer`).
//! These counters add how much of the time between two frames the chrome
//! itself took: its polls, its render passes, and its event handling. Three
//! more only count: the redraws the pages and the chrome asked for, and the
//! cursors sent to the window (ledger L9, H8). Without the feature a counter
//! is empty, and a span or a count costs nothing.

#[cfg(feature = "frame-times")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "frame-times")]
use std::time::Instant;

/// Calls and nanoseconds since the counter was last taken.
pub struct Counter {
    #[cfg(feature = "frame-times")]
    calls: AtomicU64,
    #[cfg(feature = "frame-times")]
    nanos: AtomicU64,
}

impl Counter {
    const fn new() -> Counter {
        Counter {
            #[cfg(feature = "frame-times")]
            calls: AtomicU64::new(0),
            #[cfg(feature = "frame-times")]
            nanos: AtomicU64::new(0),
        }
    }

    /// Time the rest of the enclosing scope into this counter.
    #[inline]
    pub fn span(&'static self) -> Span {
        Span {
            #[cfg(feature = "frame-times")]
            counter: self,
            #[cfg(feature = "frame-times")]
            started: Instant::now(),
        }
    }

    /// Count one call without timing it.
    #[inline]
    pub fn count(&'static self) {
        #[cfg(feature = "frame-times")]
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    /// The calls and milliseconds since the last take, resetting both.
    #[cfg(feature = "frame-times")]
    pub fn take(&self) -> (u64, f64) {
        (
            self.calls.swap(0, Ordering::Relaxed),
            self.nanos.swap(0, Ordering::Relaxed) as f64 / 1e6,
        )
    }
}

/// A timed scope: its duration is added to its counter when it is dropped.
#[must_use = "a span measures until it is dropped; bind it to a variable"]
pub struct Span {
    #[cfg(feature = "frame-times")]
    counter: &'static Counter,
    #[cfg(feature = "frame-times")]
    started: Instant,
}

#[cfg(feature = "frame-times")]
impl Drop for Span {
    fn drop(&mut self) {
        let nanos = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.counter.calls.fetch_add(1, Ordering::Relaxed);
        self.counter.nanos.fetch_add(nanos, Ordering::Relaxed);
    }
}

/// `ChromeDocument::poll`.
pub static POLL: Counter = Counter::new();
/// `ChromeDocument::render`, from polls and from actions.
pub static RENDER: Counter = Counter::new();
/// `ChromeDocument::handle_ui_event`.
pub static EVENT: Counter = Counter::new();
/// Redraw requests from pages (`cursor::PageShell`), counted only.
pub static PAGE_REDRAW: Counter = Counter::new();
/// Redraw requests from the chrome's own document (`cursor::WindowShell`),
/// counted only.
pub static CHROME_REDRAW: Counter = Counter::new();
/// Cursors sent to the window (`cursor::WindowShell::apply`), counted only.
pub static CURSOR_SET: Counter = Counter::new();

#[cfg(all(test, feature = "frame-times"))]
mod tests {
    use super::*;

    static SCRATCH: Counter = Counter::new();

    #[test]
    fn spans_add_up_and_take_resets() {
        {
            let _span = SCRATCH.span();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        {
            let _span = SCRATCH.span();
        }
        let (calls, millis) = SCRATCH.take();
        assert_eq!(calls, 2);
        assert!(millis >= 2.0, "{millis} ms");
        assert_eq!(SCRATCH.take(), (0, 0.0), "taken and reset");
    }

    static COUNTED: Counter = Counter::new();

    #[test]
    fn counts_add_calls_without_time() {
        COUNTED.count();
        COUNTED.count();
        assert_eq!(COUNTED.take(), (2, 0.0));
        assert_eq!(COUNTED.take(), (0, 0.0), "taken and reset");
    }
}
