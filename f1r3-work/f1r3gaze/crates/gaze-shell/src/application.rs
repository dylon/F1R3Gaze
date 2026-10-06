//! The window's application handler: Blitz's, plus three things blitz-shell
//! leaves out (docs/ui/ledger.md, L9), and a fourth: the colour scheme. The
//! system's scheme, which winit reports on macOS and Windows, goes to the
//! chrome, and so does the focus, on which the chrome asks the XDG portal
//! again (Linux; docs/storage/README.md §12). The scheme the chrome shows
//! goes to its pages, through Blitz's theme override (ledger L13), and to
//! the window's decorations (`decoration_theme`); the chrome asks for both
//! with `WindowRequest`s. A window resized by its frame had painted each
//! new size twice: once for the size, and once more for no
//! visible change. In a live drag that was 76–88 % of the frames that
//! applied a size, against 2 % on `main`. Since L8, a page's redraw request
//! reaches the window (`cursor::PageShell`). Two kinds of request then
//! arrived with every resize, and neither needed a frame:
//!
//! 1. **The mouse leaves the window (H8).** blitz-shell forwards nothing
//!    when a mouse leaves (`View::handle_winit_event`, `PointerLeft`). Blitz
//!    then goes on hovering wherever the pointer was last seen, and every
//!    layout re-resolves that hover (`BaseDocument::refresh_hover`). Grabbing
//!    a window's frame takes the pointer out of the window, and as the window
//!    is resized the page reflows under that old spot. Each change of hover
//!    asks for a redraw: one more frame for every size. On a mouse
//!    `PointerLeft`, [`ChromeApplication`] tells the chrome, which ends the
//!    hover of the chrome and of its page (`ChromeDocument::pointer_left`).
//! 2. **A page asks for the frame it is in (H9).** The chrome's layout pass
//!    gives each page its new viewport and lays the page out right away
//!    (`BaseDocument::resolve`, sub-documents). Setting the viewport makes the
//!    page ask for a redraw (`queue_device_changes`), which the frame being
//!    painted already answers. [`ChromeApplication`] brackets each
//!    `RedrawRequested` with `ChromeDocument::begin_paint` and `end_paint`.
//!    A page's request made in between gets one more frame only if Blitz
//!    left the page a restyle for the next pass, which is how it leaves a
//!    hover changed after layout.
//!
//! The third addition makes the frame for a new size show the right chrome:
//!
//! 3. **A resize is polled before it is painted (H7).**
//!    `BlitzApplication::window_event` asks for a poll of the document
//!    through the event-loop proxy. On Wayland that request arrives at the
//!    start of the next loop iteration (winit-wayland 0.31.0-beta.3,
//!    `EventLoop::single_iteration`: `proxy_wake_up` comes before the
//!    compositor's resizes, and `RedrawRequested` after them). On macOS,
//!    AppKit's live resize paints inside the resize notification, before
//!    any proxy request (winit-appkit, `frame_did_change`). Either way, the
//!    frame for a new size showed the tab strip and the status bar fitted to
//!    the old width, and the poll that came next painted again.
//!    [`ChromeApplication`] polls the window as soon as Blitz has handled a
//!    resize, so the frame painted for a size is already fitted to it. When
//!    the poll changed the chrome, the window is also laid out at once:
//!    Blitz hit-tests input with the last layout, which would still hold the
//!    replaced nodes (`ChromeDocument::lay_out_now`).
//!
//! A fifth makes the window where it was left and keeps `state/window.json`
//! (storage ledger S13, part 2; `window_state.rs`):
//!
//! 5. **The window is made once the monitors can be listed**, in
//!    `can_create_surfaces`, at the size and place `plan_restore` gives;
//!    after Blitz has shown it, its zoom, scheme, maximized and full-screen
//!    states are restored, in that order (a window manager ignores a
//!    maximize asked of a hidden window). Its geometry events only mark it
//!    changed: it is read, and `window.json` written if it changed, when a
//!    save is due (`SaveClock`, through `ControlFlow::WaitUntil`), before it
//!    closes, when it loses a focus it had, and around full screen. Blitz's
//!    zoom keys are kept within the range `window.json` holds (chrome ledger
//!    L15).
//!
//! In a `frame-times` build, the environment variables
//! `F1R3GAZE_END_HOVER_ON_LEAVE=0`, `F1R3GAZE_ANSWER_IN_PAINT=0`,
//! `F1R3GAZE_POLL_ON_RESIZE=0` and `F1R3GAZE_KEEP_WINDOW=0` turn the first
//! three and the keeping of `window.json` off, so one binary can measure each
//! behaviour with and without it.

use crate::chrome::{ChromeDocument, WindowRequest};
use crate::frame_stats;
use crate::theme::Scheme;
use crate::window_state::{
    FullScreen, Keeper, Monitor, MonitorReading, RestorePlan, Sample, Units, WindowReading, WindowState, WindowSystem,
    plan_restore, zoom_correction,
};
use anyrender::WindowRenderer;
use blitz_shell::{BlitzApplication, WindowConfig};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::application::macos::ApplicationHandlerExtMacOS;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, Position};
use winit::event::{DeviceEvent, DeviceId, PointerKind, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::monitor::{Fullscreen, MonitorHandle};
use winit::raw_window_handle::RawDisplayHandle;
use winit::window::{Theme, Window, WindowAttributes, WindowId};

/// Writes `window.json`: `Profile::write_state` on the window's profile.
pub type SaveWindow = Box<dyn Fn(&WindowState) -> Result<(), String>>;

/// The window `launch()` asks for. It is made in `can_create_surfaces`, the
/// first point at which the event loop can list the monitors the saved place
/// is checked against.
pub struct PendingWindow<R: WindowRenderer> {
    pub chrome: ChromeDocument,
    pub renderer: R,
    /// `window.json` as start-up left it.
    pub saved: WindowState,
    /// How to write it. The chrome, which holds the profile, is dropped with
    /// the window at `CloseRequested`, so the saver is kept apart.
    pub save: SaveWindow,
}

/// The window `window.json` keeps.
struct KeptWindow {
    /// None once the window has closed.
    id: Option<WindowId>,
    system: WindowSystem,
    keeper: Keeper,
    save: SaveWindow,
    /// Whether the window has had the focus: winit-x11 reports
    /// `Focused(false)` when a window is mapped (`event_processor.rs:879-890`),
    /// before a window manager has maximized it, and a save then would
    /// write the state it is leaving.
    focused: bool,
}

/// Blitz's application handler, ending hover when the mouse leaves a window,
/// answering the pages' requests for the frame being painted, and polling a
/// window's document as soon as the window changes size.
pub struct ChromeApplication<R: WindowRenderer> {
    blitz: BlitzApplication<R>,
    /// End the hover in a window the mouse has left (H8).
    end_hover_on_leave: bool,
    /// Weigh the pages' redraw requests made while a frame is painted (H9).
    answer_in_paint: bool,
    /// Poll at once after a resize, or leave the poll to Blitz's proxy (H7).
    poll_on_resize: bool,
    /// The decorations each window was last given (`None` inside: the
    /// system's own), so that a request that changes nothing calls no
    /// platform code (Windows redraws its title bar at every `set_theme`).
    decorations: HashMap<WindowId, Option<Theme>>,
    /// The window to make once the monitors can be listed.
    pending: Option<PendingWindow<R>>,
    /// The window `window.json` keeps.
    kept: Option<KeptWindow>,
    /// Keep `window.json` (S13, part 2).
    keep_window: bool,
}

/// How many times the chrome's requests are drained in a row: carrying one
/// out can report back to the chrome, which can ask one more thing
/// (`FollowSystem` reports the system's scheme, and that can ask for a
/// `Scheme`). Two rounds settle every chain; the rest are margin.
const REQUEST_ROUNDS: usize = 4;

impl<R: WindowRenderer> ChromeApplication<R> {
    // Was `new(blitz)`, with the window already added to Blitz by `launch()`
    // (`add_window(WindowConfig::new(..))`): it was made before the monitors
    // could be listed, so it could not be put where it was left.
    pub fn new(blitz: BlitzApplication<R>, window: PendingWindow<R>) -> Self {
        ChromeApplication {
            blitz,
            end_hover_on_leave: enabled("F1R3GAZE_END_HOVER_ON_LEAVE"),
            answer_in_paint: enabled("F1R3GAZE_ANSWER_IN_PAINT"),
            poll_on_resize: enabled("F1R3GAZE_POLL_ON_RESIZE"),
            decorations: HashMap::new(),
            pending: Some(window),
            kept: None,
            keep_window: enabled("F1R3GAZE_KEEP_WINDOW"),
        }
    }

    /// Makes the pending window where it was left (`plan_restore`); then,
    /// once Blitz has shown it, restores its zoom, its scheme, and whether it
    /// was maximized or full screen, in that order. Blitz makes a window
    /// hidden and shows it in `View::init` (blitz-shell `window.rs:142-155`),
    /// and a window manager ignores a maximize asked of a hidden window.
    fn create_window(&mut self, event_loop: &dyn ActiveEventLoop, pending: PendingWindow<R>, preference: Option<Scheme>) {
        let PendingWindow { mut chrome, renderer, saved, save } = pending;
        let display = event_loop.rwh_06_handle().display_handle().ok().map(|handle| handle.as_raw());
        let system = window_system_of(display);
        let (handles, monitors): (Vec<MonitorHandle>, Vec<Monitor>) = event_loop
            .available_monitors()
            .filter_map(|handle| {
                let reading = monitor_reading_of(&handle, system)?;
                Some((handle, Monitor::from_platform(reading, system.units())))
            })
            .unzip();
        let primary = event_loop.primary_monitor().and_then(|primary| handles.iter().position(|h| *h == primary));
        let plan = plan_restore(&saved, &monitors, primary, system);
        // The window is made with its decorations' theme, except where the
        // platform would keep that for good (Windows).
        chrome.window_reported_scheme(preference);
        let (effective, follows_system) = chrome.scheme_shown();
        let theme = creation_theme(
            CreationTheme::CURRENT,
            decoration_theme(DecorationPlatform::CURRENT, effective, follows_system),
        );
        let before: HashSet<WindowId> = self.blitz.windows.keys().copied().collect();
        self.blitz.add_window(WindowConfig::with_attributes(
            Box::new(chrome),
            renderer,
            window_attributes(&plan, system, theme),
        ));
        self.blitz.can_create_surfaces(event_loop);
        let Some(id) = self.blitz.windows.keys().copied().find(|id| !before.contains(id)) else {
            return;
        };
        self.decorations.insert(id, theme);
        // `View::init` gave the chrome a viewport at zoom 1 (blitz-traits
        // `shell.rs:97-108`).
        if plan.zoom != 1.0
            && let Some(view) = self.blitz.windows.get_mut(&id)
        {
            view.doc.inner_mut().viewport_mut().set_zoom(plan.zoom as f32);
        }
        // The scheme shown: Blitz's theme override, and the decorations.
        self.carry_out_requests(event_loop, id);
        if let Some(view) = self.blitz.windows.get(&id) {
            if plan.maximized {
                view.window.set_maximized(true);
            }
            if let Some(full) = plan.fullscreen {
                let monitor = match full {
                    FullScreen::On(i) => handles.get(i).cloned(),
                    FullScreen::Current => None,
                };
                view.window.set_fullscreen(Some(Fullscreen::Borderless(monitor)));
            }
        }
        if plan.fullscreen.is_some()
            && let Some(chrome) = self.chrome(id)
        {
            chrome.full_screen_changed(true);
        }
        if self.keep_window {
            let mut keeper = Keeper::new(saved);
            // No event need report the window's first size and place (an X11
            // window mapped with no window manager gets none): it is read
            // once the window system has applied the restore.
            keeper.changed(Instant::now());
            self.kept = Some(KeptWindow {
                id: Some(id),
                system,
                keeper,
                save,
                focused: false,
            });
        }
    }

    /// Reads the window into the state `window.json` keeps.
    fn observe_window(&mut self, window_id: WindowId) {
        let (Some(kept), Some(view)) = (self.kept.as_mut(), self.blitz.windows.get(&window_id)) else {
            return;
        };
        if kept.id != Some(window_id) {
            return;
        }
        let reading = reading_of(view.window.as_ref(), kept.system);
        kept.keeper.observe(&Sample::from_platform(reading, kept.system.units()));
    }

    /// Reads the window, and writes `window.json` if it changed.
    fn save_window(&mut self, window_id: WindowId) {
        // Was a span: a reading is now also printed with its own time and
        // whether it wrote, which the resize bench needs to tell a reading
        // made during a sweep from one made before it and reported on the
        // sweep's first frame (frame_stats::WindowSave; storage ledger S13,
        // part 2).
        // let _span = frame_stats::WINDOW_SAVE.span();
        // self.observe_window(window_id);
        // if let Some(kept) = self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
        //     let KeptWindow { keeper, save, .. } = kept;
        //     keeper.save_with(|state| save(state));
        // }
        let reading = frame_stats::WindowSave::start();
        self.observe_window(window_id);
        let changed = match self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
            Some(KeptWindow { keeper, save, .. }) => keeper.save_with(|state| save(state)),
            None => false,
        };
        reading.end(changed);
    }

    /// After a key Blitz may have zoomed the window with: the zoom kept
    /// within its range (chrome ledger L15), and a change kept for
    /// `window.json`.
    fn follow_zoom(&mut self, window_id: WindowId, now: Instant) {
        let Some(view) = self.blitz.windows.get_mut(&window_id) else {
            return;
        };
        let zoom = f64::from(view.doc.inner().viewport().zoom());
        let zoom = match zoom_correction(zoom) {
            Some(kept) => {
                view.doc.inner_mut().viewport_mut().set_zoom(kept as f32);
                kept
            }
            None => zoom,
        };
        if let Some(kept) = self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
            kept.keeper.set_zoom(zoom, now);
        }
    }

    /// F11, or Ctrl+Cmd+F on macOS: full screen on the window's monitor, or
    /// back. The window is read first, so that leaving full screen restores
    /// what was there just before.
    fn toggle_full_screen(&mut self, window_id: WindowId) {
        self.observe_window(window_id);
        let Some(view) = self.blitz.windows.get(&window_id) else {
            return;
        };
        let entering = view.window.fullscreen().is_none();
        view.window.set_fullscreen(entering.then_some(Fullscreen::Borderless(None)));
        if let Some(chrome) = self.chrome(window_id) {
            chrome.full_screen_changed(entering);
        }
        if let Some(kept) = self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
            kept.keeper.changed(Instant::now());
        }
    }

    /// The chrome shown in `window_id`, if that window shows one.
    fn chrome(&mut self, window_id: WindowId) -> Option<&mut ChromeDocument> {
        let view = self.blitz.windows.get_mut(&window_id)?;
        let doc: &mut dyn Any = view.doc.as_mut();
        doc.downcast_mut::<ChromeDocument>()
    }

    /// Every window's chrome.
    fn chromes(&mut self) -> impl Iterator<Item = &mut ChromeDocument> {
        self.blitz.windows.values_mut().filter_map(|view| {
            let doc: &mut dyn Any = view.doc.as_mut();
            doc.downcast_mut::<ChromeDocument>()
        })
    }

    /// Carries out what the chrome in `window_id` asked of its window.
    fn carry_out_requests(&mut self, event_loop: &dyn ActiveEventLoop, window_id: WindowId) {
        for _ in 0..REQUEST_ROUNDS {
            let Some(requests) = self.chrome(window_id).map(ChromeDocument::take_window_requests) else {
                return;
            };
            if requests.is_empty() {
                return;
            }
            for request in requests {
                match request {
                    WindowRequest::Scheme { effective, follows_system } => {
                        self.show_scheme(window_id, effective, follows_system)
                    }
                    WindowRequest::ToggleFullScreen => self.toggle_full_screen(window_id),
                    WindowRequest::FollowSystem => {
                        // macOS and Windows; None elsewhere, which the chrome
                        // ignores (it reads the portal there).
                        let preference = event_loop.system_theme().map(scheme_of);
                        if let Some(chrome) = self.chrome(window_id) {
                            chrome.window_reported_scheme(preference);
                        }
                    }
                }
            }
        }
    }

    /// Carries out every window's requests.
    fn carry_out_everywhere(&mut self, event_loop: &dyn ActiveEventLoop) {
        let ids: Vec<WindowId> = self.blitz.windows.keys().copied().collect();
        for id in ids {
            self.carry_out_requests(event_loop, id);
        }
    }

    /// Shows `effective` in `window_id`: to its pages and in its decorations,
    /// each only if it changes.
    fn show_scheme(&mut self, window_id: WindowId, effective: Scheme, follows_system: bool) {
        let Some(view) = self.blitz.windows.get_mut(&window_id) else {
            return;
        };
        let change = scheme_change(
            DecorationPlatform::CURRENT,
            view.theme_override(),
            self.decorations.get(&window_id).copied(),
            effective,
            follows_system,
        );
        // L13: Blitz copies the chrome's scheme into its pages; the override
        // keeps the window's own theme from replacing it (`View::init` sets
        // the scheme from the window's theme, which winit-x11 never knows,
        // and so does `ThemeChanged`).
        if let Some(page) = change.override_to {
            view.set_theme_override(Some(page));
        }
        if let Some(decorations) = change.decorations_to {
            view.window.set_theme(decorations);
            self.decorations.insert(window_id, decorations);
        }
    }
}

/// The window system, from the event loop's display handle: X11 (Xlib or
/// XCB), macOS (AppKit) or Windows. Anything else (Wayland, and a display
/// handle that cannot be read) is taken as a system that places windows
/// itself: no window is positioned.
fn window_system_of(display: Option<RawDisplayHandle>) -> WindowSystem {
    match display {
        Some(RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)) => WindowSystem::X11,
        Some(RawDisplayHandle::AppKit(_)) => WindowSystem::MacOs,
        Some(RawDisplayHandle::Windows(_)) => WindowSystem::Windows,
        _ => WindowSystem::Wayland,
    }
}

/// Whether the platform keeps the theme a window is made with for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreationTheme {
    /// Windows: winit keeps it as the window's `preferred_theme`, and while
    /// that is set the window never reports the system's changes again, even
    /// after `set_theme(None)` (winit-win32 `event_loop.rs:2508-2523`).
    KeptForGood,
    /// Elsewhere the window's theme can be changed later.
    Replaceable,
}

impl CreationTheme {
    const CURRENT: CreationTheme = match cfg!(windows) {
        true => CreationTheme::KeptForGood,
        false => CreationTheme::Replaceable,
    };
}

/// The theme a window is made with: its decorations' theme, except where
/// the platform would keep it for good. There the first `Scheme` request
/// sets it, once the window is shown (ledger S12, part 3, finding 4).
fn creation_theme(platform: CreationTheme, decorations: Option<Theme>) -> Option<Theme> {
    match platform {
        CreationTheme::KeptForGood => None,
        CreationTheme::Replaceable => decorations,
    }
}

/// The attributes of a window made by `plan`: its surface size in logical
/// pixels, its place in desktop units (macOS's points, which winit takes as
/// logical; elsewhere physical pixels), and the theme it is made with.
fn window_attributes(plan: &RestorePlan, system: WindowSystem, theme: Option<Theme>) -> WindowAttributes {
    let attributes = WindowAttributes::default()
        .with_surface_size(LogicalSize::new(plan.size.0, plan.size.1))
        .with_theme(theme);
    match (plan.position, system.units()) {
        (Some((x, y)), Some(Units::Points)) => {
            attributes.with_position(Position::Logical(LogicalPosition::new(f64::from(x), f64::from(y))))
        }
        (Some((x, y)), Some(Units::Physical)) => attributes.with_position(Position::Physical(PhysicalPosition::new(x, y))),
        _ => attributes,
    }
}

/// Whether `event` may change what `window.json` keeps: its size or place.
fn changes_window_state(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::SurfaceResized(_) | WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. }
    )
}

/// Whether `event` takes away a focus the window had: the window is then
/// read and saved at once.
fn loses_focus(had_focus: bool, event: &WindowEvent) -> bool {
    had_focus && matches!(event, WindowEvent::Focused(false))
}

/// The loop's control flow: woken when `window.json` is due, else waiting.
fn control_flow_for(due: Option<Instant>) -> ControlFlow {
    match due {
        Some(due) => ControlFlow::WaitUntil(due),
        None => ControlFlow::Wait,
    }
}

/// A monitor as winit gives it; none for one with no position, or a mode
/// with no area. Its UUID is kept on macOS, where winit's id is the display's
/// UUID (winit-appkit `monitor.rs:112-114`).
fn monitor_reading_of(handle: &MonitorHandle, system: WindowSystem) -> Option<MonitorReading> {
    let position = handle.position()?;
    let mode = handle.current_video_mode()?.size();
    if mode.width == 0 || mode.height == 0 {
        return None;
    }
    Some(MonitorReading {
        name: handle.name().map(|name| name.into_owned()),
        uuid: (system == WindowSystem::MacOs).then(|| format!("{:032x}", handle.id())),
        position: (position.x, position.y),
        mode: (mode.width, mode.height),
        scale: handle.scale_factor(),
    })
}

/// The window as winit gives it now.
fn reading_of(window: &dyn Window, system: WindowSystem) -> WindowReading {
    let size = window.surface_size();
    let inset = window.surface_position();
    WindowReading {
        surface_size: (size.width, size.height),
        scale: window.scale_factor(),
        frame: window.outer_position().ok().map(|frame| (frame.x, frame.y)),
        inset: (inset.x, inset.y),
        maximized: window.is_maximized(),
        fullscreen: window.fullscreen().is_some(),
        minimized: window.is_minimized(),
        monitor: window
            .current_monitor()
            .and_then(|handle| monitor_reading_of(&handle, system))
            .map(|reading| Monitor::from_platform(reading, system.units())),
    }
}

/// How a platform's window decorations follow the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecorationPlatform {
    /// macOS and Windows: a window with no theme of its own follows the
    /// system and reports its changes (`ThemeChanged`). macOS stops
    /// reporting while a window has a theme of its own (winit-appkit,
    /// `window_delegate.rs`, the effective-appearance observer).
    FollowsSystem,
    /// X11 and Wayland: nothing is reported, and a window with no theme is
    /// dark on X11 (`_GTK_THEME_VARIANT`) and the desktop's on Wayland, so
    /// the window is always given the scheme shown.
    Shown,
}

impl DecorationPlatform {
    const CURRENT: DecorationPlatform = match cfg!(any(target_os = "macos", windows)) {
        true => DecorationPlatform::FollowsSystem,
        false => DecorationPlatform::Shown,
    };
}

/// A scheme as winit names it.
fn theme_of(scheme: Scheme) -> Theme {
    match scheme {
        Scheme::Dark => Theme::Dark,
        Scheme::Light => Theme::Light,
    }
}

/// The theme for a window's decorations: none while the window follows the
/// system where the platform does that itself (so that it goes on reporting
/// the system's changes); otherwise the scheme shown.
fn decoration_theme(platform: DecorationPlatform, effective: Scheme, follows_system: bool) -> Option<Theme> {
    match (platform, follows_system) {
        (DecorationPlatform::FollowsSystem, true) => None,
        _ => Some(theme_of(effective)),
    }
}

/// What a `Scheme` request changes in a window: Blitz's theme override, and
/// the decorations, each `Some` only if it differs from what the window has.
#[derive(Debug, PartialEq, Eq)]
struct SchemeChange {
    override_to: Option<Theme>,
    decorations_to: Option<Option<Theme>>,
}

/// The change that shows `effective` in a window whose override is
/// `has_override` and whose decorations were last given `has_decorations`
/// (`None`: never, or forgotten).
fn scheme_change(
    platform: DecorationPlatform,
    has_override: Option<Theme>,
    has_decorations: Option<Option<Theme>>,
    effective: Scheme,
    follows_system: bool,
) -> SchemeChange {
    let page = theme_of(effective);
    let decorations = decoration_theme(platform, effective, follows_system);
    SchemeChange {
        override_to: (has_override != Some(page)).then_some(page),
        decorations_to: (has_decorations != Some(decorations)).then_some(decorations),
    }
}

/// winit's theme as a scheme.
fn scheme_of(theme: Theme) -> Scheme {
    match theme {
        Theme::Light => Scheme::Light,
        Theme::Dark => Scheme::Dark,
    }
}

/// The scheme a `ThemeChanged` event reports (macOS and Windows).
fn reported_scheme(event: &WindowEvent) -> Option<Scheme> {
    match event {
        WindowEvent::ThemeChanged(theme) => Some(scheme_of(*theme)),
        _ => None,
    }
}

/// Whether `event` gives the window the focus: the chrome then asks the
/// portal again (Linux).
fn gains_focus(event: &WindowEvent) -> bool {
    matches!(event, WindowEvent::Focused(true))
}

/// Whether a behaviour is on. It always is, except in a `frame-times` build
/// whose environment sets `name` to `0`.
#[cfg(feature = "frame-times")]
fn enabled(name: &str) -> bool {
    !matches!(std::env::var(name).as_deref(), Ok("0"))
}

#[cfg(not(feature = "frame-times"))]
fn enabled(_name: &str) -> bool {
    true
}

/// Whether `event` changes the size the window's document is laid out for.
fn changes_layout_size(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. }
    )
}

/// Whether `event` is the mouse leaving the window. A finger or a pen that
/// leaves is Blitz's to handle: it cancels the touch.
fn mouse_left(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::PointerLeft {
            kind: PointerKind::Mouse,
            ..
        }
    )
}

// Every method is forwarded, so Blitz behaves as it does on its own apart
// from the three additions. The lint fails the build if winit adds a method
// that is not forwarded.
#[deny(clippy::missing_trait_methods)]
impl<R: WindowRenderer> ApplicationHandler for ChromeApplication<R> {
    fn new_events(&mut self, event_loop: &dyn ActiveEventLoop, cause: StartCause) {
        self.blitz.new_events(event_loop, cause);
    }

    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.resumed(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        // The system's scheme before the first frame (macOS and Windows; None
        // elsewhere, where the chrome reads the portal instead).
        let preference = event_loop.system_theme().map(scheme_of);
        // Was Blitz's alone, which made the window `launch()` had added with
        // default attributes: now the window is made where it was left.
        // self.blitz.can_create_surfaces(event_loop);
        match self.pending.take() {
            Some(window) => self.create_window(event_loop, window, preference),
            None => self.blitz.can_create_surfaces(event_loop),
        }
        for chrome in self.chromes() {
            chrome.window_reported_scheme(preference);
        }
        // Blitz has just made each window and set its scheme from the
        // window's own theme (`View::init`): the scheme shown is put back
        // before the first frame (L13).
        self.carry_out_everywhere(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.proxy_wake_up(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let resized = self.poll_on_resize && changes_layout_size(&event);
        let left = self.end_hover_on_leave && mouse_left(&event);
        // Read before Blitz takes the event.
        let reported = reported_scheme(&event);
        let focused = gains_focus(&event);
        let closing = matches!(event, WindowEvent::CloseRequested);
        let geometry = changes_window_state(&event);
        let keys = matches!(event, WindowEvent::KeyboardInput { .. });
        let blurred = self
            .kept
            .as_ref()
            .is_some_and(|kept| kept.id == Some(window_id) && loses_focus(kept.focused, &event));
        if closing {
            // Blitz drops the window at `CloseRequested` (blitz-shell
            // `application.rs:151-160`): it is read now, for the last time.
            self.save_window(window_id);
            if let Some(kept) = self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
                kept.id = None;
            }
        }
        let painting = self.answer_in_paint && matches!(event, WindowEvent::RedrawRequested);
        if painting && let Some(chrome) = self.chrome(window_id) {
            chrome.begin_paint();
        }
        self.blitz.window_event(event_loop, window_id, event);
        if painting && let Some(chrome) = self.chrome(window_id) {
            chrome.end_paint();
        }
        if left && let Some(chrome) = self.chrome(window_id) {
            chrome.pointer_left();
        }
        if let Some(scheme) = reported {
            // The window system may have changed the decorations itself
            // (Windows applies the system's theme at a settings change): the
            // next request sets them whatever they were last given.
            self.decorations.remove(&window_id);
            if let Some(chrome) = self.chrome(window_id) {
                chrome.window_reported_scheme(Some(scheme));
            }
        }
        if focused && let Some(chrome) = self.chrome(window_id) {
            chrome.window_focused(Instant::now());
        }
        let now = Instant::now();
        if focused && let Some(kept) = self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
            kept.focused = true;
        }
        if keys {
            self.follow_zoom(window_id, now);
        }
        // A size or place changed: read and saved when due, never here, on
        // the resize path (L9).
        if geometry && let Some(kept) = self.kept.as_mut().filter(|kept| kept.id == Some(window_id)) {
            kept.keeper.changed(now);
        }
        if blurred {
            self.save_window(window_id);
        }
        // Before this iteration's `RedrawRequested`: the frame painted for
        // the new size has the chrome fitted to it.
        if resized
            && let Some(view) = self.blitz.windows.get_mut(&window_id)
            && view.poll()
        {
            // The poll replaced chrome regions, and more input can come
            // before the frame. Blitz would hit-test it against the last
            // layout's paint tree, which holds the removed nodes (a panic),
            // so the window is laid out at once (`ChromeDocument::lay_out_now`).
            let time = view.current_animation_time();
            if let Some(chrome) = self.chrome(window_id) {
                chrome.lay_out_now(time);
            }
        }
        if closing {
            self.decorations.remove(&window_id);
        }
        // What this event made the chrome ask (none once Blitz has closed
        // the window).
        self.carry_out_requests(event_loop, window_id);
    }

    fn device_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        device_id: Option<DeviceId>,
        event: DeviceEvent,
    ) {
        self.blitz.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.about_to_wait(event_loop);
        // What the chromes asked while their documents were polled (a portal
        // answer).
        self.carry_out_everywhere(event_loop);
        // The save timer (`SaveClock`). Blitz never sets the control flow,
        // and `launch()` starts it at `Wait`.
        let now = Instant::now();
        let due_now = self
            .kept
            .as_ref()
            .filter(|kept| kept.keeper.due().is_some_and(|due| due <= now))
            .and_then(|kept| kept.id);
        if let Some(id) = due_now {
            self.save_window(id);
        }
        let due = self.kept.as_ref().and_then(|kept| kept.id.and(kept.keeper.due()));
        event_loop.set_control_flow(control_flow_for(due));
    }

    fn suspended(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.suspended(event_loop);
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.destroy_surfaces(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.memory_warning(event_loop);
    }

    fn macos_handler(&mut self) -> Option<&mut dyn ApplicationHandlerExtMacOS> {
        self.blitz.macos_handler()
    }
}

impl<R: WindowRenderer> Drop for ChromeApplication<R> {
    /// The loop has ended (macOS's Quit closes the window without a
    /// `CloseRequested`, winit-appkit `app_state.rs:188-192`): what was last
    /// read is written, without reading the window, which is gone or going.
    fn drop(&mut self) {
        if let Some(kept) = self.kept.as_mut() {
            let KeptWindow { keeper, save, .. } = kept;
            keeper.save_with(|state| save(state));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use winit::dpi::PhysicalSize;
    use winit::event::{FingerId, SurfaceSizeWriter};

    #[test]
    fn only_a_new_size_or_scale_polls_at_once() {
        assert!(changes_layout_size(&WindowEvent::SurfaceResized(
            PhysicalSize::new(800, 600)
        )));
        let size = Arc::new(Mutex::new(PhysicalSize::new(800, 600)));
        assert!(changes_layout_size(&WindowEvent::ScaleFactorChanged {
            scale_factor: 2.0,
            surface_size_writer: SurfaceSizeWriter::new(Arc::downgrade(&size)),
        }));
        for event in [
            WindowEvent::RedrawRequested,
            WindowEvent::Focused(true),
            WindowEvent::CloseRequested,
        ] {
            assert!(!changes_layout_size(&event), "{event:?}");
        }
    }

    #[test]
    fn only_theme_changes_report_a_scheme() {
        assert_eq!(reported_scheme(&WindowEvent::ThemeChanged(Theme::Dark)), Some(Scheme::Dark));
        assert_eq!(reported_scheme(&WindowEvent::ThemeChanged(Theme::Light)), Some(Scheme::Light));
        for event in [WindowEvent::Focused(true), WindowEvent::RedrawRequested, WindowEvent::CloseRequested] {
            assert_eq!(reported_scheme(&event), None, "{event:?}");
        }
    }

    #[test]
    fn only_gaining_focus_asks_again() {
        assert!(gains_focus(&WindowEvent::Focused(true)));
        for event in [WindowEvent::Focused(false), WindowEvent::ThemeChanged(Theme::Dark), WindowEvent::RedrawRequested] {
            assert!(!gains_focus(&event), "{event:?}");
        }
    }

    /// The decorations follow the system only where the platform reports
    /// the system's changes and the window follows the system; otherwise
    /// they are the scheme shown (an X11 window with no theme is dark).
    #[test]
    fn the_title_bar_follows_the_system_only_where_the_os_reports_changes() {
        use DecorationPlatform::{FollowsSystem, Shown};
        let rows = [
            (FollowsSystem, Scheme::Dark, true, None),
            (FollowsSystem, Scheme::Light, true, None),
            (FollowsSystem, Scheme::Dark, false, Some(Theme::Dark)),
            (FollowsSystem, Scheme::Light, false, Some(Theme::Light)),
            (Shown, Scheme::Dark, true, Some(Theme::Dark)),
            (Shown, Scheme::Light, true, Some(Theme::Light)),
            (Shown, Scheme::Dark, false, Some(Theme::Dark)),
            (Shown, Scheme::Light, false, Some(Theme::Light)),
        ];
        for (platform, effective, follows, want) in rows {
            assert_eq!(
                decoration_theme(platform, effective, follows),
                want,
                "{platform:?}, {effective:?}, following the system: {follows}"
            );
        }
        assert_eq!(
            DecorationPlatform::CURRENT,
            if cfg!(any(target_os = "macos", windows)) { FollowsSystem } else { Shown }
        );
    }

    /// A `Scheme` request changes the override and the decorations only
    /// where they differ from what the window has.
    #[test]
    fn a_scheme_request_changes_only_what_differs() {
        use DecorationPlatform::{FollowsSystem, Shown};
        let change = |override_to, decorations_to| SchemeChange { override_to, decorations_to };
        // A new window: both are set.
        assert_eq!(
            scheme_change(Shown, None, None, Scheme::Dark, true),
            change(Some(Theme::Dark), Some(Some(Theme::Dark)))
        );
        // The same again: nothing.
        assert_eq!(
            scheme_change(Shown, Some(Theme::Dark), Some(Some(Theme::Dark)), Scheme::Dark, true),
            change(None, None)
        );
        // Back to following the system where the platform does: no theme.
        assert_eq!(
            scheme_change(FollowsSystem, Some(Theme::Dark), Some(Some(Theme::Dark)), Scheme::Light, true),
            change(Some(Theme::Light), Some(None))
        );
        // An explicit choice of the scheme already shown: the decorations only.
        assert_eq!(
            scheme_change(FollowsSystem, Some(Theme::Light), Some(None), Scheme::Light, false),
            change(None, Some(Some(Theme::Light)))
        );
        // Decorations forgotten after a report: set again.
        assert_eq!(
            scheme_change(FollowsSystem, Some(Theme::Light), None, Scheme::Light, true),
            change(None, Some(None))
        );
    }

    // ── Storage ledger S13 (part 2): the window kept in window.json ──

    #[test]
    fn only_geometry_events_schedule_a_save() {
        use winit::dpi::{PhysicalPosition, PhysicalSize};
        let size = Arc::new(Mutex::new(PhysicalSize::new(800, 600)));
        for event in [
            WindowEvent::SurfaceResized(PhysicalSize::new(800, 600)),
            WindowEvent::Moved(PhysicalPosition::new(10, 20)),
            WindowEvent::ScaleFactorChanged {
                scale_factor: 2.0,
                surface_size_writer: SurfaceSizeWriter::new(Arc::downgrade(&size)),
            },
        ] {
            assert!(changes_window_state(&event), "{event:?}");
        }
        for event in [
            WindowEvent::RedrawRequested,
            WindowEvent::Focused(true),
            WindowEvent::Focused(false),
            WindowEvent::CloseRequested,
            WindowEvent::Occluded(true),
        ] {
            assert!(!changes_window_state(&event), "{event:?}");
        }
    }

    /// winit-x11 reports `Focused(false)` when a window is mapped, before a
    /// window manager has maximized it: only a focus the window had counts.
    #[test]
    fn only_losing_a_focus_it_had_saves_at_once() {
        assert!(loses_focus(true, &WindowEvent::Focused(false)));
        assert!(!loses_focus(false, &WindowEvent::Focused(false)), "the X11 map");
        assert!(!loses_focus(true, &WindowEvent::Focused(true)));
        assert!(!loses_focus(true, &WindowEvent::RedrawRequested));
    }

    #[test]
    fn the_window_system_comes_from_the_display() {
        use std::ptr::NonNull;
        use winit::raw_window_handle::{
            AppKitDisplayHandle, WaylandDisplayHandle, WindowsDisplayHandle, XcbDisplayHandle, XlibDisplayHandle,
        };
        let cases = [
            (Some(RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0))), WindowSystem::X11),
            (Some(RawDisplayHandle::Xcb(XcbDisplayHandle::new(None, 0))), WindowSystem::X11),
            (Some(RawDisplayHandle::AppKit(AppKitDisplayHandle::new())), WindowSystem::MacOs),
            (Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())), WindowSystem::Windows),
            (Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(NonNull::dangling()))), WindowSystem::Wayland),
            (None, WindowSystem::Wayland),
        ];
        for (display, system) in cases {
            assert_eq!(window_system_of(display), system, "{display:?}");
        }
    }

    /// Windows keeps a creation theme for good (step 10, finding 4).
    #[test]
    fn a_window_is_made_with_no_theme_on_windows() {
        for decorations in [Some(Theme::Dark), Some(Theme::Light), None] {
            assert_eq!(creation_theme(CreationTheme::KeptForGood, decorations), None, "{decorations:?}");
            assert_eq!(creation_theme(CreationTheme::Replaceable, decorations), decorations);
        }
        assert_eq!(
            CreationTheme::CURRENT,
            if cfg!(windows) { CreationTheme::KeptForGood } else { CreationTheme::Replaceable }
        );
    }

    #[test]
    fn the_window_is_made_at_the_planned_size_and_place() {
        use winit::dpi::Size;
        let plan = |position| RestorePlan {
            size: (900.0, 600.0),
            position,
            maximized: false,
            fullscreen: None,
            zoom: 1.0,
        };
        let made = window_attributes(&plan(Some((100, 78))), WindowSystem::MacOs, Some(Theme::Dark));
        assert_eq!(made.surface_size, Some(Size::Logical(LogicalSize::new(900.0, 600.0))));
        assert_eq!(made.position, Some(Position::Logical(LogicalPosition::new(100.0, 78.0))), "macOS: points");
        assert_eq!(made.preferred_theme, Some(Theme::Dark));
        let x11 = window_attributes(&plan(Some((120, 80))), WindowSystem::X11, None);
        assert_eq!(x11.position, Some(Position::Physical(PhysicalPosition::new(120, 80))), "X11: physical pixels");
        assert_eq!(x11.preferred_theme, None);
        assert_eq!(window_attributes(&plan(None), WindowSystem::X11, None).position, None);
        assert_eq!(window_attributes(&plan(Some((5, 5))), WindowSystem::Wayland, None).position, None);
    }

    #[test]
    fn the_loop_wakes_when_a_save_is_due() {
        let due = Instant::now();
        assert_eq!(control_flow_for(Some(due)), ControlFlow::WaitUntil(due));
        assert_eq!(control_flow_for(None), ControlFlow::Wait);
    }

    #[test]
    fn only_the_mouse_leaving_ends_the_hover() {
        let left = |kind| WindowEvent::PointerLeft {
            device_id: None,
            position: None,
            primary: true,
            kind,
        };
        assert!(mouse_left(&left(PointerKind::Mouse)));
        assert!(!mouse_left(&left(PointerKind::Touch(FingerId::from_raw(1)))));
        assert!(!mouse_left(&left(PointerKind::Unknown)));
        assert!(!mouse_left(&WindowEvent::Focused(false)));
    }
}
