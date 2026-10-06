//! The window's renderer: Vello's, with surface resizes coalesced
//! (docs/ui/ledger.md, L9).
//!
//! blitz-shell calls `WindowRenderer::set_size` for every `SurfaceResized`
//! event (`View::with_viewport`). Vello's `set_size` reconfigures the surface
//! and allocates a new target texture each time
//! (`wgpu_context::SurfaceRenderer::resize`). While a window edge is dragged,
//! several resizes can arrive between two frames, and only the size in force at
//! the next `render` is ever seen. When coalescing, [`CoalescingRenderer`]
//! records the latest size and applies it once, just before rendering.
//!
//! With the `frame-times` feature it also prints one line per frame on stdout,
//! next to Blitz's resolve phases and Vello's frame phases. In such a build
//! the environment variable `F1R3GAZE_COALESCE_RESIZE` (`0` or `1`) overrides
//! the default, so one binary can measure both behaviours.

use anyrender::{RegisterResourceError, RenderContext, ResourceId, WindowHandle, WindowRenderer};
use std::any::Any;
use std::sync::Arc;
#[cfg(feature = "frame-times")]
use std::time::{Duration, Instant};

/// Whether resizes are coalesced unless a profiling build says otherwise.
/// On: under X11 several resizes arrive between two frames, and coalescing
/// them cut reconfigures from 16 to 0.95 per frame in the Xvfb sweep (3 → 18
/// frames a second). Wayland and macOS deliver one resize per frame, so
/// there it changes nothing (ledger L9, H1).
const COALESCE_BY_DEFAULT: bool = true;

/// A window renderer that can defer `set_size` to the next `render`.
pub struct CoalescingRenderer<R> {
    inner: R,
    /// Defer `set_size` to the next render, or pass every call through as
    /// blitz-shell makes it.
    coalesce: bool,
    /// The size the inner renderer was last given.
    applied: Option<(u32, u32)>,
    /// A size asked for since then and not yet given (coalescing only).
    pending: Option<(u32, u32)>,
    #[cfg(feature = "frame-times")]
    log: FrameLog,
}

impl<R> CoalescingRenderer<R> {
    pub fn new(inner: R) -> Self {
        Self::with_coalescing(inner, coalesce_default())
    }

    pub fn with_coalescing(inner: R, coalesce: bool) -> Self {
        CoalescingRenderer {
            inner,
            coalesce,
            applied: None,
            pending: None,
            #[cfg(feature = "frame-times")]
            log: FrameLog::new(),
        }
    }
}

#[cfg(feature = "frame-times")]
fn coalesce_default() -> bool {
    match std::env::var("F1R3GAZE_COALESCE_RESIZE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => COALESCE_BY_DEFAULT,
    }
}

#[cfg(not(feature = "frame-times"))]
fn coalesce_default() -> bool {
    COALESCE_BY_DEFAULT
}

impl<R: RenderContext> RenderContext for CoalescingRenderer<R> {
    fn try_register_custom_resource(
        &mut self,
        resource: Box<dyn Any>,
    ) -> Result<ResourceId, RegisterResourceError> {
        self.inner.try_register_custom_resource(resource)
    }

    fn unregister_resource(&mut self, resource_id: ResourceId) {
        self.inner.unregister_resource(resource_id);
    }

    fn renderer_specific_context(&self) -> Option<Box<dyn Any>> {
        self.inner.renderer_specific_context()
    }
}

// `'static`: `render`'s painter borrows the renderer for any lifetime, and
// every window renderer (Vello's included) owns its state.
impl<R: WindowRenderer + 'static> WindowRenderer for CoalescingRenderer<R> {
    type ScenePainter<'a>
        = R::ScenePainter<'a>
    where
        Self: 'a;

    fn resume<F: FnOnce() + 'static>(
        &mut self,
        window: Arc<dyn WindowHandle>,
        width: u32,
        height: u32,
        on_ready: F,
    ) {
        self.applied = Some((width, height));
        self.pending = None;
        self.inner.resume(window, width, height, on_ready);
    }

    fn complete_resume(&mut self) -> bool {
        self.inner.complete_resume()
    }

    fn suspend(&mut self) {
        self.inner.suspend();
    }

    fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    fn is_pending(&self) -> bool {
        self.inner.is_pending()
    }

    fn set_size(&mut self, width: u32, height: u32) {
        #[cfg(feature = "frame-times")]
        self.log.resize_asked();
        let size = (width, height);
        // A renderer that is not active yet ignores sizes: Vello does until
        // its resume completes, and blitz-shell then sets the size again
        // (`View::complete_resume`). Until then sizes pass straight through,
        // so none is mistaken for one already applied.
        match self.coalesce && self.inner.is_active() {
            true => {
                self.pending = match self.applied == Some(size) {
                    true => None,
                    false => Some(size),
                };
            }
            false => self.apply(size),
        }
    }

    fn render<F: FnOnce(&mut Self::ScenePainter<'_>)>(&mut self, draw_fn: F) {
        if let Some(size) = self.pending.take() {
            self.apply(size);
        }
        #[cfg(feature = "frame-times")]
        let started = Instant::now();
        self.inner.render(draw_fn);
        #[cfg(feature = "frame-times")]
        self.log
            .frame(started.elapsed(), self.applied, self.coalesce);
    }
}

impl<R: WindowRenderer> CoalescingRenderer<R> {
    /// Give the inner renderer `size`. Only an active renderer acts on it,
    /// so only then is it recorded as applied.
    fn apply(&mut self, size: (u32, u32)) {
        #[cfg(feature = "frame-times")]
        let started = Instant::now();
        self.inner.set_size(size.0, size.1);
        if self.inner.is_active() {
            self.applied = Some(size);
        }
        #[cfg(feature = "frame-times")]
        self.log.reconfigured(started.elapsed());
    }
}

/// One stdout line per frame, for the `frame-times` profiling build.
#[cfg(feature = "frame-times")]
struct FrameLog {
    started: Instant,
    frame: u64,
    last_frame: Option<Instant>,
    /// `set_size` calls since the last frame.
    resizes: u32,
    /// When the first of them was made.
    first_resize: Option<Instant>,
    /// Time spent in the inner `set_size` since the last frame.
    reconfigure: Duration,
    /// Inner `set_size` calls since the last frame.
    reconfigures: u32,
}

#[cfg(feature = "frame-times")]
impl FrameLog {
    fn new() -> FrameLog {
        FrameLog {
            started: Instant::now(),
            frame: 0,
            last_frame: None,
            resizes: 0,
            first_resize: None,
            reconfigure: Duration::ZERO,
            reconfigures: 0,
        }
    }

    fn resize_asked(&mut self) {
        self.resizes += 1;
        self.first_resize.get_or_insert_with(Instant::now);
    }

    fn reconfigured(&mut self, took: Duration) {
        self.reconfigure += took;
        self.reconfigures += 1;
    }

    fn frame(&mut self, render: Duration, size: Option<(u32, u32)>, coalesce: bool) {
        use crate::frame_stats::{CHROME_REDRAW, CURSOR_SET, EVENT, PAGE_REDRAW, POLL, RENDER, WINDOW_SAVE};
        use std::io::Write as _;
        let now = Instant::now();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let interval = self.last_frame.map(|last| ms(now - last)).unwrap_or(0.0);
        let latency = self.first_resize.map(|first| ms(now - first)).unwrap_or(0.0);
        let (width, height) = size.unwrap_or((0, 0));
        let (polls, poll_ms) = POLL.take();
        let (renders, render_ms) = RENDER.take();
        let (events, event_ms) = EVENT.take();
        let (page_redraws, _) = PAGE_REDRAW.take();
        let (chrome_redraws, _) = CHROME_REDRAW.take();
        let (cursor_sets, _) = CURSOR_SET.take();
        let (window_saves, window_save_ms) = WINDOW_SAVE.take();
        // Wall-clock milliseconds, to line frames up with logs taken outside
        // the process (the GPU's clocks, for one: ledger L9, H10). Was
        // computed here; frame_stats::wall_clock_ms now also times the
        // window's readings (storage ledger S13, part 2).
        // let epoch = std::time::SystemTime::now()
        //     .duration_since(std::time::UNIX_EPOCH)
        //     .map(|since| since.as_millis())
        //     .unwrap_or(0);
        let epoch = crate::frame_stats::wall_clock_ms();
        let line = format!(
            "frame n={} t={:.1} interval={interval:.2} size={width}x{height} resizes={} reconfigures={} \
             reconfigure={:.2} render={:.2} latency={latency:.2} polls={polls} poll={poll_ms:.2} \
             chrome_renders={renders} chrome_render={render_ms:.2} events={events} event={event_ms:.2} \
             page_redraws={page_redraws} chrome_redraws={chrome_redraws} cursor_sets={cursor_sets} \
             window_saves={window_saves} window_save={window_save_ms:.2} epoch={epoch} coalesce={}",
            self.frame,
            ms(now - self.started),
            self.resizes,
            self.reconfigures,
            ms(self.reconfigure),
            ms(render),
            u8::from(coalesce),
        );
        // A full or closed pipe must not take the window down.
        let _ = writeln!(std::io::stdout().lock(), "{line}");
        self.frame += 1;
        self.last_frame = Some(now);
        self.resizes = 0;
        self.first_resize = None;
        self.reconfigure = Duration::ZERO;
        self.reconfigures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::NullScenePainter;

    /// A window renderer that records what it is asked.
    #[derive(Default)]
    struct Recorder {
        sizes: Vec<(u32, u32)>,
        renders: usize,
        calls: Vec<&'static str>,
        /// Not active yet, as Vello is until its resume completes.
        inactive: bool,
        /// The surface's size. As in Vello, `set_size` changes it only while
        /// the renderer is active.
        surface: Option<(u32, u32)>,
    }

    impl RenderContext for Recorder {
        fn try_register_custom_resource(
            &mut self,
            _resource: Box<dyn Any>,
        ) -> Result<ResourceId, RegisterResourceError> {
            self.calls.push("try_register_custom_resource");
            Err(anyrender::RegisterResourceErrorKind::Unimplemented.into())
        }
        fn unregister_resource(&mut self, _resource_id: ResourceId) {
            self.calls.push("unregister_resource");
        }
        fn renderer_specific_context(&self) -> Option<Box<dyn Any>> {
            Some(Box::new("recorder"))
        }
    }

    impl WindowRenderer for Recorder {
        type ScenePainter<'a>
            = NullScenePainter
        where
            Self: 'a;

        fn resume<F: FnOnce() + 'static>(
            &mut self,
            _window: Arc<dyn WindowHandle>,
            _width: u32,
            _height: u32,
            on_ready: F,
        ) {
            self.calls.push("resume");
            on_ready();
        }
        fn complete_resume(&mut self) -> bool {
            self.calls.push("complete_resume");
            self.inactive = false;
            true
        }
        fn suspend(&mut self) {
            self.calls.push("suspend");
        }
        fn is_active(&self) -> bool {
            !self.inactive
        }
        fn is_pending(&self) -> bool {
            true
        }
        fn set_size(&mut self, width: u32, height: u32) {
            self.sizes.push((width, height));
            if !self.inactive {
                self.surface = Some((width, height));
            }
        }
        fn render<F: FnOnce(&mut Self::ScenePainter<'_>)>(&mut self, draw_fn: F) {
            draw_fn(&mut NullScenePainter);
            self.renders += 1;
        }
    }

    fn draw(_: &mut NullScenePainter) {}

    #[test]
    fn coalescing_gives_only_the_last_size_once_per_frame() {
        let mut renderer = CoalescingRenderer::with_coalescing(Recorder::default(), true);
        for width in [1000, 1010, 1020, 1030] {
            renderer.set_size(width, 700);
        }
        assert!(renderer.inner.sizes.is_empty(), "nothing is applied before the frame");
        renderer.render(draw);
        assert_eq!(renderer.inner.sizes, [(1030, 700)], "one reconfiguration, the last size");
        assert_eq!(renderer.inner.renders, 1);
        renderer.set_size(1030, 700);
        renderer.render(draw);
        assert_eq!(renderer.inner.sizes, [(1030, 700)], "an unchanged size is not applied again");
        renderer.set_size(1040, 700);
        renderer.set_size(1030, 700);
        renderer.render(draw);
        assert_eq!(
            renderer.inner.sizes,
            [(1030, 700)],
            "a resize that comes back to the applied size before the frame costs nothing"
        );
    }

    /// Vello ignores `set_size` until its resume completes, and blitz-shell
    /// then sets the size again (`View::complete_resume`). A size asked for
    /// while resuming must not count as applied, or that second request
    /// would look like a repeat and be dropped. The snapshot harness caught
    /// this: four scenes were painted at the size the window opened with
    /// (ledger L9).
    #[test]
    fn a_size_asked_for_while_resuming_reaches_the_renderer_once_active() {
        let mut renderer = CoalescingRenderer::with_coalescing(
            Recorder {
                inactive: true,
                ..Recorder::default()
            },
            true,
        );
        // The window is resized while the renderer is still resuming, and
        // something asks for a frame meanwhile.
        renderer.set_size(1280, 800);
        renderer.render(draw);
        assert_eq!(renderer.inner.surface, None, "ignored while resuming");
        // What blitz-shell does once the resume completes.
        assert!(renderer.complete_resume());
        renderer.set_size(1280, 800);
        renderer.render(draw);
        assert_eq!(renderer.inner.surface, Some((1280, 800)));
    }

    #[test]
    fn without_coalescing_every_size_passes_through() {
        let mut renderer = CoalescingRenderer::with_coalescing(Recorder::default(), false);
        renderer.set_size(1000, 700);
        renderer.set_size(1010, 700);
        assert_eq!(renderer.inner.sizes, [(1000, 700), (1010, 700)], "as blitz-shell asks");
        renderer.render(draw);
        assert_eq!(renderer.inner.sizes.len(), 2, "nothing more at the frame");
    }

    #[test]
    fn every_other_call_is_delegated() {
        let mut renderer = CoalescingRenderer::with_coalescing(Recorder::default(), true);
        assert!(renderer.try_register_custom_resource(Box::new(1u8)).is_err());
        renderer.unregister_resource(ResourceId::new());
        assert!(
            renderer
                .renderer_specific_context()
                .is_some_and(|context| context.downcast_ref::<&str>() == Some(&"recorder"))
        );
        assert!(renderer.complete_resume());
        renderer.suspend();
        assert!(renderer.is_active() && renderer.is_pending());
        assert_eq!(
            renderer.inner.calls,
            ["try_register_custom_resource", "unregister_resource", "complete_resume", "suspend"]
        );
    }
}
