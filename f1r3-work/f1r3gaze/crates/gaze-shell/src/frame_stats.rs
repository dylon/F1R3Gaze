//! Counters for profiling the render loop (feature `frame-times`).
//!
//! The window's renderer prints one line per frame (`crate::renderer`).
//! These counters add how much of the time between two frames the chrome
//! itself took: its polls, its render passes, and its event handling. Three
//! more only count: the redraws the pages and the chrome asked for, and the
//! cursors sent to the window (ledger L9, H8). One more times the window's
//! readings, each of which writes `window.json` if it changed, and also
//! prints each of them on a line of its own with its wall-clock time
//! ([`WindowSave`]; storage ledger S13, part 2). Without the feature a
//! counter is empty, and a span, a count or a reading costs nothing.

#[cfg(feature = "frame-times")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "frame-times")]
use std::time::{Duration, Instant};

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

    /// Count one call that took `took`.
    #[cfg(feature = "frame-times")]
    fn add(&self, took: Duration) {
        let nanos = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
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
        // Was the body of Counter::add, which WindowSave now shares.
        // let nanos = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        // self.counter.calls.fetch_add(1, Ordering::Relaxed);
        // self.counter.nanos.fetch_add(nanos, Ordering::Relaxed);
        self.counter.add(self.started.elapsed());
    }
}

/// Wall-clock milliseconds since the Unix epoch, to line the lines printed
/// here up with logs taken outside the process: the GPU's clocks (ledger
/// L9, H10), the resize bench's sweep, and inotify's view of `window.json`
/// (storage ledger S13, part 2).
#[cfg(feature = "frame-times")]
pub fn wall_clock_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0)
}

/// One reading of the window (`ChromeApplication::save_window`), timed
/// into [`WINDOW_SAVE`] like a span and, when it ends, printed on stdout as
/// a line of its own:
///
/// ```text
/// window_save epoch=<wall-clock ms> took=<ms> changed=<0 or 1>
/// ```
///
/// `changed` says whether the state differed from what `window.json` held,
/// so that it was written (a write that failed is tried again at the next
/// reading: `Keeper::save_with`). The frame line counts the readings made
/// since the frame before it, so a reading made while the window sits still
/// shows on the next frame, however much later; only this line says when it
/// happened. The resize bench needs that to tell a reading made during a
/// sweep from one made before it (storage ledger S13, part 2).
#[must_use = "a reading is counted and printed when it ends; call `end`"]
pub struct WindowSave {
    #[cfg(feature = "frame-times")]
    started: Instant,
}

impl WindowSave {
    /// A reading starts.
    #[inline]
    pub fn start() -> WindowSave {
        WindowSave {
            #[cfg(feature = "frame-times")]
            started: Instant::now(),
        }
    }

    /// The reading ends; `changed`: whether `window.json` was written.
    #[inline]
    #[cfg_attr(not(feature = "frame-times"), allow(unused_variables))]
    pub fn end(self, changed: bool) {
        #[cfg(feature = "frame-times")]
        {
            use std::io::Write as _;
            let took = self.started.elapsed();
            WINDOW_SAVE.add(took);
            let line = window_save_line(wall_clock_ms(), took, changed);
            // A full or closed pipe must not take the window down.
            let _ = writeln!(std::io::stdout().lock(), "{line}");
        }
    }
}

/// The line a reading prints: `scripts/resize-bench.sh` reads its `epoch`,
/// `took` and `changed` (its `fold_saves`).
#[cfg(feature = "frame-times")]
fn window_save_line(epoch_ms: u128, took: Duration, changed: bool) -> String {
    format!(
        "window_save epoch={epoch_ms} took={:.2} changed={}",
        took.as_secs_f64() * 1e3,
        u8::from(changed)
    )
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
/// `ChromeApplication::save_window`: the window read, and `window.json`
/// written if it changed, through [`WindowSave`] (storage ledger S13,
/// part 2).
pub static WINDOW_SAVE: Counter = Counter::new();

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

    #[test]
    fn a_reading_is_counted_like_a_span() {
        // The only test that touches WINDOW_SAVE, so nothing else counts
        // into it meanwhile.
        let _ = WINDOW_SAVE.take();
        let reading = WindowSave::start();
        std::thread::sleep(std::time::Duration::from_millis(2));
        reading.end(true);
        WindowSave::start().end(false);
        let (calls, millis) = WINDOW_SAVE.take();
        assert_eq!(calls, 2);
        assert!(millis >= 2.0, "{millis} ms");
        assert_eq!(WINDOW_SAVE.take(), (0, 0.0), "taken and reset");
    }

    #[test]
    fn the_reading_line_says_when_how_long_and_whether_it_wrote() {
        assert_eq!(
            window_save_line(1_791_273_183_810, Duration::from_micros(1_110), true),
            "window_save epoch=1791273183810 took=1.11 changed=1"
        );
        assert_eq!(
            window_save_line(1_791_273_185_428, Duration::from_micros(350), false),
            "window_save epoch=1791273185428 took=0.35 changed=0"
        );
    }

    #[test]
    fn the_wall_clock_is_in_milliseconds_since_the_epoch() {
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_millis();
        let now = wall_clock_ms();
        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_millis();
        assert!((before..=after).contains(&now), "{before} <= {now} <= {after}");
    }
}
