//! The operating system's light or dark preference.
//!
//! macOS and Windows report it through winit (`ActiveEventLoop::system_theme`
//! and `WindowEvent::ThemeChanged`); the window hands it to [`SystemScheme::set`].
//! Elsewhere winit reports nothing, and the preference is read from the XDG
//! desktop portal's `org.freedesktop.appearance` `color-scheme` setting by
//! running `dbus-send` (no shell, a fixed argument list):
//!
//! ```text
//! dbus-send --session --print-reply=literal --reply-timeout=250
//!   --dest=org.freedesktop.portal.Desktop /org/freedesktop/portal/desktop
//!   org.freedesktop.portal.Settings.ReadOne
//!   string:org.freedesktop.appearance string:color-scheme
//! ```
//!
//! The value is 0 for no preference, 1 for dark and 2 for light; any other
//! value means no preference, as the portal's documentation says. A portal
//! older than `ReadOne` answers `UnknownMethod`; the query is then made
//! again with `Read`, whose answer is wrapped in one more variant. A portal
//! without the setting answers `NotFound`: no preference. No bus, no portal
//! or no answer leaves the preference unknown until the next query. If
//! `dbus-send` is not installed, no query is made again this session.
//!
//! Queries run on the `gaze-system-theme` thread, one at a time, each
//! killed after [`QUERY_DEADLINE`]. The window asks again when it gains the
//! focus, at most every [`REQUERY_INTERVAL`]. At start-up the main thread
//! waits at most [`STARTUP_WAIT`] for the first answer. A preference that
//! is unknown or absent means dark.
//!
//! `F1R3GAZE_SYSTEM_THEME=dark|light|none` fixes the preference, for tests
//! and the snapshot harness.

use crate::theme::Scheme;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The environment variable that fixes the preference.
pub const OVERRIDE_VAR: &str = "F1R3GAZE_SYSTEM_THEME";

/// How long one `dbus-send` may run before it is killed.
pub const QUERY_DEADLINE: Duration = Duration::from_secs(1);

/// How often the window may ask again.
pub const REQUERY_INTERVAL: Duration = Duration::from_secs(2);

/// How long start-up waits for the first answer, before the window exists.
pub const STARTUP_WAIT: Duration = Duration::from_millis(100);

/// How often a running `dbus-send` is checked on.
const POLL: Duration = Duration::from_millis(10);

/// How many times a busy program is tried again.
const SPAWN_RETRIES: usize = 5;

/// The portal's methods, newest first.
const READ_ONE: &str = "org.freedesktop.portal.Settings.ReadOne";
const READ: &str = "org.freedesktop.portal.Settings.Read";

/// The arguments of the query, for `method`.
pub fn portal_args(method: &str) -> [&str; 8] {
    [
        "--session",
        "--print-reply=literal",
        "--reply-timeout=250",
        "--dest=org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        method,
        "string:org.freedesktop.appearance",
        "string:color-scheme",
    ]
}

/// The number in a literal reply: `variant uint32 N`, with one more
/// `variant` from `Read`.
pub fn parse_reply(stdout: &str) -> Option<u32> {
    let mut words = stdout.split_whitespace().skip_while(|w| *w == "variant");
    match (words.next(), words.next(), words.next()) {
        (Some("uint32"), Some(n), None) => n.parse().ok(),
        _ => None,
    }
}

/// What one run of `dbus-send` said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The setting's value.
    Value(u32),
    /// The portal has no such setting.
    NotFound,
    /// The portal does not have the method.
    UnknownMethod,
    /// Anything else: no bus, no portal, no answer in time, a reply that is
    /// not a number.
    Failed(String),
}

/// Reads a finished run's exit status and output.
pub fn classify(succeeded: bool, stdout: &str, stderr: &str) -> Reply {
    match succeeded {
        true => match parse_reply(stdout) {
            Some(n) => Reply::Value(n),
            None => Reply::Failed(format!("an answer that is not a colour scheme: {:?}", stdout.trim())),
        },
        false if stderr.contains("org.freedesktop.portal.Error.NotFound") => Reply::NotFound,
        false if stderr.contains("org.freedesktop.DBus.Error.UnknownMethod") => Reply::UnknownMethod,
        false => Reply::Failed(stderr.lines().next().unwrap_or("dbus-send failed without a message").trim().to_string()),
    }
}

/// The result of asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The preference: a scheme, or none for "no preference".
    Answered(Option<Scheme>),
    /// No answer this time; asked again on the next query.
    Unknown(String),
    /// Asking is not possible this session (`dbus-send` is not installed).
    Unavailable(String),
}

/// The preference a portal value stands for. The portal's documentation:
/// "Unknown values should be treated as 0 (no preference)."
pub fn outcome_of(value: u32) -> Outcome {
    match value {
        1 => Outcome::Answered(Some(Scheme::Dark)),
        2 => Outcome::Answered(Some(Scheme::Light)),
        _ => Outcome::Answered(None),
    }
}

/// How a run of a program ended.
enum Ran {
    Exited { succeeded: bool, stdout: String, stderr: String },
    TimedOut,
    Missing(String),
    Failed(String),
}

/// Runs `program` with `args`, killing and reaping it at `deadline`.
fn run(program: &Path, args: &[&str], deadline: Duration) -> Ran {
    let spawn = || {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };
    // A program being replaced (an upgrade) is briefly busy: try again.
    let mut spawned = spawn();
    for _ in 0..SPAWN_RETRIES {
        match &spawned {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(POLL);
                spawned = spawn();
            }
            _ => break,
        }
    }
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ran::Missing(format!("{} is not installed", program.display()));
        }
        Err(e) => return Ran::Failed(format!("{} cannot be run: {e}", program.display())),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    // Whatever could not be read is simply not part of the reply.
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                return Ran::Exited {
                    succeeded: status.success(),
                    stdout,
                    stderr,
                };
            }
            Ok(None) if started.elapsed() >= deadline => {
                // kill fails only if the child has just exited; wait reaps
                // it either way.
                let _ = child.kill();
                let _ = child.wait();
                return Ran::TimedOut;
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::Failed(format!("{} cannot be waited for: {e}", program.display()));
            }
        }
    }
}

/// Asks the portal through `program` (`dbus-send`), with `ReadOne`, then
/// with `Read` if the portal is too old for `ReadOne`.
pub fn query_portal(program: &Path, deadline: Duration) -> Outcome {
    for method in [READ_ONE, READ] {
        let reply = match run(program, &portal_args(method), deadline) {
            Ran::Exited { succeeded, stdout, stderr } => classify(succeeded, &stdout, &stderr),
            Ran::TimedOut => Reply::Failed(format!("no answer within {} ms", deadline.as_millis())),
            Ran::Missing(why) => return Outcome::Unavailable(why),
            Ran::Failed(why) => Reply::Failed(why),
        };
        match reply {
            Reply::Value(n) => return outcome_of(n),
            Reply::NotFound => return Outcome::Answered(None),
            Reply::UnknownMethod if method == READ_ONE => continue,
            Reply::UnknownMethod => return Outcome::Unknown("the portal has no Settings.Read".into()),
            Reply::Failed(why) => return Outcome::Unknown(why),
        }
    }
    Outcome::Unknown("the portal has neither Settings.ReadOne nor Settings.Read".into())
}

/// Where the preference comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// `F1R3GAZE_SYSTEM_THEME`: a fixed preference.
    Fixed(Option<Scheme>),
    /// The window system tells the window (macOS, Windows).
    Window,
    /// The XDG desktop portal, through this program.
    Portal(PathBuf),
}

impl Source {
    /// The source for this platform, given `F1R3GAZE_SYSTEM_THEME`'s value.
    /// A value other than `dark`, `light` and `none` is an error.
    pub fn choose(override_value: Option<&str>, window_system_reports: bool) -> Result<Source, String> {
        match override_value {
            Some("dark") => Ok(Source::Fixed(Some(Scheme::Dark))),
            Some("light") => Ok(Source::Fixed(Some(Scheme::Light))),
            Some("none") => Ok(Source::Fixed(None)),
            Some(other) => Err(format!("{OVERRIDE_VAR}={other:?} is not dark, light or none")),
            None if window_system_reports => Ok(Source::Window),
            None => Ok(Source::Portal(PathBuf::from("dbus-send"))),
        }
    }
}

/// What is known of the preference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Known {
    /// Nothing yet, or the last query had no answer (and why).
    Unknown(Option<String>),
    /// The preference: a scheme, or none for "no preference".
    Answered(Option<Scheme>),
}

impl Known {
    /// The scheme to use: dark unless the system prefers light.
    pub fn effective(&self) -> Scheme {
        match self {
            Known::Answered(Some(scheme)) => *scheme,
            Known::Answered(None) | Known::Unknown(_) => Scheme::Dark,
        }
    }
}

/// Runs one query; `query_portal` in use, a stand-in in tests.
pub type Runner = Arc<dyn Fn() -> Outcome + Send + Sync>;

/// Called on the query thread when an answer arrives, to wake the window.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

struct State {
    known: Known,
    /// A query is running.
    querying: bool,
    /// When the last query started.
    asked: Option<Instant>,
    /// No more queries this session.
    stopped: bool,
    /// Answers so far, so a waiter sees a new one.
    answers: u64,
}

struct Shared {
    state: Mutex<State>,
    answered: Condvar,
    runner: Option<Runner>,
    wake: Option<Wake>,
}

/// The system's preference, shared by the window and the query thread.
#[derive(Clone)]
pub struct SystemScheme(Arc<Shared>);

impl std::fmt::Debug for SystemScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SystemScheme").field(&self.known()).finish()
    }
}

impl SystemScheme {
    fn new(known: Known, runner: Option<Runner>, wake: Option<Wake>) -> SystemScheme {
        SystemScheme(Arc::new(Shared {
            state: Mutex::new(State {
                known,
                querying: false,
                asked: None,
                stopped: false,
                answers: 0,
            }),
            answered: Condvar::new(),
            runner,
            wake,
        }))
    }

    /// A preference that never changes by itself: the override, tests, and
    /// the window systems that report it through winit (then
    /// [`SystemScheme::set`] changes it).
    pub fn fixed(known: Known) -> SystemScheme {
        SystemScheme::new(known, None, None)
    }

    /// The preference from `source`; a portal query starts at once.
    pub fn start(source: &Source, wake: Option<Wake>, now: Instant) -> SystemScheme {
        match source {
            Source::Fixed(preference) => SystemScheme::fixed(Known::Answered(*preference)),
            Source::Window => SystemScheme::fixed(Known::Unknown(None)),
            Source::Portal(program) => {
                let program = program.clone();
                let runner: Runner = Arc::new(move || query_portal(&program, QUERY_DEADLINE));
                SystemScheme::with_runner(runner, wake, now)
            }
        }
    }

    /// A preference read by `runner`, which starts at once.
    pub fn with_runner(runner: Runner, wake: Option<Wake>, now: Instant) -> SystemScheme {
        let scheme = SystemScheme::new(Known::Unknown(None), Some(runner), wake);
        scheme.ask(now, false);
        scheme
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().expect("the system scheme's lock is never poisoned")
    }

    pub fn known(&self) -> Known {
        self.lock().known.clone()
    }

    /// The scheme to use now.
    pub fn effective(&self) -> Scheme {
        self.known().effective()
    }

    /// Whether queries have stopped for this session.
    pub fn stopped(&self) -> bool {
        self.lock().stopped
    }

    /// Records a preference the window system reported.
    pub fn set(&self, preference: Option<Scheme>) {
        let mut state = self.lock();
        state.known = Known::Answered(preference);
        state.answers += 1;
        self.0.answered.notify_all();
    }

    /// Asks again, unless a query is running, the last one started less
    /// than [`REQUERY_INTERVAL`] ago, or asking has stopped. Returns
    /// whether a query started.
    pub fn refresh(&self, now: Instant) -> bool {
        self.ask(now, true)
    }

    fn ask(&self, now: Instant, throttled: bool) -> bool {
        let Some(runner) = self.0.runner.clone() else {
            return false;
        };
        {
            let mut state = self.lock();
            let recent = state.asked.is_some_and(|asked| now.saturating_duration_since(asked) < REQUERY_INTERVAL);
            if state.stopped || state.querying || (throttled && recent) {
                return false;
            }
            state.querying = true;
            state.asked = Some(now);
        }
        let shared = Arc::clone(&self.0);
        let spawned = std::thread::Builder::new().name("gaze-system-theme".into()).spawn(move || {
            let outcome = runner();
            finish(&shared, outcome);
        });
        match spawned {
            Ok(_) => true,
            Err(e) => {
                finish(&self.0, Outcome::Unknown(format!("the query thread cannot start: {e}")));
                false
            }
        }
    }

    /// Waits at most `budget` for an answer newer than any seen before the
    /// call, unless none is coming; returns what is known then.
    pub fn wait(&self, budget: Duration) -> Known {
        let deadline = Instant::now() + budget;
        let mut state = self.lock();
        let seen = state.answers;
        while state.querying && state.answers == seen {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .0
                .answered
                .wait_timeout(state, left)
                .expect("the system scheme's lock is never poisoned")
                .0;
        }
        state.known.clone()
    }
}

/// Records a query's outcome and wakes whoever waits for it.
fn finish(shared: &Shared, outcome: Outcome) {
    {
        let mut state = shared.state.lock().expect("the system scheme's lock is never poisoned");
        state.querying = false;
        state.answers += 1;
        match outcome {
            Outcome::Answered(preference) => state.known = Known::Answered(preference),
            // An unknown answer keeps an earlier preference: one missed
            // reply does not turn a light desktop dark.
            Outcome::Unknown(why) => {
                if let Known::Unknown(_) = state.known {
                    state.known = Known::Unknown(Some(why));
                }
            }
            Outcome::Unavailable(why) => {
                state.stopped = true;
                if let Known::Unknown(_) = state.known {
                    state.known = Known::Unknown(Some(why));
                }
            }
        }
    }
    shared.answered.notify_all();
    if let Some(wake) = &shared.wake {
        wake();
    }
}

#[cfg(test)]
mod tests;
