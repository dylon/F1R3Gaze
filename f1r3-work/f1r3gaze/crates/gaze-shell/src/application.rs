//! The window's application handler: Blitz's, plus three things blitz-shell
//! leaves out (docs/ui/ledger.md, L9). A window resized by its frame had
//! painted each new size twice: once for the size, and once more for no
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
//! In a `frame-times` build, the environment variables
//! `F1R3GAZE_END_HOVER_ON_LEAVE=0`, `F1R3GAZE_ANSWER_IN_PAINT=0` and
//! `F1R3GAZE_POLL_ON_RESIZE=0` turn the three off, so one binary can measure
//! each behaviour with and without it.

use crate::chrome::ChromeDocument;
use anyrender::WindowRenderer;
use blitz_shell::BlitzApplication;
use std::any::Any;
use winit::application::ApplicationHandler;
use winit::application::macos::ApplicationHandlerExtMacOS;
use winit::event::{DeviceEvent, DeviceId, PointerKind, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

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
}

impl<R: WindowRenderer> ChromeApplication<R> {
    pub fn new(blitz: BlitzApplication<R>) -> Self {
        ChromeApplication {
            blitz,
            end_hover_on_leave: enabled("F1R3GAZE_END_HOVER_ON_LEAVE"),
            answer_in_paint: enabled("F1R3GAZE_ANSWER_IN_PAINT"),
            poll_on_resize: enabled("F1R3GAZE_POLL_ON_RESIZE"),
        }
    }

    /// The chrome shown in `window_id`, if that window shows one.
    fn chrome(&mut self, window_id: WindowId) -> Option<&mut ChromeDocument> {
        let view = self.blitz.windows.get_mut(&window_id)?;
        let doc: &mut dyn Any = view.doc.as_mut();
        doc.downcast_mut::<ChromeDocument>()
    }
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
        self.blitz.can_create_surfaces(event_loop);
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
