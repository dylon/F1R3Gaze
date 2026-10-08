//! CI probe: read the host's real system appearance through the same winit
//! event-loop API that F1R3Gaze uses when it opens a window.
//! Run on macOS or Windows with an expected `light` or `dark` argument.

#[cfg(any(target_os = "macos", windows))]
mod native {
    use gaze_shell::system_theme::{Known, Source, SystemScheme};
    use gaze_shell::theme::Scheme;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Instant;
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Theme, WindowId};

    struct Probe {
        expected: Scheme,
        observed: Arc<AtomicBool>,
    }

    impl ApplicationHandler for Probe {
        fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
            let native = event_loop
                .system_theme()
                .expect("macOS and Windows must report a system theme");
            let preference = match native {
                Theme::Dark => Scheme::Dark,
                Theme::Light => Scheme::Light,
            };
            assert_eq!(
                preference, self.expected,
                "winit disagrees with the host's appearance setting"
            );
            assert_eq!(Source::choose(None, true), Ok(Source::Window));
            let system = SystemScheme::start(&Source::Window, None, Instant::now());
            system.set(Some(preference));
            assert_eq!(system.known(), Known::Answered(Some(self.expected)));
            assert_eq!(system.effective(), self.expected);
            self.observed.store(true, Ordering::SeqCst);
            println!("native system appearance: {preference:?}");
            event_loop.exit();
        }

        fn window_event(&mut self, _: &dyn ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let expected = match std::env::args().nth(1).as_deref() {
            Some("dark") => Scheme::Dark,
            Some("light") => Scheme::Light,
            _ => return Err("usage: native_system_theme light|dark".into()),
        };
        let observed = Arc::new(AtomicBool::new(false));
        EventLoop::builder().build()?.run_app(Probe {
            expected,
            observed: Arc::clone(&observed),
        })?;
        if !observed.load(Ordering::SeqCst) {
            return Err("winit did not report the system appearance".into());
        }
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(any(target_os = "macos", windows))]
    return native::run();
    #[cfg(not(any(target_os = "macos", windows)))]
    Err("native_system_theme is for macOS and Windows; Linux uses the XDG portal tests".into())
}
