use super::*;
use crate::test_support::process_starts;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

// What `dbus-send` printed on this machine (KDE Plasma's portal,
// 2026-10-05), byte for byte. The captures and their sha256 sums are in
// target/scratch/storage/s12/portal/.
const READ_ONE_REPLY: &str = "   variant       uint32 1\n";
const READ_REPLY: &str = "   variant       variant          uint32 1\n";
const NOT_FOUND: &str = "Error org.freedesktop.portal.Error.NotFound: Requested setting not found\n";
const UNKNOWN_METHOD: &str = "Error org.freedesktop.DBus.Error.UnknownMethod: No such method \u{201c}ReadNothing\u{201d}\n";
const SERVICE_UNKNOWN: &str = "Error org.freedesktop.DBus.Error.ServiceUnknown: The name is not activatable\n";
const NO_BUS_MISSING: &str = "Failed to open connection to \"session\" message bus: Failed to connect to socket /run/user/1000/f1r3gaze-no-such-bus: No such file or directory\n";
const NO_BUS_REFUSED: &str = "Failed to open connection to \"session\" message bus: Failed to connect to socket f1r3gaze-no-such-bus: Connection refused\n";
const NO_BUS_LONG: &str = "Failed to open connection to \"session\" message bus: Socket name too long\n";
// The reply timeout's error, as libdbus words it (not captured: it needs a
// portal that does not answer).
const NO_REPLY: &str = "Error org.freedesktop.DBus.Error.NoReply: Did not receive a reply. Possible causes include: the remote application did not send a reply, the message bus security policy blocked the reply, the reply timeout expired, or the network connection was broken.\n";

#[test]
fn portal_replies_parse() {
    assert_eq!(parse_reply(READ_ONE_REPLY), Some(1));
    assert_eq!(parse_reply(READ_REPLY), Some(1), "Read wraps the value in one more variant");
    assert_eq!(parse_reply("variant uint32 2"), Some(2));
    assert_eq!(parse_reply("   variant       uint32 0\n"), Some(0));
    for other in ["", "variant", "variant string \"dark\"", "variant uint32", "variant uint32 1 2", "variant int32 1", "uint32 x"] {
        assert_eq!(parse_reply(other), None, "{other:?}");
    }
}

#[test]
fn errors_are_classified() {
    assert_eq!(classify(true, READ_ONE_REPLY, ""), Reply::Value(1));
    assert_eq!(classify(false, "", NOT_FOUND), Reply::NotFound);
    assert_eq!(classify(false, "", UNKNOWN_METHOD), Reply::UnknownMethod);
    for failure in [SERVICE_UNKNOWN, NO_BUS_MISSING, NO_BUS_REFUSED, NO_BUS_LONG, NO_REPLY] {
        match classify(false, "", failure) {
            Reply::Failed(why) => assert_eq!(why, failure.lines().next().expect("a line").trim()),
            other => panic!("{failure}: {other:?}"),
        }
    }
    assert!(matches!(classify(true, "   variant       string \"x\"\n", ""), Reply::Failed(_)));
    assert!(matches!(classify(false, "", ""), Reply::Failed(_)));
}

#[test]
fn values_map_to_schemes() {
    assert_eq!(outcome_of(0), Outcome::Answered(None));
    assert_eq!(outcome_of(1), Outcome::Answered(Some(Scheme::Dark)));
    assert_eq!(outcome_of(2), Outcome::Answered(Some(Scheme::Light)));
    // "Unknown values should be treated as 0 (no preference)."
    assert_eq!(outcome_of(3), Outcome::Answered(None));
    assert_eq!(outcome_of(u32::MAX), Outcome::Answered(None));
}

#[test]
fn the_query_is_the_documented_command() {
    assert_eq!(
        portal_args(READ_ONE),
        [
            "--session",
            "--print-reply=literal",
            "--reply-timeout=250",
            "--dest=org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings.ReadOne",
            "string:org.freedesktop.appearance",
            "string:color-scheme",
        ]
    );
    assert_eq!(portal_args(READ)[5], "org.freedesktop.portal.Settings.Read");
}

/// A stand-in `dbus-send`: a shell script that logs its arguments to
/// `<script>.log` and then runs `body`.
#[cfg(unix)]
fn fake_dbus_send(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.log\"\n{body}\n")).expect("write the script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[cfg(unix)]
#[test]
fn portal_answers_are_read_through_dbus_send() {
    let _starts = process_starts();
    for (value, expected) in [(1, Scheme::Dark), (2, Scheme::Light)] {
        let dir = gaze_fs::scratch_dir(&format!("gaze-shell-system-theme-answer-{value}"));
        let program = fake_dbus_send(&dir, "dbus-send", &format!("printf '   variant       uint32 {value}\\n'"));
        assert_eq!(query_portal(&program, QUERY_DEADLINE), Outcome::Answered(Some(expected)));
        let log = std::fs::read_to_string(dir.join("dbus-send.log")).expect("the log");
        assert_eq!(log.lines().collect::<Vec<_>>(), portal_args(READ_ONE), "exactly these arguments");
    }
}

#[cfg(unix)]
#[test]
fn an_old_portal_is_asked_again_with_read() {
    let dir = gaze_fs::scratch_dir("gaze-shell-system-theme-old");
    let body = "case \"$6\" in\n  *.ReadOne) echo 'Error org.freedesktop.DBus.Error.UnknownMethod: No such method \u{201c}ReadOne\u{201d}' >&2; exit 1;;\n  *.Read) printf '   variant       variant          uint32 2\\n';;\nesac";
    let program = fake_dbus_send(&dir, "dbus-send", body);
    let _starts = process_starts();
    assert_eq!(query_portal(&program, QUERY_DEADLINE), Outcome::Answered(Some(Scheme::Light)));
    let log = std::fs::read_to_string(dir.join("dbus-send.log")).expect("the log");
    let methods: Vec<&str> = log.lines().filter(|l| l.starts_with("org.freedesktop.portal.Settings.")).collect();
    assert_eq!(methods, [READ_ONE, READ]);
}

#[cfg(unix)]
#[test]
fn a_hung_dbus_send_is_killed_at_its_deadline() {
    let dir = gaze_fs::scratch_dir("gaze-shell-system-theme-hung");
    let program = fake_dbus_send(&dir, "dbus-send", "exec sleep 30");
    let _starts = process_starts();
    let started = Instant::now();
    let outcome = query_portal(&program, Duration::from_millis(200));
    let took = started.elapsed();
    assert_eq!(outcome, Outcome::Unknown("no answer within 200 ms".into()));
    assert!(took >= Duration::from_millis(200) && took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn a_missing_dbus_send_stops_the_queries() {
    let dir = gaze_fs::scratch_dir("gaze-shell-system-theme-missing");
    let missing = dir.join("no-dbus-send-here");
    let _starts = process_starts();
    assert!(matches!(query_portal(&missing, QUERY_DEADLINE), Outcome::Unavailable(_)));
    let now = Instant::now();
    let scheme = SystemScheme::start(&Source::Portal(missing), None, now);
    match scheme.wait(Duration::from_secs(5)) {
        Known::Unknown(Some(why)) => assert!(why.ends_with("is not installed"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(scheme.stopped());
    assert!(!scheme.refresh(now + REQUERY_INTERVAL * 10), "no query after dbus-send was found missing");
    assert_eq!(scheme.effective(), Scheme::Dark);
}

#[test]
fn the_override_fixes_the_preference() {
    assert_eq!(Source::choose(Some("dark"), false), Ok(Source::Fixed(Some(Scheme::Dark))));
    assert_eq!(Source::choose(Some("light"), true), Ok(Source::Fixed(Some(Scheme::Light))));
    assert_eq!(Source::choose(Some("none"), false), Ok(Source::Fixed(None)));
    assert!(Source::choose(Some("Dark"), false).is_err());
    assert_eq!(Source::choose(None, true), Ok(Source::Window));
    assert_eq!(Source::choose(None, false), Ok(Source::Portal(PathBuf::from("dbus-send"))));
    let fixed = SystemScheme::start(&Source::Fixed(Some(Scheme::Light)), None, Instant::now());
    assert_eq!(fixed.known(), Known::Answered(Some(Scheme::Light)));
    assert!(!fixed.refresh(Instant::now()), "a fixed preference is never asked for");
}

/// A runner that answers `answers` in turn, each only once the test
/// releases it, and counts its calls.
fn gated(answers: Vec<Outcome>) -> (Runner, mpsc::Sender<()>, Arc<AtomicUsize>) {
    let (release, gate) = mpsc::channel::<()>();
    let gate = Arc::new(Mutex::new(gate));
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let runner: Runner = Arc::new(move || {
        let n = counted.fetch_add(1, Ordering::SeqCst);
        gate.lock().expect("gate").recv().expect("released");
        answers[n.min(answers.len() - 1)].clone()
    });
    (runner, release, calls)
}

#[test]
fn asking_again_is_throttled_and_one_at_a_time() {
    let (runner, release, calls) = gated(vec![Outcome::Answered(Some(Scheme::Light)), Outcome::Answered(None)]);
    // The window is woken after the answer is recorded, so a waiter can see
    // the answer first: each wake is received, not counted at an instant.
    let (woke, wakes) = mpsc::channel::<()>();
    let woke = Mutex::new(woke);
    let wake: Wake = Arc::new(move || {
        let _ = woke.lock().expect("the wake channel").send(());
    });
    let t0 = Instant::now();
    let scheme = SystemScheme::with_runner(runner, Some(wake), t0);
    assert!(!scheme.refresh(t0 + REQUERY_INTERVAL * 2), "one query at a time");
    release.send(()).expect("release");
    assert_eq!(scheme.wait(Duration::from_secs(5)), Known::Answered(Some(Scheme::Light)));
    wakes.recv_timeout(Duration::from_secs(5)).expect("the first answer wakes the window");
    assert!(!scheme.refresh(t0 + REQUERY_INTERVAL - Duration::from_millis(1)), "not within the interval");
    assert!(scheme.refresh(t0 + REQUERY_INTERVAL), "once the interval has passed");
    release.send(()).expect("release");
    assert_eq!(scheme.wait(Duration::from_secs(5)), Known::Answered(None));
    wakes.recv_timeout(Duration::from_secs(5)).expect("the second answer wakes the window");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn start_up_waits_no_longer_than_its_budget() {
    let (runner, release, _) = gated(vec![Outcome::Answered(Some(Scheme::Light))]);
    let scheme = SystemScheme::with_runner(runner, None, Instant::now());
    let started = Instant::now();
    assert_eq!(scheme.wait(STARTUP_WAIT), Known::Unknown(None));
    let took = started.elapsed();
    assert!(took >= STARTUP_WAIT && took < Duration::from_secs(1), "{took:?}");
    release.send(()).expect("release");
    assert_eq!(scheme.wait(Duration::from_secs(5)), Known::Answered(Some(Scheme::Light)));
}

#[test]
fn an_unknown_answer_keeps_the_preference_already_known() {
    let (runner, release, _) = gated(vec![
        Outcome::Answered(Some(Scheme::Light)),
        Outcome::Unknown("no answer within 1000 ms".into()),
    ]);
    let t0 = Instant::now();
    let scheme = SystemScheme::with_runner(runner, None, t0);
    release.send(()).expect("release");
    scheme.wait(Duration::from_secs(5));
    assert!(scheme.refresh(t0 + REQUERY_INTERVAL));
    release.send(()).expect("release");
    scheme.wait(Duration::from_secs(5));
    assert_eq!(scheme.known(), Known::Answered(Some(Scheme::Light)));
}

#[test]
fn the_effective_scheme_is_dark_unless_light_is_preferred() {
    assert_eq!(Known::Unknown(None).effective(), Scheme::Dark);
    assert_eq!(Known::Answered(None).effective(), Scheme::Dark);
    assert_eq!(Known::Answered(Some(Scheme::Light)).effective(), Scheme::Light);
    let reported = SystemScheme::start(&Source::Window, None, Instant::now());
    assert_eq!(reported.known(), Known::Unknown(None));
    reported.set(Some(Scheme::Light));
    assert_eq!(reported.effective(), Scheme::Light);
}

#[test]
fn platform_source_and_preference_changes_resolve_the_displayed_palette() {
    use crate::theme::{self, ThemeChoice, ThemeDirs};

    let reports_through_window = cfg!(any(target_os = "macos", windows));
    let source = Source::choose(None, reports_through_window).expect("each supported platform has a source");
    match source {
        Source::Window => assert!(reports_through_window),
        Source::Portal(ref program) => {
            assert!(!reports_through_window);
            assert_eq!(program, Path::new("dbus-send"));
        }
        Source::Fixed(_) => panic!("no override was requested"),
    }

    let fs = gaze_fs::MemFs::new();
    let dirs = ThemeDirs { user: PathBuf::from("/unused/themes"), packaged: vec![] };
    let system = SystemScheme::fixed(Known::Unknown(None));
    for (preference, expected) in [
        (Some(Scheme::Light), Scheme::Light),
        (Some(Scheme::Dark), Scheme::Dark),
        (None, Scheme::Dark),
        (Some(Scheme::Light), Scheme::Light),
    ] {
        system.set(preference);
        let resolved = theme::resolve(&ThemeChoice::System, preference, &fs, &dirs);
        assert_eq!(system.effective(), expected);
        assert_eq!(resolved.scheme, expected);
        assert_eq!(resolved.colours, theme::builtin_palette(expected));
        assert_eq!(theme::resolve(&ThemeChoice::BuiltIn(Scheme::Dark), preference, &fs, &dirs).scheme, Scheme::Dark);
    }
}
