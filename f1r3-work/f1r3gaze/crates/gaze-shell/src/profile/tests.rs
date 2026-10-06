//! Opening a profile (ledger S14): the lock's policy, start-up's order in
//! [`start_up`], the settings, the state files, and `profile check`.

use super::*;
use crate::profile::layout::{Class, Machine, Platform, platform_layout};
use crate::profile::migrate::convert::LEGACY_TEMPLATE;
use crate::theme::Scheme;
use gaze_fs::{MemFs, TraceFs};
use std::collections::BTreeMap;
use std::time::{Duration, UNIX_EPOCH};

const STAMP: u64 = 1_791_224_462;
/// The id the first start writes.
const ID: &str = "00112233445566778899aabbccddeeff";

fn first_id() -> String {
    ID.to_string()
}

/// An id no test expects: a start that made a new id instead of reading
/// the file would show it.
fn other_id() -> String {
    "ffeeddccbbaa99887766554433221100".to_string()
}

fn env_with(fs: Arc<dyn Fs + Send + Sync>, new_user_id: fn() -> String) -> StartEnv {
    StartEnv {
        fs,
        report: Report::silent(),
        now: UNIX_EPOCH + Duration::from_secs(STAMP),
        new_user_id,
        keystore: KeystoreKind::File,
    }
}

fn env(fs: Arc<dyn Fs + Send + Sync>) -> StartEnv {
    env_with(fs, first_id)
}

fn portable() -> Layout {
    Layout::portable(Path::new("/p"))
}

/// The XDG layout of a user whose home is /home/u: the old profile is
/// /home/u/.local/share/f1r3gaze.
fn xdg() -> Layout {
    let machine = Machine {
        home: Some(PathBuf::from("/home/u")),
        env: BTreeMap::new(),
        roaming_app_data: None,
        local_app_data: None,
    };
    platform_layout(Platform::Xdg, &machine).expect("an XDG layout")
}

fn open(l: &Layout, fs: Arc<dyn Fs + Send + Sync>) -> Profile {
    Profile::open(l.clone(), Locking::Unlocked, env(fs)).expect("opened")
}

fn open_read_only(l: &Layout, fs: Arc<dyn Fs + Send + Sync>) -> Profile {
    Profile::open(l.clone(), Locking::ReadOnly("only looking"), env(fs)).expect("opened")
}

/// The operations of a start that change something, as `(op, path)`.
fn writes_of(trace: &TraceFs) -> Vec<(&'static str, PathBuf)> {
    trace.log().into_iter().filter(|t| t.writes()).map(|t| (t.op, t.path)).collect()
}

/// An old single-folder profile at the XDG layout's legacy place.
fn legacy(l: &Layout) -> Arc<MemFs> {
    let fs = Arc::new(MemFs::new());
    let old = &l.legacy[0];
    fs.seed_file(old.join("settings.conf"), LEGACY_TEMPLATE.as_bytes());
    fs.seed_file(old.join("user-id"), ID.as_bytes());
    fs.seed_file(old.join("palette.css"), b":root { --gaze-bg: #101010; }\n");
    fs
}

#[test]
fn a_steady_open_writes_only_its_busy_flag() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    let first = open(&l, mem.clone());
    assert!(first.may_write());
    assert_eq!(first.migration(), &Migration::Fresh);
    assert_eq!(first.user_id(), ID, "the first start writes a new id");
    drop(first);
    let trace = Arc::new(TraceFs::new(mem.clone()));
    let second = Profile::open(l.clone(), Locking::Unlocked, env_with(trace.clone(), other_id)).expect("opened");
    assert_eq!(second.migration(), &Migration::Current);
    assert_eq!(second.user_id(), ID, "the id is read, never made again");
    assert_eq!(
        writes_of(&trace),
        [("create_new", l.busy_file()), ("remove_file", l.busy_file()), ("sync_dir", l.data.clone())]
    );
    assert_eq!(second.report.count(Severity::Notice), 0, "{:#?}", second.report.events());
}

#[test]
fn a_stopped_migration_leaves_the_session_read_only_and_unfinished() {
    let l = xdg();
    let mem = legacy(&l);
    // A file where the themes folder must go: the move stops at M2.
    mem.seed_file(l.themes_dir(), b"in the way");
    let stopped = open(&l, mem.clone());
    assert!(matches!(stopped.migration(), Migration::Failed { .. }), "{:?}", stopped.migration());
    assert!(stopped.migration_pending());
    assert!(!stopped.may_write());
    let why = stopped.read_only_reason().expect("read only");
    assert!(why.contains("stopped"), "{why}");
    assert!(stopped.blocked.all.is_some(), "every file is blocked");
    assert!(stopped.write_state(&SessionState::default()).is_err());
    assert!(mem.files().contains_key(&l.busy_file()), "the flag stays: the next start runs the barrier");
    drop(stopped);
    // Once the way is clear, the next start finishes the move and its flag.
    mem.remove_file(&l.themes_dir()).expect("rm");
    let resumed = open(&l, mem.clone());
    assert!(matches!(resumed.migration(), Migration::Done(_)), "{:?}", resumed.migration());
    assert!(resumed.may_write());
    assert_eq!(resumed.user_id(), ID, "the old profile's id");
    assert!(!mem.files().contains_key(&l.busy_file()));
}

#[test]
fn a_newer_profile_is_opened_read_only_without_a_busy_flag() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    drop(open(&l, mem.clone()));
    mem.seed_file(l.marker_file(), br#"{"layout":2}"#);
    let trace = Arc::new(TraceFs::new(mem.clone()));
    let newer = open(&l, trace.clone());
    assert_eq!(newer.migration(), &Migration::Newer { layout: 2 });
    assert!(!newer.may_write());
    assert_eq!(writes_of(&trace), Vec::<(&str, PathBuf)>::new(), "no busy flag, nothing written");
    assert!(
        newer
            .report
            .events()
            .iter()
            .any(|e| matches!(e.kind, EventKind::Newer { version: 2, .. }) && e.severity == Severity::Alert),
        "{:#?}",
        newer.report.events()
    );
}

#[test]
fn a_read_only_open_writes_nothing() {
    let l = xdg();
    let damaged = Arc::new(MemFs::new());
    drop(open(&l, damaged.clone()));
    damaged.seed_file(l.session_file(), b"{ not json");
    damaged.seed_file(l.settings_file(), b"[appearance\n");
    for (what, mem) in [("nothing yet", Arc::new(MemFs::new())), ("an old profile", legacy(&l)), ("damaged files", damaged)] {
        let (files, dirs) = (mem.files(), mem.dirs());
        let trace = Arc::new(TraceFs::new(mem.clone()));
        let looked = open_read_only(&l, trace.clone());
        assert!(!looked.may_write(), "{what}");
        assert_eq!(looked.read_only_reason(), Some("only looking"), "{what}");
        assert_eq!(writes_of(&trace), Vec::<(&str, PathBuf)>::new(), "{what}");
        assert_eq!((mem.files(), mem.dirs()), (files, dirs), "{what}");
    }
}

#[test]
fn settings_diagnostics_are_reported_with_their_place() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    drop(open(&l, mem.clone()));
    mem.seed_file(l.settings_file(), b"[browsing]\nhttps_only = \"yes\"\n\n[nonsense]\n");
    let opened = open(&l, mem.clone());
    let settings: Vec<Event> = opened
        .report
        .events()
        .into_iter()
        .filter(|e| e.kind == EventKind::Setting)
        .collect();
    assert_eq!(settings.len(), 2, "{settings:#?}");
    assert!(settings.iter().all(|e| e.path == l.settings_file() && e.severity == Severity::Warning));
    assert!(settings[0].message.contains("settings.toml:2:"), "{}", settings[0].message);
    assert!(settings[0].message.contains("browsing.https_only"), "{}", settings[0].message);
    assert!(settings[1].message.contains("nonsense"), "{}", settings[1].message);
    assert!(!opened.settings.https_only, "the bad value keeps the default");
}

#[test]
fn set_theme_changes_the_session_first() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    let opened = open(&l, mem.clone());
    assert_eq!(opened.theme(), ThemeChoice::System, "the default follows the system");
    opened.set_theme(ThemeChoice::BuiltIn(Scheme::Light)).expect("saved");
    assert_eq!(opened.theme(), ThemeChoice::BuiltIn(Scheme::Light));
    let text = String::from_utf8(mem.files()[&l.settings_file()].clone()).expect("UTF-8");
    assert!(text.contains("theme = \"default-light\""), "{text}");
    assert!(text.starts_with("# F1R3Gaze settings"), "the comments are kept: {text}");
    drop(opened);
    let reopened = open(&l, mem.clone());
    assert_eq!(reopened.theme(), ThemeChoice::BuiltIn(Scheme::Light), "the choice was saved");
    drop(reopened);
    // A session that cannot save it still uses it.
    let before = mem.files()[&l.settings_file()].clone();
    let looked = open_read_only(&l, mem.clone());
    let refused = looked.set_theme(ThemeChoice::BuiltIn(Scheme::Dark)).expect_err("read only");
    assert_eq!(refused, "only looking");
    assert_eq!(looked.theme(), ThemeChoice::BuiltIn(Scheme::Dark), "the session uses it anyway");
    assert_eq!(mem.files()[&l.settings_file()], before, "the file is unchanged");
}

#[test]
fn state_files_round_trip_and_are_refused_when_blocked() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    let opened = open(&l, mem.clone());
    let session = SessionState {
        sidebar_open: true,
        panel: "wallet".into(),
        ..SessionState::default()
    };
    let mut history = History::default();
    history.visit("https://example.org/", "Example");
    let window = WindowState {
        maximized: true,
        ..WindowState::default()
    };
    opened.write_state(&session).expect("session");
    opened.write_state(&history).expect("history");
    opened.write_state(&window).expect("window");
    assert_eq!(opened.read_state::<SessionState>(), session);
    assert_eq!(opened.read_state::<History>(), history);
    assert_eq!(opened.read_state::<WindowState>(), window);
    for path in [l.session_file(), l.history_file(), l.window_file()] {
        assert_eq!(mem.mode(&path).expect("mode"), Some(gaze_fs::PRIVATE_FILE_MODE), "{}", path.display());
    }
    drop(opened);
    // Read only: read, never written.
    let looked = open_read_only(&l, mem.clone());
    assert_eq!(looked.read_state::<SessionState>(), session);
    let refused = looked.write_state(&SessionState::default()).expect_err("read only");
    assert!(refused.contains("only looking"), "{refused}");
    assert_eq!(looked.read_state::<SessionState>(), session, "unchanged");
    drop(looked);
    // A file a newer F1R3Gaze wrote is left alone: its default is used, and
    // nothing is saved over it.
    let newer = br#"{"version":9,"tabs":"a newer shape"}"#;
    mem.seed_file(l.session_file(), newer);
    let opened = open(&l, mem.clone());
    assert_eq!(opened.read_state::<SessionState>(), SessionState::default());
    assert!(opened.write_state(&session).is_err());
    assert_eq!(mem.files()[&l.session_file()], newer);
    assert!(opened.write_state(&history).is_ok(), "the other files are not blocked");
}

#[test]
fn store_salvage_backs_up_into_this_starts_session() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    let opened = open(&l, mem.clone());
    let store = l.site_data_dir().join("0a1b.gzs");
    let salvage = opened.store_salvage("https://a.example");
    gaze_store::Salvage::keep(
        &salvage,
        &gaze_store::Damage {
            path: &store,
            bytes: b"records and a torn tail",
            kept: 11,
            torn_tail: true,
        },
    )
    .expect("kept");
    let copy = l.backups(Class::Data).join("2026-10-05T18-21-02Z/site-data/0a1b.gzs");
    assert_eq!(mem.files().get(&copy).map(Vec::as_slice), Some(&b"records and a torn tail"[..]));
    let salvaged: Vec<Event> = opened
        .report
        .events()
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::Salvaged { .. }))
        .collect();
    assert_eq!(salvaged.len(), 1);
    assert_eq!(salvaged[0].backup.as_deref(), Some(copy.as_path()));
    assert_eq!(salvaged[0].severity, Severity::Notice, "a torn tail is what a crash leaves");
}

#[test]
fn history_backups_are_kept_in_a_read_only_session() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    drop(open(&l, mem.clone()));
    let kept = l.backups(Class::State).join("2026-10-01T00-00-00Z/history.json");
    mem.seed_file(&kept, b"{\"visits\":[]}");
    let looked = open_read_only(&l, mem.clone());
    assert!(looked.forget_history_backups().is_err());
    assert!(mem.files().contains_key(&kept));
    drop(looked);
    let opened = open(&l, mem.clone());
    assert_eq!(opened.forget_history_backups(), Ok(1));
    assert!(!mem.files().contains_key(&kept));
}

#[test]
fn profile_check_reports_what_start_up_would_do() {
    let l = xdg();
    let now = UNIX_EPOCH + Duration::from_secs(STAMP);
    // Nothing yet: the folders, the files and the marker would be made.
    let empty = Arc::new(MemFs::new());
    let trace = Arc::new(TraceFs::new(empty.clone()));
    let checked = check(&l, trace.clone(), now);
    assert_eq!(checked.detected, Ok(Detected::Fresh));
    assert!(checked.plan.is_none());
    assert_eq!(writes_of(&trace), Vec::<(&str, PathBuf)>::new());
    let changes = checked.changes();
    assert!(changes.iter().any(|c| c.contains("marker")), "{changes:#?}");
    assert!(changes.iter().any(|c| c.contains("would be created")), "{changes:#?}");
    // An old profile: it would be moved, by this plan.
    let old = legacy(&l);
    let trace = Arc::new(TraceFs::new(old.clone()));
    let checked = check(&l, trace.clone(), now);
    assert_eq!(checked.detected, Ok(Detected::Legacy { root: l.legacy[0].clone() }));
    let plan = checked.plan.clone().expect("a plan").expect("planned");
    assert!(plan.actions.iter().any(|a| matches!(a, migrate::Action::Move { .. })), "{plan:#?}");
    assert!(checked.changes().iter().any(|c| c.contains("would be moved")), "{:#?}", checked.changes());
    assert_eq!(writes_of(&trace), Vec::<(&str, PathBuf)>::new());
    // After a start, a second start would change nothing.
    drop(open(&l, old.clone()));
    let checked = check(&l, old.clone(), now);
    assert_eq!(checked.detected, Ok(Detected::Current { cleanup: false }));
    assert_eq!(checked.changes(), Vec::<String>::new(), "{:#?}", checked.events);
    // A damaged file would be repaired.
    old.seed_file(l.session_file(), b"{ not json");
    let checked = check(&l, old.clone(), now);
    let changes = checked.changes();
    assert_eq!(changes.len(), 1, "{changes:#?}");
    assert!(changes[0].contains("would be backed up and repaired"), "{}", changes[0]);
}

/// The lock decides what a command may do: a writer is refused while
/// another F1R3Gaze holds the profile, a reader goes on read-only, and
/// `profile check` takes no lock at all.
#[test]
fn the_lock_policy_decides_access() {
    let root = gaze_fs::scratch_dir("gaze-shell-profile-lock-policy");
    let _ = std::fs::remove_dir_all(&root);
    let l = Layout::portable(&root);
    let real = || -> StartEnv { env(Arc::new(gaze_fs::StdFs)) };
    let holder = InstanceLock::acquire(&l, Mode::Window, SystemTime::now(), &Report::silent()).expect("held");
    match Profile::open(l.clone(), Locking::Exclusive(Mode::Headless), real()) {
        Err(OpenError::Held(LockError::Held { holder, .. })) => {
            assert_eq!(holder.pid, Some(std::process::id()));
        }
        other => panic!("a second writer must be refused: {other:?}"),
    }
    let reader = Profile::open(l.clone(), Locking::IfFree(Mode::Command("wallet")), real()).expect("a reader");
    assert!(!reader.may_write());
    assert!(reader.read_only_reason().is_some_and(|why| why.contains("another F1R3Gaze")));
    let notices: Vec<Event> = reader
        .report
        .events()
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::ReadOnly { .. }))
        .collect();
    assert_eq!(notices.len(), 1, "{notices:#?}");
    assert_eq!(notices[0].severity, Severity::Notice);
    drop(reader);
    let looked = Profile::open(l.clone(), Locking::ReadOnly("profile check only looks"), real()).expect("no lock");
    assert_eq!(looked.read_only_reason(), Some("profile check only looks"));
    drop(looked);
    drop(holder);
    let writer = Profile::open(l.clone(), Locking::Exclusive(Mode::Headless), real()).expect("free now");
    assert!(writer.may_write());
    let second = Profile::open(l.clone(), Locking::IfFree(Mode::Command("wallet")), real()).expect("a reader");
    assert!(!second.may_write(), "the writer holds it");
    drop(second);
    drop(writer);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Start-up's order, in one place: a start whose folders cannot be made
/// goes read-only, and keeps its flag for the next start.
#[test]
fn a_profile_whose_folders_cannot_be_made_only_reads() {
    let l = portable();
    let mem = Arc::new(MemFs::new());
    drop(open(&l, mem.clone()));
    mem.remove_dir(&l.content_dir()).expect("rmdir");
    mem.seed_file(l.content_dir(), b"in the way");
    let opened = open(&l, mem.clone());
    assert!(!opened.may_write());
    let why = opened.read_only_reason().expect("read only");
    assert!(why.contains("cannot be made"), "{why}");
    assert!(mem.files().contains_key(&l.busy_file()), "the next start checks everything again");
    assert!(
        opened
            .report
            .events()
            .iter()
            .any(|e| matches!(e.kind, EventKind::ReadOnly { .. }) && e.severity == Severity::Warning)
    );
}
