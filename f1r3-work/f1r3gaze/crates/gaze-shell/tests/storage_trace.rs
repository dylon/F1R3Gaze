//! Real migrations checked against the start-up model (ledger S8).
//!
//! Each case runs start-up (`gaze_shell::profile::start_up`, the order
//! `main.rs` opens a profile in) on a `MemFs` through a `TraceFs`, with the
//! crashes and power cuts it calls for. Every simulated
//! restart is another process (`gaze_fs::with_process_id`). What happened is
//! put into the model's terms (`profile::migrate::trace`), and TLC is asked
//! whether `ProfileStartup.tla` can do the same: it accepts a trace by
//! reporting `TraceEnd` violated (docs/storage/tla/ProfileStartupTrace.tla).
//!
//! `F1R3GAZE_TLA2TOOLS` names the pinned `tla2tools.jar`; the test fails
//! without it (`scripts/storage-trace.sh` sets it).
//! `F1R3GAZE_TRACE_QUICK` crashes at every 25th operation instead of every
//! 5th.
#![cfg(feature = "storage-trace")]
// The cases write their fixtures and scratch folders directly.
#![allow(clippy::disallowed_methods)]

use gaze_fs::{Fs, MemFs, TraceFs, with_process_id};
use gaze_shell::profile::layout::{Layout, Machine, Platform, platform_layout};
use gaze_shell::profile::migrate::trace::{Case, Event};
use gaze_shell::profile::migrate::{Migration, Plan, plan};
use gaze_shell::profile::report::Report;
use gaze_shell::profile::{Access, KeystoreKind, StartUp, Started};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod common;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// When the starts run: 2026-10-05 18:21:02 UTC.
const STAMP: u64 = 1_791_224_462;
/// The old profile of the layout below.
const OLD: &str = "/home/u/.local/share/f1r3gaze";

/// The XDG layout of a user whose home is /home/u.
fn xdg() -> Layout {
    let machine = Machine {
        home: Some(PathBuf::from("/home/u")),
        env: BTreeMap::new(),
        roaming_app_data: None,
        local_app_data: None,
    };
    platform_layout(Platform::Xdg, &machine).expect("an XDG layout")
}

fn old(name: &str) -> PathBuf {
    Path::new(OLD).join(name)
}

/// One start, as `main.rs` opens a profile (the lock aside): the profile's
/// own `start_up`, so the cases check the order the browser runs.
///
/// Was a copy of that order here (begin, the migration, prepare, reconcile,
/// finish), which could drift from `main.rs`'s; `start_up` became the one
/// copy at the switch-over (ledger S14).
fn start_up(l: &Layout, fs: Arc<dyn Fs + Send + Sync>) -> Started {
    let report = Report::silent();
    let new_id = || "0123456789abcdef0123456789abcdef".to_string();
    gaze_shell::profile::start_up(StartUp {
        layout: l,
        fs,
        report: &report,
        access: Access::Write,
        keystore: KeystoreKind::File,
        now: UNIX_EPOCH + Duration::from_secs(STAMP),
        new_user_id: &new_id,
    })
}

/// Whether a start left the profile moved and writable: what every start
/// after a crash must do.
fn finished(started: &Started) -> bool {
    started.access == Access::Write && matches!(started.migration, Migration::Done(_) | Migration::Current)
}

/// A case as it runs: its file system, what happened, and the next
/// simulated process's id.
struct Running {
    layout: Layout,
    mem: Arc<MemFs>,
    plan: Plan,
    events: Vec<Event>,
    pid: u32,
}

impl Running {
    fn new(mem: MemFs) -> Running {
        let layout = xdg();
        let plan = plan(&layout, &mem, Path::new(OLD), UNIX_EPOCH + Duration::from_secs(STAMP)).expect("a plan");
        Running {
            layout,
            mem: Arc::new(mem),
            plan,
            events: Vec::with_capacity(1024),
            pid: 70_001,
        }
    }

    /// One start, as a new process, dying after `crash` operations if given.
    fn start(&mut self, crash: Option<usize>) -> Started {
        let trace = Arc::new(TraceFs::new(self.mem.clone()));
        if let Some(k) = crash {
            self.mem.fail_after(k);
        }
        let layout = self.layout.clone();
        let started = with_process_id(self.pid, || start_up(&layout, trace.clone()));
        self.pid += 1;
        self.events.extend(trace.log().into_iter().map(Event::Op));
        if crash.is_some() {
            self.mem.revive();
        }
        started
    }

    /// The process is gone: killed, or done after a failed move.
    fn process_crash(&mut self) {
        self.events.push(Event::ProcessCrash);
    }

    /// The power is cut, keeping the pending operations `keep` chooses.
    fn power_cut(&mut self, keep: impl Fn(usize) -> bool) {
        let pending = self.mem.pending_changes();
        let kept: Vec<bool> = (0..pending.len()).map(keep).collect();
        self.mem.power_cut(|i| kept[i]);
        self.events.push(Event::PowerCut { pending, keep: kept });
    }

    /// How many file-system operations a whole start on a copy of this one
    /// makes up to and including the first that `stop` picks: a crash point
    /// just after it.
    fn ops_through(&self, stop: impl Fn(&gaze_fs::Traced) -> bool) -> usize {
        let copy = Arc::new(MemFs::clone(&self.mem));
        let trace = Arc::new(TraceFs::new(copy));
        let layout = self.layout.clone();
        with_process_id(self.pid, || start_up(&layout, trace.clone()));
        let log = trace.log();
        let at = log.iter().position(stop).expect("the operation happens");
        // Notes are not file-system operations: fail_after does not count them.
        log[..=at].iter().filter(|t| t.op != "note").count()
    }

    /// How many operations one whole start makes on a copy of this one.
    fn ops(&self) -> usize {
        let copy = Arc::new(MemFs::clone(&self.mem));
        copy.reset_ops();
        let layout = self.layout.clone();
        with_process_id(self.pid, || start_up(&layout, copy.clone()));
        copy.ops()
    }
}

/// The profile this machine's F1R3Gaze made: a template that sets nothing,
/// an id, and a dark workspace.
fn users_profile() -> MemFs {
    let fs = MemFs::new();
    fs.seed_file(old("settings.conf"), b"# F1R3Gaze settings. Lists are comma-separated.\n# quorum = 2\n");
    fs.seed_file(old("user-id"), b"5f1c2d3e4b5a69788796a5b4c3d2e1f0");
    fs.seed_file(
        old("workspace.json"),
        br#"{"theme":"dark","panel":"appearance","tabs":[{"url":"https://example.org/","title":"Example"}],"visits":[]}"#,
    );
    fs
}

/// Every kind of item: converted files, moved files, a key, a cache shard.
fn full_profile() -> MemFs {
    let fs = users_profile();
    fs.seed_file(old("settings.conf"), b"quorum = 3\n");
    fs.seed_file(old("palette.css"), b":root { --gaze-bg: #101010; }\n");
    fs.seed_file(old("grants.tsv"), b"");
    fs.seed_file(old(&format!("keys/{}.key", "c".repeat(64))), b"not checked by the move");
    fs.seed_file(old("cache/ab/abcdef"), b"blob");
    fs
}

/// A case, ready to check.
struct Checked {
    name: String,
    layout: Layout,
    plan: Plan,
    far: Vec<PathBuf>,
    ntfs: bool,
    bad_copies: bool,
    taken: Vec<PathBuf>,
    events: Vec<Event>,
}

impl Checked {
    fn of(name: &str, run: Running, far: Vec<PathBuf>, ntfs: bool, bad_copies: bool, taken: Vec<PathBuf>) -> Checked {
        Checked {
            name: name.into(),
            layout: run.layout,
            plan: run.plan,
            far,
            ntfs,
            bad_copies,
            taken,
            events: run.events,
        }
    }
}

/// The old profile's folders, on the other volume in the cross-device cases.
fn far_folders() -> Vec<PathBuf> {
    vec![PathBuf::from(OLD), old("keys")]
}

/// Every case of the design (B.14), the crash points spaced by `step`.
fn cases(step: usize) -> Vec<Checked> {
    let mut all = Vec::with_capacity(128);
    // The shape of this machine's profile, and a full one, on one volume.
    for (name, shape) in [("users_profile", users_profile as fn() -> MemFs), ("full", full_profile)] {
        let mut run = Running::new(shape());
        assert!(matches!(run.start(None).migration, Migration::Done(_)), "{name}");
        all.push(Checked::of(name, run, Vec::new(), false, false, Vec::new()));
    }
    // Across volumes, and under NTFS semantics.
    let mut run = Running::new(full_profile().with_device(OLD));
    assert!(matches!(run.start(None).migration, Migration::Done(_)), "cross_device");
    all.push(Checked::of("cross_device", run, far_folders(), false, false, Vec::new()));
    let mut run = Running::new(full_profile().ntfs());
    assert!(matches!(run.start(None).migration, Migration::Done(_)), "ntfs");
    all.push(Checked::of("ntfs", run, Vec::new(), true, false, Vec::new()));
    // A destination already taken: a conflict, both kept.
    let fs = users_profile();
    let l = xdg();
    fs.seed_file(l.user_id_file(), b"ffffffffffffffffffffffffffffffff");
    let taken = vec![l.user_id_file()];
    let mut run = Running::new(fs);
    assert!(matches!(run.start(None).migration, Migration::Done(_)), "conflict");
    all.push(Checked::of("conflict", run, Vec::new(), false, false, taken));
    // A copy that comes out wrong: the move stops; the next start, on a
    // good disk, finishes it.
    let mut run = Running::new(full_profile().with_device(OLD).with_faulty_copies());
    assert!(matches!(run.start(None).migration, Migration::Failed { .. }), "bad_copy");
    run.process_crash();
    run.mem.set_faulty_copies(false);
    assert!(matches!(run.start(None).migration, Migration::Done(_)), "bad_copy, resumed");
    all.push(Checked::of("bad_copy", run, far_folders(), false, true, Vec::new()));
    // A process killed at every step-th operation, then a start that
    // finishes, on one volume and across two.
    for (shape, far) in [("one", false), ("two", true)] {
        let fresh = || match far {
            true => full_profile().with_device(OLD),
            false => full_profile(),
        };
        let total = Running::new(fresh()).ops();
        for k in (0..total).step_by(step) {
            let mut run = Running::new(fresh());
            run.start(Some(k));
            run.process_crash();
            let after = run.start(None);
            assert!(finished(&after), "{shape} volume, crash at {k}: {after:?}");
            let folders = if far { far_folders() } else { Vec::new() };
            all.push(Checked::of(&format!("killed_{shape}_{k}"), run, folders, false, false, Vec::new()));
        }
    }
    // Crashes in the windows each guard is for, named so they never depend
    // on the spacing above: just after the marker's temporary file is made
    // (the next start must sweep it), just after the marker is linked (its
    // temporary file outlives the migration: the model's PrepareStep), and
    // just after a copy to another volume, before its comparison.
    // (name, on two volumes, the operation to crash just after)
    type Window = (&'static str, bool, fn(&gaze_fs::Traced) -> bool);
    let named: [Window; 3] = [
        ("killed_after_marker_temp", false, |t| {
            t.op == "create_new" && t.path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".layout.json.tmp-"))
        }),
        ("killed_after_marker_link", false, |t| {
            t.op == "hard_link" && t.to.as_deref().is_some_and(|to| to.ends_with("layout.json"))
        }),
        ("killed_after_copy", true, |t| t.op == "copy_new" && t.error.is_none()),
    ];
    for (name, far, stop) in named {
        let fresh = || match far {
            true => full_profile().with_device(OLD),
            false => full_profile(),
        };
        let k = Running::new(fresh()).ops_through(stop);
        let mut run = Running::new(fresh());
        run.start(Some(k));
        run.process_crash();
        let after = run.start(None);
        assert!(finished(&after), "{name}: {after:?}");
        let folders = if far { far_folders() } else { Vec::new() };
        all.push(Checked::of(name, run, folders, false, false, Vec::new()));
    }
    // A power cut in the middle, keeping none, all, or every other pending
    // operation, then a start that finishes.
    let total = Running::new(full_profile()).ops();
    for (keeping, keep) in [
        ("none", (|_| false) as fn(usize) -> bool),
        ("all", |_| true),
        ("alternate", |i| i % 2 == 0),
    ] {
        for k in [total / 4, total / 2, 3 * total / 4] {
            let mut run = Running::new(full_profile());
            run.start(Some(k));
            run.power_cut(keep);
            let after = run.start(None);
            assert!(finished(&after), "power cut at {k}, keeping {keeping}: {after:?}");
            all.push(Checked::of(&format!("power_{keeping}_{k}"), run, Vec::new(), false, false, Vec::new()));
        }
    }
    all
}

// The TLC helpers (`jar`, `accepted`, `diagnose`, `tlc`, `module_log`,
// `matched_prefix`, `check_all`) moved to `common/mod.rs`, which
// `crash_points.rs` shares (ledger S8, part 2).

/// A case, as `trace::tla` takes it.
fn case_of(case: &Checked) -> Case<'_> {
    Case {
        name: &case.name,
        layout: &case.layout,
        plan: &case.plan,
        far: &case.far,
        ntfs: case.ntfs,
        bad_copies: case.bad_copies,
        taken: &case.taken,
        events: &case.events,
    }
}

#[test]
fn real_migrations_are_behaviours_of_the_model() {
    let step = match std::env::var_os("F1R3GAZE_TRACE_QUICK") {
        Some(_) => 25,
        None => 5,
    };
    let all = cases(step);
    let dir = gaze_fs::scratch_dir("storage-trace");
    eprintln!("{} traces, in {}", all.len(), dir.display());
    let rejected = common::check_all(&all, &dir, case_of);
    assert!(rejected.is_empty(), "{} of {} traces: {rejected:#?}", rejected.len(), all.len());
}

/// The index, in `events`, of the first item move's link, its next folder
/// sync, and its unlink: `link`, then `fsync-dir`, then `unlink`.
fn first_move(events: &[Event]) -> (usize, usize, usize) {
    let op = |e: &Event| match e {
        Event::Op(t) if t.error.is_none() => Some((t.op, t.path.clone(), t.to.clone())),
        _ => None,
    };
    for (i, e) in events.iter().enumerate() {
        if let Some(("hard_link", from, Some(_))) = op(e)
            && from.starts_with(OLD)
        {
            let sync = (i + 1..events.len())
                .find(|&j| matches!(op(&events[j]), Some(("sync_dir", _, _))))
                .expect("a sync after the link");
            let unlink = (sync + 1..events.len())
                .find(|&j| matches!(op(&events[j]), Some(("remove_file", ref p, _)) if *p == from))
                .expect("the old name's unlink");
            return (i, sync, unlink);
        }
    }
    panic!("no move in the trace");
}

#[test]
fn the_model_rejects_what_the_protocol_forbids() {
    let mut run = Running::new(users_profile());
    assert!(matches!(run.start(None).migration, Migration::Done(_)));
    let good = Checked::of("good", run, Vec::new(), false, false, Vec::new());
    let (link, sync, unlink) = first_move(&good.events);
    // The new name's folder never synced before the old name goes.
    let mut unsynced = good.events.clone();
    unsynced.remove(sync);
    // The old name removed before the new one exists.
    let mut swapped = good.events.clone();
    swapped.swap(link, unlink);
    let dir = gaze_fs::scratch_dir("storage-trace-mutants");
    let mutants = [
        Checked { name: "mutant_unsynced".into(), events: unsynced, ..clone_case(&good) },
        Checked { name: "mutant_swapped".into(), events: swapped, ..clone_case(&good) },
    ];
    assert_eq!(common::accepted(&case_of(&good), &dir), Ok(true), "the trace the mutants come from is accepted");
    for mutant in &mutants {
        assert_eq!(common::accepted(&case_of(mutant), &dir), Ok(false), "{} must be rejected", mutant.name);
    }
}

/// A copy of a case's description, to change one field of.
fn clone_case(case: &Checked) -> Checked {
    Checked {
        name: case.name.clone(),
        layout: case.layout.clone(),
        plan: case.plan.clone(),
        far: case.far.clone(),
        ntfs: case.ntfs,
        bad_copies: case.bad_copies,
        taken: case.taken.clone(),
        events: case.events.clone(),
    }
}
