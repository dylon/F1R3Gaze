//! The window's mouse cursor, and pointer hover inside pages
//! (`docs/ui/ledger.md`, L8).
//!
//! Each Blitz document decides its own cursor. `BaseDocument::get_cursor`
//! reads the element under the pointer, and when a document's hover changes
//! it passes that cursor to its `ShellProvider::set_cursor`. Pages are
//! sub-documents of the chrome, and at Blitz 674d7d2 three facts break this
//! for them:
//!
//! 1. The chrome's hit test stops at a page's host element, `.view`. Moving
//!    the pointer inside a page therefore never changes the chrome's hover,
//!    and the chrome never asks for a new cursor (`set_hover_to`).
//! 2. When the chrome's hover does move onto a host, the chrome asks for the
//!    page's cursor *before* it forwards the event to the page
//!    (`EventDriver::handle_ui_event`). The answer describes where the pointer
//!    was when the page last saw it. A page that has never seen the pointer
//!    answers `None`, and blitz-shell hides the cursor for `None`.
//! 3. A page's own requests go to its provider, Blitz's
//!    `DummyShellProvider`, so its new cursors and its redraw requests are
//!    lost.
//!
//! Handing pages the window's real provider would fix 3, but it would also
//! give them the window title, the clipboard, file dialogs and window
//! controls. Instead, one place decides:
//!
//! * [`WindowShell`] wraps the chrome's provider. It passes every request on
//!   except the cursor, which it treats as a hint that the cursor may have
//!   changed.
//! * [`PageShell`] is every page's provider. It can only report that the
//!   page's hover changed, or that the page needs painting.
//! * [`CursorArbiter::sync`] computes the cursor from the state of the
//!   chrome and of the page under the pointer, after the page has seen the
//!   event. It sends that cursor to the window only when it changes.
//!   - It also ends a page's hover when the pointer leaves the page, because
//!     Blitz forwards no leave events to sub-documents.
//!   - It tells a page where the pointer is when the page comes under a
//!     pointer that did not move, for example a page loading or a tab
//!     switching under a resting pointer.

use blitz_dom::{BaseDocument, NodeId, Point};
use blitz_traits::events::UiEvent;
use blitz_traits::shell::{ClipboardError, FileDialogFilter, ShellProvider};
use cursor_icon::CursorIcon;
use gaze_dom_blitz::WakeHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The reports the chrome's and the pages' providers have made since the
/// arbiter last looked. Both flags are atomics, because a provider must be
/// `Send + Sync`.
struct CursorRouter {
    /// A document's hover changed, so the cursor may differ.
    dirty: AtomicBool,
    /// A page asked to be painted.
    repaint: AtomicBool,
    wake: WakeHandle,
}

impl CursorRouter {
    /// Raise `flag`, waking the window's event loop only when the flag was
    /// clear. A burst of reports then costs one poll.
    fn raise(&self, flag: &AtomicBool) {
        if !flag.swap(true, Ordering::AcqRel) {
            self.wake.wake();
        }
    }
}

/// The chrome's provider: blitz-shell's, with the cursor taken over.
///
/// It is installed over the provider blitz-shell sets in `View::init`
/// ([`CursorArbiter::install`]). It forwards every `ShellProvider` method of
/// Blitz 674d7d2 except `set_cursor`, and the forwarding test lists them. A
/// Blitz upgrade that adds a method must add it here, or the default no-op
/// would silently swallow it.
pub struct WindowShell {
    window: Arc<dyn ShellProvider>,
    router: Arc<CursorRouter>,
}

impl WindowShell {
    /// The only way a cursor reaches the window.
    fn apply(&self, icon: Option<CursorIcon>) {
        self.window.set_cursor(icon);
    }
}

impl ShellProvider for WindowShell {
    fn request_redraw(&self) {
        self.window.request_redraw();
    }
    /// The chrome's hover changed. Blitz computed `icon` before the page
    /// under the pointer saw the event, so the arbiter recomputes it rather
    /// than show it.
    fn set_cursor(&self, _icon: Option<CursorIcon>) {
        self.router.raise(&self.router.dirty);
    }
    fn set_window_title(&self, title: String) {
        self.window.set_window_title(title);
    }
    fn set_ime_enabled(&self, is_enabled: bool) {
        self.window.set_ime_enabled(is_enabled);
    }
    fn set_ime_cursor_area(&self, x: f32, y: f32, width: f32, height: f32) {
        self.window.set_ime_cursor_area(x, y, width, height);
    }
    fn get_clipboard_text(&self) -> Result<String, ClipboardError> {
        self.window.get_clipboard_text()
    }
    fn set_clipboard_text(&self, text: String) -> Result<(), ClipboardError> {
        self.window.set_clipboard_text(text)
    }
    fn open_file_dialog(&self, multiple: bool, filter: Option<FileDialogFilter>) -> Vec<PathBuf> {
        self.window.open_file_dialog(multiple, filter)
    }
    fn request_window_close(&self) {
        self.window.request_window_close();
    }
    fn set_window_minimized(&self, minimized: bool) {
        self.window.set_window_minimized(minimized);
    }
    fn set_window_maximized(&self, maximized: bool) {
        self.window.set_window_maximized(maximized);
    }
    fn is_window_maximized(&self) -> bool {
        self.window.is_window_maximized()
    }
    fn set_window_decorations(&self, decorations: bool) {
        self.window.set_window_decorations(decorations);
    }
    fn drag_window(&self) {
        self.window.drag_window();
    }
}

/// Every page's provider, and so also that of the iframes inside a page.
/// It grants nothing that `DummyShellProvider` did not: a page can report
/// that its hover changed, or that it needs painting, and nothing else.
pub struct PageShell {
    router: Arc<CursorRouter>,
}

impl ShellProvider for PageShell {
    /// A page's hover restyles (and its scrolls and loaded images) must be
    /// painted, and only the window can redraw.
    fn request_redraw(&self) {
        self.router.raise(&self.router.repaint);
    }
    /// The page's hover changed. Its cursor matters only while the page is
    /// under the pointer, and the arbiter checks that.
    fn set_cursor(&self, _icon: Option<CursorIcon>) {
        self.router.raise(&self.router.dirty);
    }
}

/// Decides the window's cursor. The chrome owns one.
pub struct CursorArbiter {
    router: Arc<CursorRouter>,
    page_shell: Arc<PageShell>,
    /// The installed wrapper. `None` until the first event or poll.
    window: Option<Arc<WindowShell>>,
    /// What the window shows: `None` while unknown, `Some(None)` while the
    /// cursor is hidden. Only [`WindowShell::apply`] changes the window's
    /// cursor, so this record is exact.
    shown: Option<Option<CursorIcon>>,
    /// The pointer's last position, in the chrome's coordinates.
    pointer: Option<(f32, f32)>,
    /// The page host under the pointer at the last sync.
    hovered: Option<NodeId>,
}

impl CursorArbiter {
    pub fn new(wake: WakeHandle) -> CursorArbiter {
        let router = Arc::new(CursorRouter {
            dirty: AtomicBool::new(false),
            repaint: AtomicBool::new(false),
            wake,
        });
        CursorArbiter {
            page_shell: Arc::new(PageShell {
                router: Arc::clone(&router),
            }),
            router,
            window: None,
            shown: None,
            pointer: None,
            hovered: None,
        }
    }

    /// The provider for every page's `DocumentConfig`.
    pub fn page_shell(&self) -> Arc<dyn ShellProvider> {
        self.page_shell.clone()
    }

    /// Wrap the chrome's provider in a [`WindowShell`], unless it is wrapped
    /// already. blitz-shell sets its provider after the document is built
    /// and offers no hook, so the chrome calls this before handling any event
    /// or poll.
    pub fn install(&mut self, doc: &mut BaseDocument) {
        let installed = self.window.as_ref().is_some_and(|window| {
            std::ptr::addr_eq(Arc::as_ptr(&doc.shell_provider), Arc::as_ptr(window))
        });
        if installed {
            return;
        }
        let window = Arc::new(WindowShell {
            window: Arc::clone(&doc.shell_provider),
            router: Arc::clone(&self.router),
        });
        doc.set_shell_provider(window.clone());
        self.window = Some(window);
        // What this window shows is not known yet: the next sync sets it.
        self.shown = None;
        self.router.dirty.store(true, Ordering::Release);
    }

    /// Remember where the pointer is, for a page that comes under it later
    /// without the pointer moving.
    pub fn note_pointer(&mut self, event: &UiEvent) {
        match event {
            UiEvent::PointerMove(e)
            | UiEvent::PointerDown(e)
            | UiEvent::PointerUp(e)
            | UiEvent::PointerCancel(e) => self.pointer = Some((e.page_x(), e.page_y())),
            _ => {}
        }
    }

    /// Whether a hover changed since the last sync.
    pub fn needs_sync(&self) -> bool {
        self.router.dirty.load(Ordering::Acquire)
    }

    /// Whether a page asked to be painted since the last call.
    pub fn take_repaint(&self) -> bool {
        self.router.repaint.swap(false, Ordering::AcqRel)
    }

    /// Bring the pages' hover and the window's cursor up to date. This runs
    /// after the chrome, and any page under the pointer, have handled the
    /// event. Returns whether a page's hover changed, in which case its
    /// `:hover` styles need painting.
    ///
    /// 1. When the page host under the pointer has changed, the page the
    ///    pointer left stops hovering, and the page it is now over learns
    ///    where the pointer is. Only hover state changes: no DOM event is
    ///    dispatched, just as with Blitz's own `refresh_hover`.
    /// 2. The cursor is the one Blitz computes for the element under the
    ///    pointer, read through the page host when there is one. Blitz's
    ///    `None` means either `cursor: none` or "nothing hovered", and only
    ///    the first hides the cursor ([`window_cursor`]).
    /// 3. The window is told only when the cursor differs from what it
    ///    shows.
    pub fn sync(&mut self, doc: &mut BaseDocument) -> bool {
        let host = hovered_host(doc);
        let mut hover_changed = false;
        if host != self.hovered {
            if let Some(left) = self.hovered.take()
                && let Some(page) = doc.subdoc_mut(left)
            {
                hover_changed |= page.inner_mut().clear_hover();
            }
            if let (Some(entered), Some(pointer)) = (host, self.pointer) {
                hover_changed |= seed_hover(doc, entered, pointer);
            }
            self.hovered = host;
        }
        let want = window_cursor(doc.get_cursor(), hidden_by_css(doc));
        if self.shown != Some(want) {
            if let Some(window) = &self.window {
                window.apply(want);
            }
            self.shown = Some(want);
        }
        // The reports caused by the clearing and seeding above are covered by
        // this sync.
        self.router.dirty.store(false, Ordering::Release);
        hover_changed
    }

    /// A new document is now hosted by `host`. If it sits under the pointer,
    /// it learns where the pointer is. The pointer has not moved, so no
    /// event will tell it, and its first layout then hovers the right
    /// element (Blitz's `refresh_hover`).
    pub fn page_attached(&mut self, doc: &mut BaseDocument, host: NodeId) {
        if self.hovered == Some(host)
            && let Some(pointer) = self.pointer
        {
            seed_hover(doc, host, pointer);
            self.router.dirty.store(true, Ordering::Release);
        }
    }

    /// `host` is being removed from the chrome. Forget it, because Blitz
    /// reuses node ids.
    pub fn view_removed(&mut self, host: NodeId) {
        if self.hovered == Some(host) {
            self.hovered = None;
        }
    }
}

/// The page host (a node with a sub-document) under the pointer.
fn hovered_host(doc: &BaseDocument) -> Option<NodeId> {
    doc.get_hover_node_id()
        .filter(|&id| doc.subdoc(id).is_some())
}

/// Tell the page hosted by `host` where the pointer is. Returns whether its
/// hover changed: before the page's first layout nothing is hit yet, but the
/// position is kept for that layout.
fn seed_hover(doc: &mut BaseDocument, host: NodeId, pointer: (f32, f32)) -> bool {
    let Some(origin) = doc
        .get_node(host)
        .map(|node| node.absolute_position(0.0, 0.0))
    else {
        return false;
    };
    let Some(page) = doc.subdoc_mut(host) else {
        return false;
    };
    let mut page = page.inner_mut();
    let (x, y) = page_point(pointer, origin, page.viewport_scroll());
    page.set_hover_to(x, y)
}

/// Whether the element under the pointer hides the cursor with
/// `cursor: none`. Page hosts are followed down, as
/// `BaseDocument::get_cursor` follows them.
fn hidden_by_css(doc: &BaseDocument) -> bool {
    let Some(id) = doc.get_hover_node_id() else {
        return false;
    };
    match doc.subdoc(id) {
        Some(page) => hidden_by_css(&page.inner()),
        None => doc.resolved_style_value(id, "cursor") == "none",
    }
}

/// The cursor the window shows, given Blitz's answer `answer` and whether
/// the element under the pointer has `cursor: none`.
///
/// Blitz answers `None` in two cases: when CSS hides the cursor, and when
/// the document under the pointer has nothing hovered. A page is in the
/// second case until it has seen the pointer. Only the first should hide the
/// cursor, and in the second the pointer is over the page itself, which
/// shows the arrow.
pub fn window_cursor(answer: Option<CursorIcon>, hidden_by_css: bool) -> Option<CursorIcon> {
    match (answer, hidden_by_css) {
        (Some(icon), _) => Some(icon),
        (None, true) => None,
        (None, false) => Some(CursorIcon::Default),
    }
}

/// A point in the chrome, in the coordinates of a page whose host is at
/// `origin` and which has scrolled by `scroll`. This is the mapping Blitz
/// applies to the pointer events it forwards to a sub-document
/// (`adjust_coords_for_subdocument` in `blitz-dom/src/events/mod.rs`).
pub fn page_point(pointer: (f32, f32), origin: Point<f32>, scroll: Point<f64>) -> (f32, f32) {
    (
        pointer.0 - origin.x + scroll.x as f32,
        pointer.1 - origin.y + scroll.y as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn blitz_none_hides_the_cursor_only_for_css_none() {
        let cases = [
            (Some(CursorIcon::Pointer), false, Some(CursorIcon::Pointer)),
            (Some(CursorIcon::Pointer), true, Some(CursorIcon::Pointer)),
            (Some(CursorIcon::Text), false, Some(CursorIcon::Text)),
            (None, true, None),
            (None, false, Some(CursorIcon::Default)),
        ];
        for (answer, hidden, want) in cases {
            assert_eq!(
                window_cursor(answer, hidden),
                want,
                "{answer:?}, cursor: none = {hidden}"
            );
        }
    }

    #[test]
    fn page_points_match_blitz_forwarding() {
        let origin = Point { x: 342.0, y: 84.0 };
        assert_eq!(
            page_point((402.0, 184.0), origin, Point { x: 0.0, y: 0.0 }),
            (60.0, 100.0)
        );
        // Scrolled down 500 px: the same window point is 500 px further
        // down the page.
        assert_eq!(
            page_point((402.0, 184.0), origin, Point { x: 0.0, y: 500.0 }),
            (60.0, 600.0)
        );
    }

    /// Each request a `WindowShell` received from its document, by name.
    #[derive(Default)]
    struct Calls(Mutex<Vec<&'static str>>);

    impl Calls {
        fn push(&self, name: &'static str) {
            self.0.lock().expect("the call log").push(name);
        }
    }

    impl ShellProvider for Calls {
        fn request_redraw(&self) {
            self.push("request_redraw");
        }
        fn set_cursor(&self, _icon: Option<CursorIcon>) {
            self.push("set_cursor");
        }
        fn set_window_title(&self, _title: String) {
            self.push("set_window_title");
        }
        fn set_ime_enabled(&self, _is_enabled: bool) {
            self.push("set_ime_enabled");
        }
        fn set_ime_cursor_area(&self, _x: f32, _y: f32, _width: f32, _height: f32) {
            self.push("set_ime_cursor_area");
        }
        fn get_clipboard_text(&self) -> Result<String, ClipboardError> {
            self.push("get_clipboard_text");
            Ok("clip".into())
        }
        fn set_clipboard_text(&self, _text: String) -> Result<(), ClipboardError> {
            self.push("set_clipboard_text");
            Ok(())
        }
        fn open_file_dialog(
            &self,
            _multiple: bool,
            _filter: Option<FileDialogFilter>,
        ) -> Vec<PathBuf> {
            self.push("open_file_dialog");
            vec![PathBuf::from("chosen")]
        }
        fn request_window_close(&self) {
            self.push("request_window_close");
        }
        fn set_window_minimized(&self, _minimized: bool) {
            self.push("set_window_minimized");
        }
        fn set_window_maximized(&self, _maximized: bool) {
            self.push("set_window_maximized");
        }
        fn is_window_maximized(&self) -> bool {
            self.push("is_window_maximized");
            true
        }
        fn set_window_decorations(&self, _decorations: bool) {
            self.push("set_window_decorations");
        }
        fn drag_window(&self) {
            self.push("drag_window");
        }
    }

    /// L8: the chrome's wrapper keeps every window request except the
    /// cursor, including the answers of those that return one.
    #[test]
    fn window_shell_forwards_everything_but_the_cursor() {
        let calls = Arc::new(Calls::default());
        let router = Arc::new(CursorRouter {
            dirty: AtomicBool::new(false),
            repaint: AtomicBool::new(false),
            wake: WakeHandle::default(),
        });
        let shell = WindowShell {
            window: calls.clone(),
            router: Arc::clone(&router),
        };
        shell.request_redraw();
        shell.set_cursor(Some(CursorIcon::Pointer));
        shell.set_window_title("title".into());
        shell.set_ime_enabled(true);
        shell.set_ime_cursor_area(1.0, 2.0, 3.0, 4.0);
        assert_eq!(shell.get_clipboard_text().ok().as_deref(), Some("clip"));
        assert!(shell.set_clipboard_text("text".into()).is_ok());
        assert_eq!(
            shell.open_file_dialog(false, None),
            vec![PathBuf::from("chosen")]
        );
        shell.request_window_close();
        shell.set_window_minimized(true);
        shell.set_window_maximized(true);
        assert!(shell.is_window_maximized());
        shell.set_window_decorations(false);
        shell.drag_window();
        assert_eq!(
            *calls.0.lock().expect("the call log"),
            [
                "request_redraw",
                "set_window_title",
                "set_ime_enabled",
                "set_ime_cursor_area",
                "get_clipboard_text",
                "set_clipboard_text",
                "open_file_dialog",
                "request_window_close",
                "set_window_minimized",
                "set_window_maximized",
                "is_window_maximized",
                "set_window_decorations",
                "drag_window",
            ],
            "every request but set_cursor reaches the window"
        );
        assert!(
            router.dirty.load(Ordering::Acquire),
            "set_cursor became a request to recompute the cursor"
        );
    }

    /// L8: a page's provider gives it no window capability. It only reports.
    #[test]
    fn page_shell_only_reports() {
        let arbiter = CursorArbiter::new(WakeHandle::default());
        let page = arbiter.page_shell();
        page.set_window_title("a page's title".into());
        assert!(page.get_clipboard_text().is_err(), "no clipboard");
        assert!(
            page.open_file_dialog(true, None).is_empty(),
            "no file dialog"
        );
        assert!(!page.is_window_maximized());
        assert!(
            !arbiter.needs_sync() && !arbiter.take_repaint(),
            "nothing reported yet"
        );
        page.set_cursor(None);
        page.request_redraw();
        assert!(arbiter.needs_sync(), "a hover change is reported");
        assert!(arbiter.take_repaint(), "a redraw request is reported");
        assert!(!arbiter.take_repaint(), "and taken once");
    }
}
