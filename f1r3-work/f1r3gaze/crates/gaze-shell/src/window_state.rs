//! The window's size and place, kept in `state/window.json` and restored
//! when the window is made. Pure: the window system's facts come in as
//! values, so every rule is tested without a window.
//!
//! # Units
//!
//! Window systems give positions in different units. Windows and X11 use
//! physical pixels, one space across every monitor. macOS uses points, and
//! winit converts them to physical pixels with the window's own scale,
//! which differs from monitor to monitor; so on macOS positions are kept in
//! points. Wayland gives no positions at all: a window is placed by the
//! compositor. *Desktop units* below are points on macOS and physical
//! pixels elsewhere, and `σ(M)`, the desktop units per logical pixel on
//! monitor `M`, is 1 on macOS and `M`'s scale factor elsewhere.
//!
//! # Restoring
//!
//! [`plan_restore`] keeps the saved place only if the window would be
//! visible there. With the window `W` and a monitor `M`, both rectangles in
//! desktop units, `W` is visible on `M` when
//!
//! - at least `min(W_w, 96·σ(M))` of its width is over `M`, and
//! - its top edge is where its title bar can be grabbed:
//!   `M_y − 16·σ(M) ≤ W_y ≤ M_y + M_h − 48·σ(M)`.
//!
//! A window visible on no monitor is centred on the monitor it was on, or
//! on the primary one. Its size is shrunk to fit that monitor, and the page
//! zoom is kept within `[0.25, 5]`, to hundredths. Where no monitor bounds
//! the size (none is listed; Wayland, whose compositor bounds a new window
//! itself), it is kept within [`MIN_SIZE`] and [`MAX_SIZE`].
//!
//! # Making and following the window
//!
//! `ChromeApplication` (`application.rs`) makes the window from the plan once
//! the event loop can list the monitors, and only then, after Blitz has shown
//! it, restores the zoom, maximizes it and makes it full screen: Blitz makes
//! windows hidden, and a window manager ignores a maximize asked of a hidden
//! window. It reads the window, in [`WindowReading`]s and
//! [`MonitorReading`]s that [`Sample::from_platform`] and
//! [`Monitor::from_platform`] turn into desktop units, when a save is due:
//! 500 ms after the window is made (no event need report its first size),
//! 500 ms after the last change, at least every 3 s while it keeps changing
//! ([`SaveClock`]), before it closes, when it loses the focus it had, and when
//! it enters or leaves full screen. A [`Keeper`] writes `window.json` only
//! when what it holds changed.

use crate::ui_state::{StateError, read_state, state_bytes};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// The version of `window.json` this build writes.
pub const WINDOW_VERSION: u32 = 1;

/// The surface size of a window that has never been saved, in logical
/// pixels.
pub const DEFAULT_SIZE: (f64, f64) = (1280.0, 800.0);

/// The smallest surface size restored, in logical pixels.
pub const MIN_SIZE: (f64, f64) = (320.0, 240.0);

/// The largest surface size restored where no monitor bounds it (none is
/// listed; Wayland, whose compositor bounds it), in logical pixels: wgpu's
/// default largest texture side, far below X11's 16-bit window sizes
/// (winit-x11 `window.rs:638-639` unwraps that conversion, so a larger size
/// from a damaged file would end every start).
pub const MAX_SIZE: (f64, f64) = (8192.0, 8192.0);

/// The page zoom's bounds.
pub const ZOOM_RANGE: (f64, f64) = (0.25, 5.0);

/// Logical pixels of a window's width that must be over a monitor.
const VISIBLE_WIDTH: f64 = 96.0;
/// How far above a monitor's top a window's top edge may be.
const ABOVE_TOP: f64 = 16.0;
/// How far above a monitor's bottom a window's top edge must be.
const ABOVE_BOTTOM: f64 = 48.0;

/// The position Windows reports for a minimized window.
const WINDOWS_MINIMIZED: i32 = -32_000;

/// The unit of positions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    /// macOS points.
    Points,
    /// Physical pixels (Windows, X11).
    Physical,
}

/// The window system the window is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSystem {
    Wayland,
    X11,
    MacOs,
    Windows,
}

impl WindowSystem {
    /// The unit it gives positions in, if it gives positions.
    pub fn units(self) -> Option<Units> {
        match self {
            WindowSystem::Wayland => None,
            WindowSystem::X11 | WindowSystem::Windows => Some(Units::Physical),
            WindowSystem::MacOs => Some(Units::Points),
        }
    }
}

/// The window's size and place when it is neither maximized nor full
/// screen.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Normal {
    /// The surface (content area) size, in logical pixels.
    pub width: f64,
    pub height: f64,
    /// The frame's top-left corner, in `units`; none where the window
    /// system gives no positions.
    #[serde(default)]
    pub position: Option<(i32, i32)>,
    /// From the frame's corner to the surface's, in `units`. macOS places a
    /// window by its surface.
    #[serde(default)]
    pub surface_offset: (i32, i32),
    pub units: Units,
    /// The scale factor of the window when it was saved.
    pub scale: f64,
}

/// The monitor the window was on.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SavedMonitor {
    pub name: Option<String>,
    /// macOS's display UUID.
    pub uuid: Option<String>,
    /// In desktop units.
    pub origin: (i32, i32),
    pub size: (u32, u32),
    pub scale: f64,
}

/// `state/window.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowState {
    pub version: u32,
    pub normal: Option<Normal>,
    pub maximized: bool,
    pub fullscreen: bool,
    pub monitor: Option<SavedMonitor>,
    pub zoom: f64,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            version: WINDOW_VERSION,
            normal: None,
            maximized: false,
            fullscreen: false,
            monitor: None,
            zoom: 1.0,
        }
    }
}

/// `value` if it is a finite number at least `min`, else `fallback`.
fn finite_at_least(value: f64, min: f64, fallback: f64) -> f64 {
    match value.is_finite() && value >= min {
        true => value,
        false => fallback,
    }
}

/// The zoom kept within [`ZOOM_RANGE`]; 1 for a value that is not a number.
pub fn clamp_zoom(zoom: f64) -> f64 {
    match zoom.is_nan() {
        true => 1.0,
        false => zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1),
    }
}

/// The zoom to hundredths. Blitz steps it in tenths and keeps it in an
/// `f32`, which holds 1.2 as 1.2000000476837158: unrounded, a restored zoom
/// would be written again at the first key press.
fn round_zoom(zoom: f64) -> f64 {
    (zoom * 100.0).round() / 100.0
}

/// The zoom to set back when Blitz's keys took the window's zoom out of
/// [`ZOOM_RANGE`] (chrome ledger L15): Ctrl+− subtracts a tenth with no bound
/// (blitz-traits `shell.rs:144-146`), down to nothing and below it, and the
/// viewport's logical size divides by the zoom. None when it is within range.
pub fn zoom_correction(zoom: f64) -> Option<f64> {
    let kept = clamp_zoom(zoom);
    // NaN never equals itself, so it is corrected to 1.
    (kept != zoom).then_some(kept)
}

impl WindowState {
    /// Reads `window.json`.
    pub fn read(bytes: &[u8]) -> Result<WindowState, StateError> {
        read_state::<WindowState>(bytes, WINDOW_VERSION).map(WindowState::sanitized)
    }

    /// This build's version, a zoom within its range, and no normal size
    /// or scale that is not a positive number.
    pub fn sanitized(mut self) -> WindowState {
        self.version = WINDOW_VERSION;
        self.zoom = clamp_zoom(self.zoom);
        if let Some(normal) = &self.normal
            && !(normal.width.is_finite() && normal.height.is_finite() && normal.width >= 1.0 && normal.height >= 1.0)
        {
            self.normal = None;
        }
        if let Some(normal) = &mut self.normal {
            normal.scale = finite_at_least(normal.scale, f64::MIN_POSITIVE, 1.0);
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.scale = finite_at_least(monitor.scale, f64::MIN_POSITIVE, 1.0);
        }
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        state_bytes(self)
    }
}

/// A monitor, as the window system reports it when the window is made.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: Option<String>,
    pub uuid: Option<String>,
    /// In desktop units.
    pub origin: (i32, i32),
    pub size: (u32, u32),
    pub scale: f64,
}

impl Monitor {
    fn saved(&self) -> SavedMonitor {
        SavedMonitor {
            name: self.name.clone(),
            uuid: self.uuid.clone(),
            origin: self.origin,
            size: self.size,
            scale: self.scale,
        }
    }

    /// Desktop units per logical pixel on this monitor.
    fn sigma(&self, units: Units) -> f64 {
        match units {
            Units::Points => 1.0,
            Units::Physical => finite_at_least(self.scale, f64::MIN_POSITIVE, 1.0),
        }
    }
}

/// Full screen, on which monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullScreen {
    /// The monitor at this index of those given to [`plan_restore`].
    On(usize),
    /// Whichever monitor the window is on.
    Current,
}

/// How to make the window.
#[derive(Clone, Debug, PartialEq)]
pub struct RestorePlan {
    /// The surface size, in logical pixels.
    pub size: (f64, f64),
    /// Where to put the window, in desktop units: the frame's corner, or on
    /// macOS the surface's. None leaves it to the window system.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    pub fullscreen: Option<FullScreen>,
    pub zoom: f64,
}

/// The monitor among `monitors` that `saved` was: by macOS display UUID,
/// then by name (several of one name: the one at the same origin), then by
/// origin, then by size.
pub fn find_monitor(saved: &SavedMonitor, monitors: &[Monitor]) -> Option<usize> {
    let by = |matches: &dyn Fn(&Monitor) -> bool| monitors.iter().position(matches);
    let uuid = saved.uuid.as_ref().and_then(|uuid| by(&|m| m.uuid.as_ref() == Some(uuid)));
    let named = || {
        let name = saved.name.as_ref()?;
        by(&|m| m.name.as_ref() == Some(name) && m.origin == saved.origin).or_else(|| by(&|m| m.name.as_ref() == Some(name)))
    };
    uuid.or_else(named)
        .or_else(|| by(&|m| m.origin == saved.origin))
        .or_else(|| by(&|m| m.size == saved.size))
}

/// A rectangle in desktop units, wide enough that no sum overflows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn of(monitor: &Monitor) -> Rect {
        Rect {
            x: f64::from(monitor.origin.0),
            y: f64::from(monitor.origin.1),
            width: f64::from(monitor.size.0),
            height: f64::from(monitor.size.1),
        }
    }
}

/// Whether the window `w` is visible on `monitor`, by the rule in the
/// module documentation.
fn visible_on(w: Rect, monitor: &Monitor, units: Units) -> bool {
    let m = Rect::of(monitor);
    let sigma = monitor.sigma(units);
    let overlap = (w.x + w.width).min(m.x + m.width) - w.x.max(m.x);
    overlap >= w.width.min(VISIBLE_WIDTH * sigma)
        && m.y - ABOVE_TOP * sigma <= w.y
        && w.y <= m.y + m.height - ABOVE_BOTTOM * sigma
}

/// `value` rounded into `i32`, saturating.
fn to_i32(value: f64) -> i32 {
    // A float-to-int `as` cast saturates, and maps NaN to 0.
    value.round() as i32
}

/// `value` rounded into `u32`, saturating (negative and NaN give 0).
fn to_u32(value: f64) -> u32 {
    value.round() as u32
}

/// How to make the window from what was saved, on `monitors` (`primary`
/// one of them, if the system says), under `system`.
pub fn plan_restore(saved: &WindowState, monitors: &[Monitor], primary: Option<usize>, system: WindowSystem) -> RestorePlan {
    let target = saved
        .monitor
        .as_ref()
        .and_then(|m| find_monitor(m, monitors))
        .or(primary.filter(|&i| i < monitors.len()))
        .or(match monitors.is_empty() {
            true => None,
            false => Some(0),
        });
    let (width, height) = saved.normal.as_ref().map_or(DEFAULT_SIZE, |n| (n.width, n.height));
    let units = system.units();
    // Shrink to the target monitor, in logical pixels, and no smaller
    // than MIN_SIZE (unless the monitor itself is smaller).
    let size = match (target, units) {
        (Some(i), Some(units)) => {
            let monitor = &monitors[i];
            let sigma = monitor.sigma(units);
            let fits = (f64::from(monitor.size.0) / sigma, f64::from(monitor.size.1) / sigma);
            (width.min(fits.0).max(MIN_SIZE.0.min(fits.0)), height.min(fits.1).max(MIN_SIZE.1.min(fits.1)))
        }
        // Disabled in step 11 (ledger S13, part 2): it shrank a Wayland window
        // to the output's mode divided by `Monitor.scale`. winit-wayland's
        // monitor scale is the whole-number `wl_output` scale (winit-wayland
        // `output.rs:53-56`), not the fractional one, and the mode is not
        // rotated with the output: a 2560×1440 output at 125 % measured
        // 1280×720, and every larger window shrank to it. The compositor
        // bounds a new window itself (winit-wayland `window/state.rs`,
        // `configure_bounds`).
        // (Some(i), None) => {
        //     // Wayland gives sizes in its own logical pixels.
        //     let monitor = &monitors[i];
        //     let scale = finite_at_least(monitor.scale, f64::MIN_POSITIVE, 1.0);
        //     let fits = (f64::from(monitor.size.0) / scale, f64::from(monitor.size.1) / scale);
        //     (width.min(fits.0).max(MIN_SIZE.0.min(fits.0)), height.min(fits.1).max(MIN_SIZE.1.min(fits.1)))
        // }
        // Disabled in step 11 (ledger S13, part 2): with no monitor listed, a
        // damaged file's huge size reached winit-x11, which converts it to
        // X11's 16-bit sizes with an unwrap.
        // (None, _) => (width.max(MIN_SIZE.0), height.max(MIN_SIZE.1)),
        (Some(_), None) | (None, _) => (width.clamp(MIN_SIZE.0, MAX_SIZE.0), height.clamp(MIN_SIZE.1, MAX_SIZE.1)),
    };
    let position = match (units, saved.normal.as_ref(), target) {
        (Some(units), Some(normal), Some(target)) if normal.units == units => {
            let saved_sigma = match units {
                Units::Points => 1.0,
                Units::Physical => normal.scale,
            };
            let window = normal.position.map(|(x, y)| Rect {
                x: f64::from(x),
                y: f64::from(y),
                width: size.0 * saved_sigma,
                height: size.1 * saved_sigma,
            });
            let surface_offset = match units {
                Units::Points => normal.surface_offset,
                Units::Physical => (0, 0),
            };
            match window {
                Some(w) if monitors.iter().any(|m| visible_on(w, m, units)) => Some((
                    to_i32(w.x + f64::from(surface_offset.0)),
                    to_i32(w.y + f64::from(surface_offset.1)),
                )),
                Some(_) => {
                    // Centre the surface on the target monitor.
                    let monitor = &monitors[target];
                    let sigma = monitor.sigma(units);
                    let m = Rect::of(monitor);
                    Some((
                        to_i32(m.x + (m.width - size.0 * sigma) / 2.0),
                        to_i32(m.y + (m.height - size.1 * sigma) / 2.0),
                    ))
                }
                // Saved where the window system gave no position: leave it
                // to the window system.
                None => None,
            }
        }
        _ => None,
    };
    let fullscreen = match saved.fullscreen {
        false => None,
        true => Some(match saved.monitor.as_ref().and_then(|m| find_monitor(m, monitors)) {
            Some(i) => FullScreen::On(i),
            None => FullScreen::Current,
        }),
    };
    RestorePlan {
        size,
        position,
        maximized: saved.maximized,
        fullscreen,
        zoom: clamp_zoom(saved.zoom),
    }
}

/// What the window system says about the window at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The surface size, in logical pixels.
    pub width: f64,
    pub height: f64,
    /// The frame's corner, in `units`; none where not given.
    pub position: Option<(i32, i32)>,
    /// The surface's corner, in `units`.
    pub surface_position: Option<(i32, i32)>,
    pub units: Units,
    pub scale: f64,
    pub maximized: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    /// The monitor the window is on.
    pub monitor: Option<Monitor>,
}

/// Follows the window and keeps what [`WindowState`] should hold.
#[derive(Clone, Debug)]
pub struct Tracker {
    state: WindowState,
}

impl Tracker {
    pub fn new(saved: WindowState) -> Tracker {
        Tracker { state: saved }
    }

    pub fn state(&self) -> &WindowState {
        &self.state
    }

    /// Takes in a sample; returns whether the state changed. A minimized
    /// window, or a size that is not a positive number, changes nothing:
    /// Windows reports a minimized window at (−32000, −32000) and a zero
    /// size. The normal size and place change only while the window is
    /// neither maximized nor full screen, so leaving either restores them.
    pub fn observe(&mut self, sample: &Sample) -> bool {
        let minimized = sample.minimized || sample.position == Some((WINDOWS_MINIMIZED, WINDOWS_MINIMIZED));
        let degenerate = !(sample.width.is_finite() && sample.height.is_finite() && sample.width >= 1.0 && sample.height >= 1.0);
        if minimized || degenerate {
            return false;
        }
        let before = self.state.clone();
        self.state.maximized = sample.maximized;
        self.state.fullscreen = sample.fullscreen;
        if let Some(monitor) = &sample.monitor {
            self.state.monitor = Some(monitor.saved());
        }
        if !sample.maximized && !sample.fullscreen {
            let surface_offset = match (sample.position, sample.surface_position) {
                (Some(frame), Some(surface)) => (
                    surface.0.saturating_sub(frame.0),
                    surface.1.saturating_sub(frame.1),
                ),
                _ => (0, 0),
            };
            self.state.normal = Some(Normal {
                width: sample.width,
                height: sample.height,
                position: sample.position,
                surface_offset,
                units: sample.units,
                scale: finite_at_least(sample.scale, f64::MIN_POSITIVE, 1.0),
            });
        }
        self.state != before
    }

    /// Records the page zoom, within its range and to hundredths; returns
    /// whether it changed.
    pub fn set_zoom(&mut self, zoom: f64) -> bool {
        // Was `clamp_zoom(zoom)` alone: Blitz's f32 noise rewrote a restored
        // zoom at the first key press (ledger S13, part 2).
        // let zoom = clamp_zoom(zoom);
        let zoom = round_zoom(clamp_zoom(zoom));
        let changed = self.state.zoom != zoom;
        self.state.zoom = zoom;
        changed
    }
}

/// A monitor as winit gives it: its position and current mode in physical
/// pixels (macOS: points times the monitor's scale), and its scale.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorReading {
    pub name: Option<String>,
    /// macOS's display UUID.
    pub uuid: Option<String>,
    pub position: (i32, i32),
    pub mode: (u32, u32),
    pub scale: f64,
}

impl Monitor {
    /// The monitor in desktop units. On macOS winit's values divided by the
    /// monitor's scale are points (winit-appkit `monitor.rs:223-230`);
    /// elsewhere they are kept: physical pixels, or on Wayland values used
    /// for nothing but finding the monitor again.
    pub fn from_platform(reading: MonitorReading, units: Option<Units>) -> Monitor {
        let scale = finite_at_least(reading.scale, f64::MIN_POSITIVE, 1.0);
        let (origin, size) = match units {
            Some(Units::Points) => (
                (to_i32(f64::from(reading.position.0) / scale), to_i32(f64::from(reading.position.1) / scale)),
                (to_u32(f64::from(reading.mode.0) / scale), to_u32(f64::from(reading.mode.1) / scale)),
            ),
            Some(Units::Physical) | None => (reading.position, reading.mode),
        };
        Monitor {
            name: reading.name,
            uuid: reading.uuid,
            origin,
            size,
            scale,
        }
    }
}

/// The window as winit gives it at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowReading {
    /// The surface size, in physical pixels.
    pub surface_size: (u32, u32),
    /// The window's scale factor.
    pub scale: f64,
    /// The frame's corner (`outer_position`), in physical pixels; none where
    /// the window system gives none (Wayland).
    pub frame: Option<(i32, i32)>,
    /// From the frame's corner to the surface's (`surface_position`), in
    /// physical pixels.
    pub inset: (i32, i32),
    pub maximized: bool,
    pub fullscreen: bool,
    /// None where the window system cannot tell (Wayland).
    pub minimized: Option<bool>,
    /// The monitor the window is on, already in desktop units.
    pub monitor: Option<Monitor>,
}

impl Sample {
    /// A reading in desktop units: the size in logical pixels, and the
    /// frame's and the surface's corners in `units` (on macOS, physical
    /// pixels divided by the window's scale are points, winit-appkit
    /// `window_delegate.rs:1284-1308`). Where `units` is none (Wayland) there
    /// is no position, and the units say physical; a window that cannot say
    /// whether it is minimized is taken as not.
    pub fn from_platform(reading: WindowReading, units: Option<Units>) -> Sample {
        let scale = finite_at_least(reading.scale, f64::MIN_POSITIVE, 1.0);
        let place = |(x, y): (i32, i32)| match units {
            Some(Units::Points) => (to_i32(f64::from(x) / scale), to_i32(f64::from(y) / scale)),
            Some(Units::Physical) | None => (x, y),
        };
        let frame = units.and(reading.frame);
        Sample {
            width: f64::from(reading.surface_size.0) / scale,
            height: f64::from(reading.surface_size.1) / scale,
            position: frame.map(place),
            surface_position: frame.map(|(x, y)| place((x.saturating_add(reading.inset.0), y.saturating_add(reading.inset.1)))),
            units: units.unwrap_or(Units::Physical),
            scale,
            maximized: reading.maximized,
            fullscreen: reading.fullscreen,
            minimized: reading.minimized.unwrap_or(false),
            monitor: reading.monitor,
        }
    }
}

/// Keeps `window.json`: the state the window is in, when to save it, and
/// what was last written, so that a save writes only a change.
#[derive(Clone, Debug)]
pub struct Keeper {
    tracker: Tracker,
    clock: SaveClock,
    written: WindowState,
}

impl Keeper {
    /// Keeps a window made from `saved`, which counts as written.
    pub fn new(saved: WindowState) -> Keeper {
        Keeper {
            tracker: Tracker::new(saved.clone()),
            clock: SaveClock::default(),
            written: saved,
        }
    }

    pub fn state(&self) -> &WindowState {
        self.tracker.state()
    }

    /// The window may have changed at `now`: it is read and saved when due.
    pub fn changed(&mut self, now: Instant) {
        self.clock.changed(now);
    }

    /// When the window is to be read and saved; none while nothing changed.
    pub fn due(&self) -> Option<Instant> {
        self.clock.due()
    }

    /// Takes in a reading of the window; returns whether the state changed.
    pub fn observe(&mut self, sample: &Sample) -> bool {
        self.tracker.observe(sample)
    }

    /// Records the page zoom; a change is saved when due.
    pub fn set_zoom(&mut self, zoom: f64, now: Instant) -> bool {
        let changed = self.tracker.set_zoom(zoom);
        if changed {
            self.clock.changed(now);
        }
        changed
    }

    /// Calls `write` with the state if it differs from what was last
    /// written; the clock starts over either way, and a failed write is
    /// tried again at the next save. Returns whether `write` was called.
    pub fn save_with(&mut self, write: impl FnOnce(&WindowState) -> Result<(), String>) -> bool {
        self.clock.saved();
        if self.tracker.state() == &self.written {
            return false;
        }
        if write(self.tracker.state()).is_ok() {
            self.written = self.tracker.state().clone();
        }
        true
    }
}

/// When to save the state after it changes: once it has been still for
/// [`SaveClock::QUIET`], and during a long drag at least every
/// [`SaveClock::LONGEST`].
#[derive(Clone, Debug, Default)]
pub struct SaveClock {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl SaveClock {
    pub const QUIET: Duration = Duration::from_millis(500);
    pub const LONGEST: Duration = Duration::from_secs(3);

    /// The state changed at `now`.
    pub fn changed(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// When the state should be saved; none while nothing is unsaved.
    pub fn due(&self) -> Option<Instant> {
        match (self.first, self.last) {
            (Some(first), Some(last)) => Some((last + Self::QUIET).min(first + Self::LONGEST)),
            _ => None,
        }
    }

    /// The state was saved.
    pub fn saved(&mut self) {
        self.first = None;
        self.last = None;
    }
}

#[cfg(test)]
mod tests;
