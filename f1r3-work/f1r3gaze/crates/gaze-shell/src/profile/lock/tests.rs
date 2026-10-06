use super::*;
use crate::test_support::process_starts;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Names the profile the child process of
/// [`a_holder_that_dies_releases_its_lock`] locks.
const CHILD_VAR: &str = "F1R3GAZE_LOCK_TEST_CHILD";

/// What the child prints once it holds the lock. The test harness prints
/// `test <name> ... ` before running a test, without a line break, so the
/// marker is looked for anywhere in a line.
const HELD_MARKER: &str = "F1R3GAZE-LOCK-HELD";

fn layout(name: &str) -> (PathBuf, Layout) {
    let root = gaze_fs::scratch_dir(name);
    let layout = Layout::portable(&root);
    (root, layout)
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn a_second_instance_is_refused_and_told_who_holds_the_profile() {
    let (_, l) = layout("gaze-shell-lock-second");
    let report = Report::silent();
    let first = InstanceLock::acquire(&l, Mode::Window, at(1_791_224_462), &report).expect("first");
    assert_eq!(first.files().collect::<Vec<_>>(), [l.anchor_lock_file(), l.lock_file()]);
    match InstanceLock::acquire(&l, Mode::Headless, at(1_791_224_500), &report) {
        Err(LockError::Held { lock, holder }) => {
            assert_eq!(lock, l.anchor_lock_file(), "the anchor is met first");
            assert_eq!(holder.pid, Some(std::process::id()));
            assert_eq!(holder.mode.as_deref(), Some("window"));
            assert_eq!(holder.started.as_deref(), Some("2026-10-05T18:21:02Z"));
            assert_eq!(holder.data.as_deref(), l.data.to_str());
            let message = LockError::Held { lock, holder }.to_string();
            let named = format!("process {} (window, since 2026-10-05T18:21:02Z)", std::process::id());
            assert!(message.contains(&named), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(report.events().is_empty(), "{:?}", report.events());
    let _starts = process_starts();
    drop(first);
    let again = InstanceLock::acquire(&l, Mode::Headless, at(1), &report).expect("released on drop");
    drop(again);
}

/// Not a test of its own: [`a_holder_that_dies_releases_its_lock`] runs it
/// in a child process, which takes the lock of the profile named by
/// [`CHILD_VAR`], says so, and waits to be killed. Without the variable it
/// does nothing.
#[test]
fn child_process_that_holds_a_lock() {
    let Some(root) = std::env::var_os(CHILD_VAR) else {
        return;
    };
    let l = Layout::portable(Path::new(&root));
    let _lock = InstanceLock::acquire(&l, Mode::Headless, SystemTime::now(), &Report::silent()).expect("the child's lock");
    println!("{HELD_MARKER}");
    io::stdout().flush().expect("flush");
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Kills and reaps the child when the test ends, however it ends: a
/// failing assertion must not leave the child holding its lock forever.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // It may be dead already: then both report an error to ignore.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_holder_that_dies_releases_its_lock() {
    let (root, l) = layout("gaze-shell-lock-child");
    let starts = process_starts();
    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().expect("the test binary"))
            .args([
                "--exact",
                "profile::lock::tests::child_process_that_holds_a_lock",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_VAR, &root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the child"),
    );
    drop(starts);
    let stdout = child.0.stdout.take().expect("the child's stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let held = BufReader::new(stdout).lines().map_while(Result::ok).any(|line| line.contains(HELD_MARKER));
        let _ = tx.send(held);
    });
    let held = rx.recv_timeout(Duration::from_secs(60));
    assert_eq!(held, Ok(true), "the child never took the lock");
    let report = Report::silent();
    match InstanceLock::acquire(&l, Mode::Window, SystemTime::now(), &report) {
        Err(LockError::Held { holder, .. }) => assert_eq!(holder.pid, Some(child.0.id())),
        other => panic!("{other:?}"),
    }
    // SIGKILL on Unix, TerminateProcess on Windows: no destructor runs, as
    // in a crash. The operating system releases the lock.
    child.0.kill().expect("kill the child");
    child.0.wait().expect("reap the child");
    let lock = InstanceLock::acquire(&l, Mode::Window, SystemTime::now(), &report).expect("the dead holder's lock is free");
    drop(lock);
}

#[test]
fn a_lock_freed_while_waiting_is_taken() {
    // A holder that ends within the retry window (a quick restart after a
    // crash): the lock is taken, not reported as held.
    let (_, l) = layout("gaze-shell-lock-patience");
    let report = Report::silent();
    let first = InstanceLock::acquire(&l, Mode::Window, at(0), &report).expect("first");
    let _starts = process_starts();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(first);
    });
    let started = std::time::Instant::now();
    let second = InstanceLock::acquire(&l, Mode::Window, at(1), &report).expect("taken once the holder is gone");
    let waited = started.elapsed();
    releaser.join().expect("the releaser");
    assert!(waited >= Duration::from_millis(150) && waited < HELD_RETRY + Duration::from_millis(500), "{waited:?}");
    drop(second);
}

#[test]
fn a_lock_still_held_after_the_retry_is_reported() {
    let (_, l) = layout("gaze-shell-lock-impatience");
    let report = Report::silent();
    let first = InstanceLock::acquire(&l, Mode::Window, at(0), &report).expect("first");
    let started = std::time::Instant::now();
    let patience = Duration::from_millis(300);
    match InstanceLock::acquire_within(&StdFs, &l, Mode::Headless, at(1), &report, patience) {
        Err(LockError::Held { .. }) => {}
        other => panic!("{other:?}"),
    }
    let waited = started.elapsed();
    assert!(waited >= patience && waited < patience + Duration::from_millis(500), "{waited:?}");
    drop(first);
}

#[test]
fn the_anchor_holds_when_the_runtime_lock_is_removed_or_elsewhere() {
    let (root, l) = layout("gaze-shell-lock-anchor");
    let report = Report::silent();
    let first = InstanceLock::acquire(&l, Mode::Window, at(0), &report).expect("first");
    // A cleaner removes the runtime lock file while it is held.
    std::fs::remove_file(l.lock_file()).expect("remove the runtime lock file");
    // Another process sees another runtime folder (a cron job, su).
    let mut elsewhere = l.clone();
    elsewhere.runtime = root.join("other-runtime");
    for candidate in [&l, &elsewhere] {
        match InstanceLock::acquire(candidate, Mode::Headless, at(1), &report) {
            Err(LockError::Held { lock, .. }) => assert_eq!(lock, l.anchor_lock_file()),
            other => panic!("{other:?}"),
        }
    }
    let _starts = process_starts();
    drop(first);
    let next = InstanceLock::acquire(&l, Mode::Window, at(2), &report).expect("free again");
    assert!(l.lock_file().exists(), "the removed lock file is made again");
    drop(next);
}

#[test]
fn the_runtime_lock_is_released_first() {
    // Dropping releases in reverse: a process taking the anchor right after
    // finds the runtime lock free too.
    let (_, l) = layout("gaze-shell-lock-order");
    let report = Report::silent();
    let lock = InstanceLock::acquire(&l, Mode::Window, at(0), &report).expect("lock");
    let taken: Vec<PathBuf> = lock.files().map(Path::to_path_buf).collect();
    assert_eq!(taken, [l.anchor_lock_file(), l.lock_file()]);
    let _starts = process_starts();
    assert_eq!(lock.release(), [l.lock_file(), l.anchor_lock_file()]);
    let again = InstanceLock::acquire(&l, Mode::Window, at(1), &report).expect("both released");
    drop(again);
}

#[cfg(unix)]
#[test]
fn lock_files_are_private_and_the_runtime_one_sticky() {
    use std::os::unix::fs::PermissionsExt;
    let (_, l) = layout("gaze-shell-lock-modes");
    let lock = InstanceLock::acquire(&l, Mode::Command("wallet new"), at(0), &Report::silent()).expect("lock");
    let mode = |path: &Path| std::fs::metadata(path).expect("stat").permissions().mode() & 0o7777;
    assert_eq!(mode(&l.anchor_lock_file()), 0o600);
    let runtime_mode = if cfg!(target_os = "linux") { 0o1600 } else { 0o600 };
    assert_eq!(mode(&l.lock_file()), runtime_mode);
    assert_eq!(mode(&l.data), 0o700);
    assert_eq!(mode(&l.runtime), 0o700);
    for lock_path in [l.anchor_lock_file(), l.lock_file()] {
        let beside = lock_path.with_file_name(HOLDER_FILE);
        assert_eq!(mode(&beside), 0o600);
        let text = std::fs::read_to_string(&beside).expect("read");
        assert_eq!(Holder::parse(&text).mode.as_deref(), Some("wallet new"));
        assert_eq!(std::fs::read_to_string(&lock_path).expect("read"), text, "the lock file says the same");
    }
    drop(lock);
}

#[cfg(unix)]
#[test]
fn a_profile_that_cannot_be_written_cannot_be_locked_for_writing() {
    use std::os::unix::fs::PermissionsExt;
    let (_, l) = layout("gaze-shell-lock-read-only");
    std::fs::create_dir_all(&l.data).expect("mkdir");
    std::fs::set_permissions(&l.data, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let probe = l.data.join("probe");
    if std::fs::write(&probe, b"").is_ok() {
        // Permissions do not bind this user (root): nothing to test.
        std::fs::remove_file(&probe).expect("remove the probe");
        std::fs::set_permissions(&l.data, std::fs::Permissions::from_mode(0o700)).expect("chmod back");
        return;
    }
    let result = InstanceLock::acquire(&l, Mode::Window, at(0), &Report::silent());
    std::fs::set_permissions(&l.data, std::fs::Permissions::from_mode(0o700)).expect("chmod back");
    match result {
        Err(LockError::ReadOnly { lock, error }) => {
            assert_eq!(lock, l.anchor_lock_file());
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        }
        other => panic!("{other:?}"),
    }
    assert!(!l.lock_file().exists(), "the runtime lock is not taken for a profile that cannot be written");
}

#[test]
fn a_runtime_fallback_is_reported() {
    let (_, mut l) = layout("gaze-shell-lock-fallback");
    l.runtime_is_fallback = true;
    let report = Report::silent();
    let lock = InstanceLock::acquire(&l, Mode::Window, at(0), &report).expect("lock");
    let events = report.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((&events[0].kind, events[0].severity), (&EventKind::RuntimeFallback, Severity::Notice));
    drop(lock);
}

#[test]
fn holder_lines_round_trip() {
    let holder = Holder {
        pid: Some(42),
        mode: Some("wallet new".into()),
        started: Some(utc_time(0)),
        data: Some("/odd\npath %0A 100%".into()),
    };
    assert_eq!(Holder::parse(&holder.render()), holder);
    assert_eq!(Holder::parse("garbage\npid=x\n"), Holder::default());
    assert_eq!(utc_time(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc_time(1_791_224_462), "2026-10-05T18:21:02Z");
    assert_eq!(Holder::default().to_string(), "another process");
    assert_eq!(holder.to_string(), "process 42 (wallet new, since 1970-01-01T00:00:00Z)");
}

#[test]
fn the_lock_creates_its_folders_durably() {
    let (root, l) = layout("gaze-shell-lock-durable");
    let _ = std::fs::remove_dir_all(&root);
    let trace = gaze_fs::TraceFs::new(std::sync::Arc::new(StdFs));
    let synced = |trace: &gaze_fs::TraceFs| -> Vec<PathBuf> {
        trace.log().into_iter().filter(|t| t.op == "sync_dir").map(|t| t.path).collect()
    };
    // The first start makes the profile's root and its data root: each new
    // name is synced in its parent before the lock file is made inside.
    let taken = take_in(&trace, &l.anchor_lock_file());
    assert!(matches!(taken, Take::Taken(_)), "the anchor is taken");
    let parent = root.parent().expect("the scratch root has a parent").to_path_buf();
    assert_eq!(synced(&trace), [parent, root.clone()]);
    drop(taken);
    // Later starts find the folders and sync nothing.
    trace.clear();
    let taken = take_in(&trace, &l.anchor_lock_file());
    assert!(matches!(taken, Take::Taken(_)), "the anchor is taken again");
    assert_eq!(synced(&trace), Vec::<PathBuf>::new());
    drop(taken);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Every operation the lock makes on files goes through the file system it
/// is given, so a traced start records them and a real kill at the k-th
/// operation lines up with the k-th record (ledger S8, part 2).
#[test]
fn the_lock_goes_through_the_file_system_it_is_given() {
    let (root, l) = layout("gaze-shell-lock-traced");
    let _ = std::fs::remove_dir_all(&root);
    let trace = gaze_fs::TraceFs::new(std::sync::Arc::new(StdFs));
    let lock = InstanceLock::acquire_in(&trace, &l, Mode::Window, at(0), &Report::silent()).expect("lock");
    let log = trace.log();
    let named = |op: &str, path: &Path| log.iter().any(|t| t.op == op && t.path == path);
    assert!(named("create_dir_all", &l.data) || named("create_dir", &l.data), "the anchor's folder: {log:#?}");
    for file in lock.files() {
        let beside = file.with_file_name(HOLDER_FILE);
        let published = log
            .iter()
            .any(|t| t.op == "rename" && t.to.as_deref() == Some(beside.as_path()) && t.error.is_none());
        assert!(published, "{} is written through the trace: {log:#?}", beside.display());
    }
    drop(lock);
    // A second instance reads who holds the profile through it too.
    let first = InstanceLock::acquire(&l, Mode::Window, at(1), &Report::silent()).expect("first");
    trace.clear();
    let refused = InstanceLock::acquire_within(&trace, &l, Mode::Headless, at(2), &Report::silent(), Duration::ZERO);
    assert!(matches!(refused, Err(LockError::Held { .. })), "held");
    let anchor_holder = l.anchor_lock_file().with_file_name(HOLDER_FILE);
    assert!(trace.log().iter().any(|t| t.op == "read" && t.path == anchor_holder), "{:#?}", trace.log());
    drop(first);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Who holds the lock is written beside each lock file atomically, so a
/// reader never sees half of it, but not synced: the hold it records ends
/// with any crash, so its syncs bought nothing and cost four fsyncs a start
/// (ledger S15, part 2, V1). On a profile whose folders exist, taking the
/// lock syncs nothing.
#[test]
fn the_holder_record_is_replaced_without_syncing() {
    let (root, l) = layout("gaze-shell-lock-unsynced");
    let _ = std::fs::remove_dir_all(&root);
    drop(InstanceLock::acquire(&l, Mode::Window, at(0), &Report::silent()).expect("the first start makes the folders"));
    let trace = gaze_fs::TraceFs::new(std::sync::Arc::new(StdFs));
    let lock = InstanceLock::acquire_in(&trace, &l, Mode::Headless, at(1), &Report::silent()).expect("lock");
    let log = trace.log();
    let syncs: Vec<_> = log.iter().filter(|t| t.op == "sync_file" || t.op == "sync_dir").collect();
    assert!(syncs.is_empty(), "nothing is synced: {syncs:#?}");
    for file in lock.files() {
        let beside = file.with_file_name(HOLDER_FILE);
        let published = log
            .iter()
            .any(|t| t.op == "rename" && t.to.as_deref() == Some(beside.as_path()) && t.error.is_none());
        assert!(published, "{} is replaced by a rename: {log:#?}", beside.display());
        let written = std::fs::read_to_string(&beside).expect("the holder's record");
        assert!(written.contains("mode=headless"), "{}: {written}", beside.display());
    }
    drop(lock);
    std::fs::remove_dir_all(&root).expect("clean up");
}
