//! Real kills checked against the start-up model (ledger S8, part 2).
//!
//! The binary, built with `crash-points` and `storage-trace`, opens an old
//! single-folder profile laid flat at a portable root, with `wallet list`:
//! the shortest command that runs the whole start-up. With
//! `F1R3GAZE_CRASH_AT=k` it aborts at its k-th storage operation, a real
//! crash on the real file system, with nothing flushed from the process.
//! With `F1R3GAZE_STORAGE_TRACE=<file>` it appends each operation it
//! finished to the file. A clean start then finishes the move. The two
//! processes' records, with the crash between them, must be a behaviour of
//! `docs/storage/tla/ProfileStartup.tla`, checked by TLC as in
//! `storage_trace.rs`.
//!
//! **Alignment.** Every storage operation before the end of start-up (the
//! note `profile opened`, made in `main.rs`) goes through the trace, the
//! instance lock's included (`InstanceLock::acquire_in`). So a process
//! killed at its k-th operation has recorded exactly k − 1 of them. Crash
//! points are chosen before that note: the engine's own files are written
//! with `StdFs`, untraced.
//!
//! `F1R3GAZE_TLA2TOOLS` names the pinned `tla2tools.jar`
//! (`scripts/storage-kill.sh` sets it). `F1R3GAZE_TRACE_QUICK` crashes at
//! every 25th operation instead of every 5th.
#![cfg(all(feature = "crash-points", feature = "storage-trace"))]
// The cases write their fixtures and scratch folders directly.
#![allow(clippy::disallowed_methods)]

mod common;

use gaze_shell::profile::layout::Layout;
use gaze_shell::profile::migrate::Plan;
use gaze_shell::profile::migrate::trace::{Case, Event, events_of_ndjson, plan_of};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// The F1R3Gaze built for these tests, with crash points and tracing.
const BIN: &str = env!("CARGO_BIN_EXE_f1r3gaze");

/// How long one run may take before it is killed: a run that hangs (a
/// dialog of the platform's crash reporter, say) must not hang the test.
const DEADLINE: Duration = Duration::from_secs(60);

/// The note `main.rs` makes when start-up is over.
const OPENED: &str = "profile opened";

/// The files of an old profile: path below the root, and bytes.
type Fixture = Vec<(String, Vec<u8>)>;

/// The shape of the profile this machine's F1R3Gaze made: settings that set
/// no observer (so no event thread dials a node), an id, a workspace.
fn users_profile() -> Fixture {
    vec![
        ("settings.conf".into(), b"# F1R3Gaze settings. Lists are comma-separated.\nobservers =\n".to_vec()),
        ("user-id".into(), b"5f1c2d3e4b5a69788796a5b4c3d2e1f0".to_vec()),
        (
            "workspace.json".into(),
            br#"{"theme":"dark","panel":"appearance","tabs":[{"url":"https://example.org/","title":"Example"}],"visits":[]}"#.to_vec(),
        ),
    ]
}

/// Every kind of item: converted files, moved files, a key, an export, a
/// site store, a replay log, a cache shard.
fn full_profile() -> Fixture {
    let mut files = users_profile();
    files[0].1 = b"observers =\nquorum = 3\n".to_vec();
    files.extend([
        ("palette.css".into(), b":root { --gaze-bg: #101010; }\n".to_vec()),
        ("grants.tsv".into(), Vec::new()),
        (format!("keys/{}.key", "c".repeat(64)), b"not checked by the move".to_vec()),
        ("exports/1111.json".into(), b"{\"an\":\"export\"}".to_vec()),
        (format!("store/{}.gzs", "d".repeat(64)), b"store records".to_vec()),
        ("logs/tab-1-2.gzlog".into(), b"a replay log".to_vec()),
        ("cache/ab/abcdef".into(), b"a blob".to_vec()),
    ]);
    files
}

/// Lays `fixture` out flat at `root`.
fn lay_out(fixture: &Fixture, root: &Path) {
    for (name, bytes) in fixture {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().expect("a fixture file has a folder")).expect("the fixture's folders");
        std::fs::write(&path, bytes).expect("a fixture file");
    }
}

/// One run of `f1r3gaze --profile <root> wallet list` in `dir`, isolated
/// from the user's own folders, recording into `trace`; `crash_at` makes it
/// abort at that storage operation. Its output goes to `dir/run-<n>.log`.
fn run(dir: &Path, root: &Path, trace: &Path, crash_at: Option<usize>, n: usize) -> ExitStatus {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("home/");
    let log = File::create(dir.join(format!("run-{n}.log"))).expect("a run log");
    let mut command = Command::new(BIN);
    command
        .arg("--profile")
        .arg(root)
        .args(["wallet", "list"])
        .env("HOME", &home)
        .env("TMPDIR", &home)
        .env("F1R3GAZE_STORAGE_TRACE", trace)
        .env_remove("F1R3GAZE_PROFILE")
        .stdin(Stdio::null())
        .stdout(log.try_clone().expect("the run log"))
        .stderr(log);
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_DIRS",
        "XDG_DATA_DIRS",
    ] {
        command.env_remove(var);
    }
    match crash_at {
        Some(k) => command.env("F1R3GAZE_CRASH_AT", k.to_string()),
        None => command.env_remove("F1R3GAZE_CRASH_AT"),
    };
    let mut child = command.spawn().expect("f1r3gaze starts");
    let deadline = Instant::now() + DEADLINE;
    loop {
        match child.try_wait().expect("the run can be waited for") {
            Some(status) => return status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{}: a run took longer than {DEADLINE:?}", dir.display());
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Whether `status` is a process that aborted (a crash point).
fn aborted(status: ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(6)
    }
    #[cfg(not(unix))]
    {
        !status.success()
    }
}

/// The trace file's events.
fn events_in(trace: &Path) -> Vec<Event> {
    let text = std::fs::read_to_string(trace).expect("the trace");
    events_of_ndjson(&text).expect("the trace reads back")
}

/// How many operations a run recorded (notes are not operations).
fn operations(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Op(t) if t.op != "note")).count()
}

/// How many operations come before start-up's end.
fn before_opened(events: &[Event]) -> usize {
    let mut ops = 0;
    for event in events {
        match event {
            Event::Op(t) if t.op == "note" && t.what.as_deref() == Some(OPENED) => return ops,
            Event::Op(t) if t.op == "note" => {}
            Event::Op(_) => ops += 1,
            Event::ProcessCrash | Event::PowerCut { .. } => {}
        }
    }
    panic!("the run never noted the end of start-up");
}

/// The number (from 1) of the first operation `pick` chooses.
fn number_of(events: &[Event], pick: impl Fn(&gaze_fs::Traced) -> bool) -> Option<usize> {
    let mut ops = 0;
    for event in events {
        if let Event::Op(t) = event
            && t.op != "note"
        {
            ops += 1;
            if t.error.is_none() && pick(t) {
                return Some(ops);
            }
        }
    }
    None
}

/// Every file under `root`: its bytes, inode and modification time.
fn files(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u64, Option<SystemTime>)> {
    let mut all = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(names) = std::fs::read_dir(&dir) else { continue };
        for entry in names.flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).expect("stat");
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            #[cfg(unix)]
            let inode = std::os::unix::fs::MetadataExt::ino(&meta);
            #[cfg(not(unix))]
            let inode = 0;
            all.insert(path.clone(), (std::fs::read(&path).expect("read"), inode, meta.modified().ok()));
        }
    }
    all
}

/// One killed run, finished by a clean one: what TLC checks.
struct Killed {
    name: String,
    layout: Layout,
    plan: Plan,
    events: Vec<Event>,
}

fn case_of(killed: &Killed) -> Case<'_> {
    Case {
        name: &killed.name,
        layout: &killed.layout,
        plan: &killed.plan,
        far: &[],
        ntfs: cfg!(windows),
        bad_copies: false,
        taken: &[],
        events: &killed.events,
    }
}

/// Kills a start of `fixture` at operation `k`, then finishes it with a
/// clean start, and checks what was left.
fn kill_at(scratch: &Path, shape: &str, fixture: &Fixture, k: usize, rerun: bool) -> Killed {
    let name = format!("{shape}_{k}");
    let dir = scratch.join(&name);
    let _ = std::fs::remove_dir_all(&dir);
    let root = dir.join("profile");
    lay_out(fixture, &root);
    let trace = dir.join("events.ndjson");
    let status = run(&dir, &root, &trace, Some(k), 1);
    assert!(aborted(status), "{name}: the crash point did not abort ({status})");
    let recorded = events_in(&trace);
    assert_eq!(operations(&recorded), k - 1, "{name}: killed at operation {k}, it recorded every one before");
    // The test writes the crash: the dead process could not.
    let mut text = std::fs::read_to_string(&trace).expect("the trace");
    text.push_str("{\"op\":\"crash\",\"kind\":\"process\"}\n");
    std::fs::write(&trace, text).expect("the crash");
    let status = run(&dir, &root, &trace, None, 2);
    assert!(status.success(), "{name}: the next start finished ({status}); see {}", dir.display());
    // Nothing lost: every old file's bytes are somewhere in the profile, at
    // its new place or in a backup.
    let left = files(&root);
    for (file, bytes) in fixture {
        assert!(left.values().any(|(have, _, _)| have == bytes), "{name}: the bytes of {file} were lost");
    }
    let layout = Layout::portable(&root);
    assert_eq!(std::fs::read(layout.user_id_file()).expect("user-id"), fixture[1].1, "{name}: the id moved whole");
    assert!(layout.marker_file().exists(), "{name}: the marker");
    assert!(root.join("MIGRATED.txt").exists(), "{name}: the note");
    assert!(!layout.migration_dir().exists(), "{name}: the plan is gone");
    let events = events_in(&trace);
    let plan = plan_of(&events).unwrap_or_else(|e| panic!("{name}: {e}"));
    if rerun {
        // A third start changes nothing but the lock files.
        let before = files(&root);
        let third = dir.join("third.ndjson");
        assert!(run(&dir, &root, &third, None, 3).success(), "{name}: a third start");
        let after = files(&root);
        let changed: Vec<&PathBuf> = before
            .keys()
            .chain(after.keys())
            .filter(|p| !p.ends_with("instance.lock") && !p.ends_with("instance.pid"))
            .filter(|p| before.get(*p) != after.get(*p))
            .collect();
        assert!(changed.is_empty(), "{name}: a third start changed {changed:?}");
    }
    Killed {
        name,
        layout,
        plan,
        events,
    }
}

/// One kill to make: the shape and its fixture, the operation it dies at,
/// and whether a third start follows.
#[derive(Clone, Copy)]
struct Point {
    shape: &'static str,
    fixture: fn() -> Fixture,
    k: usize,
    rerun: bool,
}

/// Kills a start of each shape at every `step`-th operation of its start-up,
/// and in the windows each guard is for; runs `kill_at` on every core.
fn cases(scratch: &Path, step: usize) -> Vec<Killed> {
    let mut points: Vec<Point> = Vec::with_capacity(256);
    for (shape, fixture) in [("users_profile", users_profile as fn() -> Fixture), ("full", full_profile)] {
        // A clean reference run: how many operations start-up makes, and
        // where the windows are.
        let dir = scratch.join(format!("{shape}_reference"));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("profile");
        lay_out(&fixture(), &root);
        let trace = dir.join("events.ndjson");
        assert!(run(&dir, &root, &trace, None, 1).success(), "{shape}: the reference run");
        let reference = events_in(&trace);
        let total = before_opened(&reference);
        let file_named = |t: &gaze_fs::Traced, prefix: &str| t.path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix));
        let windows = [
            // Just after the marker's temporary file is made: the next start
            // must sweep it.
            number_of(&reference, |t| t.op == "create_new" && file_named(t, ".layout.json.tmp-")),
            // Just after the marker is linked: its temporary file outlives
            // the move (the model's PrepareStep).
            number_of(&reference, |t| t.op == "hard_link" && t.to.as_deref().is_some_and(|to| to.ends_with("layout.json"))),
            // Just after the plan's temporary file is made.
            number_of(&reference, |t| t.op == "create_new" && file_named(t, ".plan.json.tmp-")),
            // Just after the first item is linked at its new place (an old
            // file, directly in the old profile's folder).
            number_of(&reference, |t| t.op == "hard_link" && t.path.parent() == Some(root.as_path())),
        ];
        let mut ks: Vec<usize> = (1..=total).step_by(step).collect();
        for window in windows {
            let at = window.unwrap_or_else(|| panic!("{shape}: a window of the reference run is missing")) + 1;
            assert!(at <= total, "{shape}: the window at {at} is after start-up");
            ks.push(at);
        }
        ks.sort_unstable();
        ks.dedup();
        let sampled = [ks[0], ks[ks.len() / 2], ks[ks.len() - 1]];
        points.extend(ks.into_iter().map(|k| Point {
            shape,
            fixture,
            k,
            rerun: sampled.contains(&k),
        }));
    }
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = points.len().div_ceil(cores).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = points
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|point| kill_at(scratch, point.shape, &(point.fixture)(), point.k, point.rerun))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().expect("a killer")).collect()
    })
}

#[test]
fn real_kills_are_behaviours_of_the_model() {
    let step = match std::env::var_os("F1R3GAZE_TRACE_QUICK") {
        Some(_) => 25,
        None => 5,
    };
    let scratch = gaze_fs::scratch_dir("crash-points");
    let killed = cases(&scratch, step);
    eprintln!("{} real kills, in {}", killed.len(), scratch.display());
    let rejected = common::check_all(&killed, &scratch.join("tlc"), case_of);
    assert!(rejected.is_empty(), "{} of {} traces: {rejected:#?}", rejected.len(), killed.len());
}

/// The binary's crash points abort it, and only when asked: a run without
/// `F1R3GAZE_CRASH_AT` finishes, and every operation of start-up is a
/// traced one (the alignment the cases rely on).
#[test]
fn every_crash_point_is_a_traced_operation() {
    let scratch = gaze_fs::scratch_dir("crash-points-alignment");
    let root = scratch.join("profile");
    lay_out(&users_profile(), &root);
    let trace = scratch.join("events.ndjson");
    assert!(run(&scratch, &root, &trace, None, 1).success(), "a run without a crash point finishes");
    let total = before_opened(&events_in(&trace));
    assert!(total > 10, "start-up makes operations: {total}");
    // A fresh copy, killed at the last operation of start-up: every one
    // before it is in its trace.
    let again = scratch.join("again");
    let root = again.join("profile");
    lay_out(&users_profile(), &root);
    let trace = again.join("events.ndjson");
    assert!(aborted(run(&again, &root, &trace, Some(total), 1)), "killed at {total}");
    assert_eq!(operations(&events_in(&trace)), total - 1);
}
