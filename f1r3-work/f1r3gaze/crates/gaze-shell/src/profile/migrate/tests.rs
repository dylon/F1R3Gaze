//! Tests of the migration's marker, plan, busy flag and barrier (ledger S7).

use super::*;
use gaze_fs::MemFs;

fn layout() -> Layout {
    Layout::portable(Path::new("/p"))
}

/// A steady profile's folders and marker, all durable.
fn steady() -> MemFs {
    let fs = MemFs::new();
    for dir in layout().skeleton() {
        fs.seed_dir(dir);
    }
    fs.seed_file(layout().marker_file(), &Marker::repaired().to_bytes());
    fs
}

#[test]
fn markers_read_back_and_newer_ones_are_recognised() {
    let marker = Marker {
        layout: 1,
        created: Some("2026-10-05T18:21:02Z".into()),
        migrated_from: Some("/home/u/.local/share/f1r3gaze".into()),
        repaired: false,
    };
    assert_eq!(Marker::read(&marker.to_bytes()), Ok(marker));
    assert_eq!(Marker::repaired().to_bytes(), b"{\"layout\":1,\"repaired\":true}\n");
    assert_eq!(Marker::read(br#"{"layout":1}"#).map(|m| m.layout), Ok(1));
    assert_eq!(
        Marker::read(br#"{"layout":2,"created":7}"#),
        Err(StateError::Newer { version: 2, known: 1 }),
        "a newer layout is recognised even when other fields changed shape"
    );
    for bad in [&b"[]"[..], b"{}", b"{\"layout\":0}", b"{\"layout\":\"1\"}", b"{\"layout\":1,\"created\":7}", b"x"] {
        assert!(matches!(Marker::read(bad), Err(StateError::Corrupt(_))), "{}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn only_a_start_after_an_unfinished_one_runs_the_barrier() {
    let l = layout();
    let fs = steady();
    // A start after one that finished: no barrier, and the flag is set
    // without a sync.
    fs.reset_ops();
    assert_eq!(begin(&l, &fs).expect("begin"), Begun::Clean);
    assert!(fs.files().contains_key(&l.busy_file()));
    assert_eq!(fs.pending_len(), 1, "the flag is not synced");
    // It finishes: the flag goes, and nothing is left pending.
    finish(&l, &fs).expect("finish");
    assert!(!fs.files().contains_key(&l.busy_file()));
    assert_eq!(fs.pending_len(), 0);
    // A start that dies leaves its flag and what it had pending.
    begin(&l, &fs).expect("begin");
    fs.create_new(&l.state.join("left-pending"), b"x", PRIVATE_FILE_MODE).expect("a write");
    assert_eq!(fs.pending_len(), 2);
    // The next start finds the flag and syncs every folder that exists.
    let existing = barrier_folders(&l, &fs).into_iter().filter(|dir| fs.dirs().contains(dir)).count();
    assert_eq!(existing, 16, "the skeleton, /p and /");
    assert_eq!(begin(&l, &fs).expect("begin after a crash"), Begun::AfterUnfinished { synced: existing });
    assert_eq!(fs.pending_len(), 0);
    assert!(fs.files().contains_key(&l.busy_file()), "the flag stays until this start is done");
    finish(&l, &fs).expect("finish");
    assert!(!fs.files().contains_key(&l.busy_file()));
}

#[test]
fn the_first_start_makes_the_data_root_durably() {
    let l = layout();
    let fs = MemFs::new();
    // No marker yet: the barrier runs, over /p/data, /p and /.
    assert_eq!(begin(&l, &fs).expect("begin"), Begun::AfterUnfinished { synced: 3 });
    // Everything but the unsynced flag survives a power cut.
    assert_eq!(fs.pending_len(), 1);
    fs.power_cut(|_| false);
    assert!(fs.dirs().contains(&l.data));
    assert!(!fs.files().contains_key(&l.busy_file()));
}

#[test]
fn a_start_that_died_making_the_data_root_is_followed_by_the_barrier() {
    let l = layout();
    let fs = MemFs::new();
    // A start made /p/data and died before it synced /: no flag, no marker.
    fs.create_dir_all(&l.data).expect("the folders");
    assert_eq!(fs.pending(), ["+/p (sync /)", "+/p/data (sync /p)"]);
    // The next start cannot tell from the folders; the missing marker says
    // it must sync them.
    assert!(matches!(begin(&l, &fs).expect("begin"), Begun::AfterUnfinished { .. }));
    assert_eq!(fs.pending(), ["+/p/data/.startup-busy (sync /p/data)"]);
}

#[test]
fn the_barrier_covers_backups_link_targets_and_the_roots_ancestors() {
    let l = layout();
    let fs = steady();
    fs.seed_file(l.backups(Class::Config).join("2026-10-05T18-21-02Z/themes/default-dark.css.example"), b"old");
    fs.seed_file(l.backups(Class::State).join("2026-10-05T18-21-02Z.1/session.json"), b"old");
    fs.seed_file("/dotfiles/f1r3gaze/settings.toml", b"");
    fs.seed_symlink(l.settings_file(), "/dotfiles/f1r3gaze/settings.toml");
    let folders = barrier_folders(&l, &fs);
    for dir in [
        "/",
        "/p",
        "/p/config/backups",
        "/p/config/backups/2026-10-05T18-21-02Z",
        "/p/config/backups/2026-10-05T18-21-02Z/themes",
        "/p/state/backups",
        "/p/state/backups/2026-10-05T18-21-02Z.1",
        "/dotfiles/f1r3gaze",
        "/p/runtime",
    ] {
        assert!(folders.contains(Path::new(dir)), "{dir} in {folders:?}");
    }
    // The barrier leaves nothing pending, wherever it was.
    fs.create_new(Path::new("/p/config/backups/2026-10-05T18-21-02Z/themes/copy"), b"x", PRIVATE_FILE_MODE)
        .expect("a copy");
    fs.create_new(Path::new("/dotfiles/f1r3gaze/.settings.toml.tmp-1-0"), b"x", PRIVATE_FILE_MODE)
        .expect("a temporary file");
    barrier(&l, &fs).expect("the barrier");
    assert_eq!(fs.pending(), Vec::<String>::new());
}

#[test]
fn the_barrier_covers_the_folders_of_a_plan() {
    let l = layout();
    let fs = steady();
    let session = "2026-10-05T18-21-02Z";
    let plan = Plan {
        format: PLAN_FORMAT,
        legacy: "/old".into(),
        roots: BTreeMap::new(),
        session: session.into(),
        planned: "2026-10-05T18:21:02Z".into(),
        theme: ThemeNote::default(),
        settings: Vec::new(),
        notes: Vec::new(),
        actions: vec![
            Action::Dir { path: format!("/p/config/backups/{session}/legacy-profile") },
            Action::Write {
                item: "settings.conf".into(),
                from: vec!["/old/settings.conf".into()],
                dst: "/p/config/settings.toml".into(),
                text: String::new(),
            },
            Action::Move { item: "user-id".into(), role: Role::Item, src: "/old/user-id".into(), dst: "/p/data/user-id".into() },
            Action::Move {
                item: "settings.conf".into(),
                role: Role::Original,
                src: "/old/settings.conf".into(),
                dst: format!("/p/config/backups/{session}/legacy-profile/settings.conf"),
            },
            Action::MoveCacheShard { src: "/old/cache/ab".into(), dst: "/p/cache/content/ab".into() },
            Action::Leave { path: "/old/notes/today.txt".into(), why: "not part of a F1R3Gaze profile".into() },
        ],
    };
    let without_plan = barrier_folders(&l, &fs);
    fs.seed_file(l.migration_dir().join(PLAN_FILE), &serde_json::to_vec(&plan).expect("a plan serializes"));
    let with_plan = barrier_folders(&l, &fs);
    let added: Vec<PathBuf> = with_plan.difference(&without_plan).cloned().collect();
    let expected: Vec<PathBuf> = [
        "/old",
        "/old/cache",
        &format!("/p/config/backups/{session}"),
        &format!("/p/config/backups/{session}/legacy-profile"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(added, expected, "the skeleton already holds /p/config, /p/data and /p/cache/content");
    assert!(with_plan.contains(&l.migration_dir()));
    // A plan that cannot be read adds nothing (the migration then fails).
    fs.seed_file(l.migration_dir().join(PLAN_FILE), b"{\"format\":");
    assert_eq!(barrier_folders(&l, &fs), without_plan);
    // Folders that do not exist are skipped.
    fs.seed_file(l.migration_dir().join(PLAN_FILE), &serde_json::to_vec(&plan).expect("a plan serializes"));
    let existing = with_plan.iter().filter(|dir| fs.dirs().contains(dir)).count();
    assert_eq!(barrier(&l, &fs).expect("the barrier"), existing);
}

#[test]
fn plans_read_back_with_their_kinds_named() {
    let action = Action::Move { item: "user-id".into(), role: Role::Original, src: "/a".into(), dst: "/b".into() };
    let json = serde_json::to_string(&action).expect("serializes");
    assert_eq!(json, r#"{"kind":"move","item":"user-id","role":"original","src":"/a","dst":"/b"}"#);
    let shard = Action::MoveCacheShard { src: "/c".into(), dst: "/d".into() };
    assert_eq!(serde_json::to_string(&shard).expect("serializes"), r#"{"kind":"move-cache-shard","src":"/c","dst":"/d"}"#);
    assert_eq!(serde_json::from_str::<Action>(&json).expect("reads back"), action);
}

// ── The migration ────────────────────────────────────────────────────────

use super::convert::old_templates::TEMPLATE_F6EE26A;
use crate::profile::backup::Backups;
use crate::profile::layout::{Machine, Platform, platform_layout};
use crate::profile::reconcile::{Reconciled, Start, prepare, reconcile};
use crate::profile::settings;
use crate::profile::{Access, KeystoreKind};
use crate::ui_state::{History, SavedTab, SessionState, UiState, Visit};
use gaze_fs::{GARBAGE, TraceFs, every_crash, every_two_crashes};
use std::sync::Arc;
use std::time::Duration;

/// When the tests' starts run: 2026-10-05 18:21:02 UTC.
const STAMP: u64 = 1_791_224_462;
/// The backups folder a start at [`STAMP`] claims.
const SESSION: &str = "2026-10-05T18-21-02Z";
/// The id a start makes when it needs one.
const NEW_ID: &str = "0123456789abcdef0123456789abcdef";
/// The old profile of the XDG layout below.
const OLD: &str = "/home/u/.local/share/f1r3gaze";
/// An old user id, as the old F1R3Gaze wrote it: no line break.
const USER_ID: &[u8] = b"5f1c2d3e4b5a69788796a5b4c3d2e1f0";

/// The XDG layout of a user whose home is /home/u, with no XDG variables
/// set: the old profile is /home/u/.local/share/f1r3gaze.
fn xdg() -> Layout {
    let machine = Machine {
        home: Some(PathBuf::from("/home/u")),
        env: BTreeMap::new(),
        roaming_app_data: None,
        local_app_data: None,
    };
    let l = platform_layout(Platform::Xdg, &machine).expect("an XDG layout");
    assert_eq!(l.legacy, [PathBuf::from(OLD)]);
    l
}

/// A file of the old profile.
fn old(name: &str) -> PathBuf {
    Path::new(OLD).join(name)
}

/// What one start did.
struct Started {
    migration: Migration,
    reconciled: Reconciled,
    events: Vec<Event>,
}

/// One start in main.rs's order, at `now`: the lock aside, begin, step 4
/// (this migration), prepare, reconcile and finish. A move that is pending,
/// newer or failed leaves the session read only.
fn start_at(l: &Layout, fs: Arc<dyn Fs + Send + Sync>, access: Access, seconds: u64) -> Result<Started, String> {
    let report = Report::silent();
    let now = UNIX_EPOCH + Duration::from_secs(seconds);
    let backups = Backups::new(Arc::new(l.clone()), fs.clone(), now);
    let new_id = || NEW_ID.to_string();
    let writes = access == Access::Write;
    if writes {
        begin(l, fs.as_ref()).map_err(|e| format!("begin: {e}"))?;
    }
    let migration = {
        let first = Start {
            layout: l,
            fs: fs.as_ref(),
            backups: &backups,
            report: &report,
            access: &access,
            keystore: KeystoreKind::File,
            new_user_id: &new_id,
        };
        run(&first, now)
    };
    let stopped = matches!(migration, Migration::Pending { .. } | Migration::Newer { .. } | Migration::Failed { .. });
    let after = match (stopped, writes) {
        (true, true) => Access::ReadOnly {
            why: "the move of the old profile did not finish".into(),
        },
        _ => access.clone(),
    };
    let second = Start {
        layout: l,
        fs: fs.as_ref(),
        backups: &backups,
        report: &report,
        access: &after,
        keystore: KeystoreKind::File,
        new_user_id: &new_id,
    };
    prepare(&second)?;
    let reconciled = reconcile(&second);
    if after == Access::Write {
        finish(l, fs.as_ref()).map_err(|e| format!("finish: {e}"))?;
    }
    Ok(Started {
        migration,
        reconciled,
        events: report.events(),
    })
}

/// A start at [`STAMP`] with write access, which must finish.
fn start(l: &Layout, fs: &Arc<MemFs>) -> Started {
    start_at(l, fs.clone(), Access::Write, STAMP).expect("the start finishes")
}

/// The summary of a start that moved a profile.
fn done(started: &Started) -> &Summary {
    match &started.migration {
        Migration::Done(summary) => summary,
        other => panic!("the profile was not moved: {other:?}; {:?}", started.events),
    }
}

/// An old `workspace.json`: one tab, four visits, at example.org.
fn workspace(theme: &str) -> Vec<u8> {
    let state = UiState {
        theme: theme.into(),
        sidebar_open: false,
        panel: "appearance".into(),
        tree_tabs: false,
        tabs: vec![SavedTab {
            url: "https://example.org/".into(),
            title: "Example".into(),
            parent: None,
        }],
        active: 0,
        visits: (0..4u64)
            .map(|i| Visit {
                url: format!("https://example.org/{i}"),
                title: format!("Page {i}"),
                at: 1_791_224_000 + i,
            })
            .collect(),
    };
    serde_json::to_vec(&state).expect("a workspace serializes")
}

/// The profile this machine's F1R3Gaze made: the template of `f6ee26a`, an
/// id, and a workspace with the dark theme, one tab and four visits; files
/// 0644 in a folder 0755.
fn users_profile() -> MemFs {
    let fs = MemFs::new();
    fs.seed_file(old("settings.conf"), TEMPLATE_F6EE26A.as_bytes());
    fs.seed_file(old("user-id"), USER_ID);
    fs.seed_file(old("workspace.json"), &workspace("dark"));
    for name in ["settings.conf", "user-id", "workspace.json"] {
        fs.set_mode(&old(name), 0o644).expect("chmod");
    }
    fs.set_mode(Path::new(OLD), 0o755).expect("chmod");
    fs
}

/// The operations of a start that change something, as `(op, path)`.
fn writes_of(trace: &TraceFs) -> Vec<(&'static str, PathBuf)> {
    trace.log().into_iter().filter(|t| t.writes()).map(|t| (t.op, t.path)).collect()
}

#[test]
fn the_users_real_profile_shape() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    let started = start(&l, &fs);
    let summary = done(&started).clone();
    assert_eq!((summary.moved, summary.converted, summary.conflicts.len()), (1, 3, 0));
    let files = fs.files();
    assert_eq!(files[&l.settings_file()], settings::TEMPLATE.as_bytes(), "the old template sets nothing");
    assert_eq!(files[&l.user_id_file()], USER_ID, "the id is kept byte for byte");
    let session = SessionState::read(&files[&l.session_file()]).expect("a session");
    assert_eq!((session.tabs.len(), session.panel.as_str()), (1, "appearance"));
    let history = History::read(&files[&l.history_file()]).expect("a history");
    assert_eq!(history.visits.len(), 4);
    let kept = |class: Class, name: &str| l.backups(class).join(SESSION).join("legacy-profile").join(name);
    assert_eq!(files[&kept(Class::Config, "settings.conf")], TEMPLATE_F6EE26A.as_bytes());
    assert_eq!(files[&kept(Class::State, "workspace.json")], workspace("dark"));
    let left: Vec<&PathBuf> = files.keys().filter(|p| p.starts_with(OLD)).collect();
    assert_eq!(left, [&old(MIGRATED_FILE)], "the old folder holds only the note");
    let note = String::from_utf8(files[&old(MIGRATED_FILE)].clone()).expect("the note is text");
    assert!(note.starts_with("F1R3Gaze moved this profile on 2026-10-05 at 18:21:02 UTC.\n"), "{note}");
    assert!(note.contains("\n  user-id                 data/user-id\n"), "{note}");
    assert!(note.contains("config/settings.toml (no settings were set: it is the new template)"), "{note}");
    assert!(note.contains("state/session.json (1 tab) and state/history.json (4 visits)"), "{note}");
    assert!(note.contains("your theme was \"dark\", the old default"), "{note}");
    assert!(!note.contains("://"), "the note holds no address: {note}");
    assert!(
        started.events.iter().all(|e| e.severity <= Severity::Notice),
        "nothing to worry about: {:?}",
        started.events
    );
    assert!(started.events.iter().any(|e| matches!(e.kind, EventKind::Migrated { .. })));
    assert_eq!(started.reconciled.user_id, String::from_utf8_lossy(USER_ID));
    // A second start does not move anything again, and writes only its flag.
    let trace = Arc::new(TraceFs::new(Arc::new(MemFs::clone(&fs))));
    let again = start_at(&l, trace.clone(), Access::Write, STAMP + 60).expect("a second start");
    assert_eq!(again.migration, Migration::Current);
    assert_eq!(
        writes_of(&trace),
        [("create_new", l.busy_file()), ("remove_file", l.busy_file()), ("sync_dir", l.data.clone())]
    );
}

/// A valid wallet: its key file's name and content, and its address.
fn wallet(byte: u8) -> (String, String, gaze_wallet::Address) {
    let hex = gaze_net::hex(&[byte; 32]);
    let key = gaze_shard::keys::parse_key_file(hex.as_bytes()).expect("a valid secret");
    let address = gaze_wallet::Address::from_key(key.verifying_key());
    let file = gaze_shard::keys::key_file_name(&gaze_wallet::wallet_key_name(&address));
    (file, hex, address)
}

/// An old profile with every kind of item, and things that are not.
fn full_profile(fs: &MemFs) {
    let (key, hex, address) = wallet(1);
    let stem = key.strip_suffix(".key").expect("a key file");
    let gzs = format!("{}.gzs", "b".repeat(64));
    fs.seed_file(old("settings.conf"), b"quorum = 3\n");
    fs.seed_file(old("workspace.json"), &workspace("light"));
    fs.seed_file(old("palette.css"), b":root { --gaze-bg: #101010; }\n");
    fs.seed_file(old("user-id"), USER_ID);
    fs.seed_file(old("wallets.tsv"), format!("{address}\tSpending\n").as_bytes());
    fs.seed_file(old("wallet-active"), format!("{address}\n").as_bytes());
    fs.seed_file(old("grants.tsv"), format!("https://a.example\t{}\turn:f1r3:site\tallow\tread\n", "a".repeat(64)).as_bytes());
    fs.seed_file(old(&format!("keys/{key}")), hex.as_bytes());
    fs.seed_file(old(&format!("keys/{stem}.tmp")), hex.as_bytes());
    fs.seed_file(old("keys/notes.txt"), b"mine");
    fs.seed_file(old("exports/wallet.json"), b"{}");
    fs.seed_file(old("store/origins.json"), br#"["https://a.example"]"#);
    fs.seed_file(old(&format!("store/{gzs}")), b"records");
    fs.seed_file(old("store/other.txt"), b"x");
    fs.seed_file(old("logs/run.gzlog"), b"log");
    fs.seed_file(old("logs/readme"), b"x");
    fs.seed_file(old("cache/ab/abcdef"), b"blob");
    fs.seed_file(old("cache/zz/x"), b"not a shard");
    fs.seed_file(old("notes.txt"), b"my notes");
    fs.seed_dir(old("misc"));
}

#[test]
fn a_full_legacy_profile_moves_every_item() {
    let l = xdg();
    let fs = Arc::new(MemFs::new());
    full_profile(&fs);
    let (key, hex, _) = wallet(1);
    let stem = key.strip_suffix(".key").expect("a key file").to_string();
    let before = fs.files();
    let started = start(&l, &fs);
    let summary = done(&started).clone();
    let files = fs.files();
    let moved = [
        ("palette.css", l.themes_dir().join("custom.css")),
        ("user-id", l.user_id_file()),
        ("wallets.tsv", l.wallet_dir().join("wallets.tsv")),
        ("wallet-active", l.wallet_dir().join("wallet-active")),
        ("grants.tsv", l.grants_file()),
        (&format!("keys/{key}"), l.keys_dir().join(&key)),
        (&format!("keys/{stem}.tmp"), l.keys_dir().join(format!("{stem}.tmp"))),
        ("exports/wallet.json", l.exports_dir().join("wallet.json")),
        ("store/origins.json", l.origins_file()),
        (&format!("store/{}.gzs", "b".repeat(64)), l.site_data_dir().join(format!("{}.gzs", "b".repeat(64)))),
        ("logs/run.gzlog", l.replay_logs_dir().join("run.gzlog")),
        ("cache/ab/abcdef", l.content_dir().join("ab/abcdef")),
    ];
    for (name, dst) in &moved {
        assert_eq!(files.get(dst), before.get(&old(name)), "{name} → {}", dst.display());
        assert!(!files.contains_key(&old(name)), "{name} left the old folder");
        assert_eq!(fs.mode(dst).expect("its mode"), Some(0o600), "{}", dst.display());
    }
    assert_eq!(files[&l.keys_dir().join(&key)], hex.as_bytes());
    for name in ["keys/notes.txt", "store/other.txt", "logs/readme", "cache/zz/x", "notes.txt"] {
        assert_eq!(files.get(&old(name)), before.get(&old(name)), "{name} stays");
    }
    // Light, with quorum 3: both converted.
    let (applied, diagnostics) = {
        let mut applied = settings::Settings::default();
        let mut diagnostics = Vec::new();
        let text = String::from_utf8(files[&l.settings_file()].clone()).expect("text");
        settings::apply(&mut applied, &l.settings_file(), &text, &mut diagnostics).expect("valid");
        (applied, diagnostics)
    };
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(applied.shard.quorum, 3);
    assert_eq!(applied.theme.as_str(), "default-light");
    let note = String::from_utf8(files[&old(MIGRATED_FILE)].clone()).expect("text");
    for line in [
        "  cache/ (1 folder)       cache/content/\n",
        "  keys/notes.txt          not part of a F1R3Gaze profile\n",
        "  cache/zz                not part of F1R3Gaze's cache\n",
        "  misc/                   not part of a F1R3Gaze profile\n",
        "  notes.txt               not part of a F1R3Gaze profile\n",
        "Theme: light, as before",
    ] {
        assert!(note.contains(line), "{line:?} in {note}");
    }
    assert_eq!(summary.moved, moved.len());
    assert!(started.events.iter().all(|e| e.severity <= Severity::Notice), "{:?}", started.events);
}

/// The folders the move emptied go too: the old folder keeps its note and
/// what was not moved, nothing else (ledger S14: the CI's migration check
/// found `keys/`, `exports/`, `store/`, `logs/` and `cache/` left empty).
#[test]
fn emptied_folders_of_the_old_profile_go_too() {
    let l = xdg();
    let under_old = |fs: &MemFs| -> (Vec<PathBuf>, Vec<PathBuf>) {
        let files = fs.files().into_keys().filter(|p| p.starts_with(OLD)).collect();
        let dirs = fs.dirs().into_iter().filter(|p| p.starts_with(OLD)).collect();
        (files, dirs)
    };
    let every_kind = |fs: &MemFs| {
        let (key, hex, address) = wallet(1);
        fs.seed_file(old(&format!("keys/{key}")), hex.as_bytes());
        fs.seed_file(old("wallets.tsv"), format!("{address}\tSpending\n").as_bytes());
        fs.seed_file(old("exports/wallet.json"), b"{}");
        fs.seed_file(old(&format!("store/{}.gzs", "b".repeat(64))), b"records");
        fs.seed_file(old("logs/run.gzlog"), b"log");
        fs.seed_file(old("cache/ab/abcdef"), b"blob");
    };
    // Everything moved: only the note is left.
    let fs = Arc::new(users_profile());
    every_kind(&fs);
    done(&start(&l, &fs));
    assert_eq!(under_old(&fs), (vec![old(MIGRATED_FILE)], vec![PathBuf::from(OLD)]));
    assert_eq!(fs.pending_len(), 0, "the removals are durable");
    // Something not moved keeps its folder.
    let fs = Arc::new(users_profile());
    every_kind(&fs);
    fs.seed_file(old("keys/notes.txt"), b"mine");
    done(&start(&l, &fs));
    assert_eq!(under_old(&fs), (vec![old(MIGRATED_FILE), old("keys/notes.txt")], vec![PathBuf::from(OLD), old("keys")]));
    // Across devices, the cache stays behind, in its folder.
    let fs = Arc::new(users_profile().with_device(OLD));
    every_kind(&fs);
    done(&start(&l, &fs));
    let (files, dirs) = under_old(&fs);
    assert_eq!(files, [old(MIGRATED_FILE), old("cache/ab/abcdef")]);
    assert_eq!(dirs, [PathBuf::from(OLD), old("cache"), old("cache/ab")]);
}

#[test]
fn a_portable_root_is_migrated_in_place() {
    let l = Layout::portable(Path::new("/p"));
    let fs = Arc::new(MemFs::new());
    fs.seed_file("/p/settings.conf", b"https_only = true\n");
    fs.seed_file("/p/user-id", USER_ID);
    fs.seed_file("/p/cache/ab/abcdef", b"blob");
    let started = start(&l, &fs);
    done(&started);
    let files = fs.files();
    assert_eq!(files[&l.user_id_file()], USER_ID);
    assert_eq!(files[Path::new("/p/cache/content/ab/abcdef")], b"blob");
    assert!(String::from_utf8_lossy(&files[&l.settings_file()]).contains("https_only = true"));
    let note = String::from_utf8(files[Path::new("/p/MIGRATED.txt")].clone()).expect("text");
    // The new layout's own folders (config/, data/, state/, runtime/ and
    // cache/content) are not listed as left: nothing else was in /p.
    assert!(!note.contains("Left here"), "{note}");
    assert_eq!(start(&l, &fs).migration, Migration::Current);
}

#[test]
fn conflicts_keep_both() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    // A different user id is already in the new place, the same grants
    // too, and a session that is not what the old workspace converts to.
    let grants = format!("https://a.example\t{}\turn:f1r3:site\tallow\tread\n", "a".repeat(64));
    fs.seed_file(old("grants.tsv"), grants.as_bytes());
    fs.seed_file(l.user_id_file(), b"ffffffffffffffffffffffffffffffff");
    fs.seed_file(l.grants_file(), grants.as_bytes());
    let mine = SessionState {
        sidebar_open: true,
        ..SessionState::default()
    }
    .to_bytes();
    fs.seed_file(l.session_file(), &mine);
    let started = start(&l, &fs);
    let summary = done(&started).clone();
    let files = fs.files();
    assert_eq!(files[&l.user_id_file()], b"ffffffffffffffffffffffffffffffff", "never overwritten");
    assert_eq!(files[&old("user-id")], USER_ID, "the old one is kept");
    assert_eq!(files[&l.session_file()], mine, "never overwritten");
    assert!(!files.contains_key(&old("grants.tsv")), "the same grants: the old copy goes");
    assert_eq!(files[&l.grants_file()], grants.as_bytes());
    // The workspace is still kept in the backups.
    assert_eq!(
        files[&l.backups(Class::State).join(SESSION).join("legacy-profile/workspace.json")],
        workspace("dark")
    );
    let mut conflicts = summary.conflicts.clone();
    conflicts.sort();
    assert_eq!(conflicts, ["user-id", "workspace.json"]);
    let warnings: Vec<&Event> = started.events.iter().filter(|e| matches!(e.kind, EventKind::Conflict { .. })).collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    let note = String::from_utf8(files[&old(MIGRATED_FILE)].clone()).expect("text");
    for line in [
        "Not moved (something was already there; both are kept)\n",
        "\n  workspace.json          state/session.json already exists\n",
        "\n  user-id                 data/user-id already exists\n",
        "state/history.json (4 visits); state/session.json was already there\n",
        "\n  user-id                 not moved: data/user-id already exists; both are kept\n",
    ] {
        assert!(note.contains(line), "{line:?} in {note}");
    }
}

#[test]
fn moves_across_file_systems() {
    let l = xdg();
    let fs = Arc::new(users_profile().with_device(OLD));
    fs.seed_file(old("grants.tsv"), format!("https://a.example\t{}\turn:f1r3:site\tallow\tread\n", "a".repeat(64)).as_bytes());
    let before = fs.files();
    let started = start(&l, &fs);
    done(&started);
    let files = fs.files();
    assert_eq!(files[&l.user_id_file()], USER_ID);
    assert_eq!(files.get(&l.grants_file()), before.get(&old("grants.tsv")));
    let left: Vec<&PathBuf> = files.keys().filter(|p| p.starts_with(OLD)).collect();
    assert_eq!(left, [&old(MIGRATED_FILE)]);
    assert_eq!(fs.pending_len(), 0);
}

#[test]
fn the_cache_is_left_behind_across_devices() {
    let l = xdg();
    let fs = Arc::new(users_profile().with_device(OLD));
    fs.seed_file(old("cache/ab/x"), b"one");
    fs.seed_file(old("cache/cd/y"), b"two");
    let started = start(&l, &fs);
    let summary = done(&started).clone();
    let files = fs.files();
    assert_eq!(files[&old("cache/ab/x")], b"one");
    assert_eq!(files[&old("cache/cd/y")], b"two");
    assert!(!files.keys().any(|p| p.starts_with(l.content_dir())), "nothing was copied");
    assert_eq!(summary.left.len(), 2);
    let note = String::from_utf8(files[&old(MIGRATED_FILE)].clone()).expect("text");
    assert!(note.contains("  cache/ab                on another file system; F1R3Gaze fetches it again, so you may delete it\n"), "{note}");
}

#[test]
fn the_cache_moves_on_one_device() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    fs.seed_file(old("cache/ab/x"), b"one");
    fs.seed_file(old("cache/cd/y"), b"two");
    let started = start(&l, &fs);
    done(&started);
    let files = fs.files();
    assert_eq!(files[&l.content_dir().join("ab/x")], b"one");
    assert_eq!(files[&l.content_dir().join("cd/y")], b"two");
    assert!(!files.contains_key(&old("cache/ab/x")));
    let note = String::from_utf8(files[&old(MIGRATED_FILE)].clone()).expect("text");
    assert!(note.contains("  cache/ (2 folders)      cache/content/\n"), "{note}");
}

#[test]
fn a_profile_is_migrated_once() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    done(&start(&l, &fs));
    // An old F1R3Gaze started afterwards writes a new old profile.
    fs.seed_file(old("user-id"), b"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    assert_eq!(start(&l, &fs).migration, Migration::Current);
    assert_eq!(fs.files()[&l.user_id_file()], USER_ID);
    // Without the marker (the new data folder deleted), the note still
    // stops a second migration.
    let fs = Arc::new(users_profile());
    fs.seed_file(old(MIGRATED_FILE), b"moved");
    assert_eq!(detect(&l, fs.as_ref(), &Report::silent()), Ok(Detected::Fresh));
}

#[test]
fn a_fresh_profile_never_migrates_later() {
    let l = xdg();
    let fs = Arc::new(MemFs::new());
    assert_eq!(start(&l, &fs).migration, Migration::Fresh);
    let marker = Marker::read(&fs.files()[&l.marker_file()]).expect("a marker");
    assert_eq!((marker.created.as_deref(), marker.migrated_from), (Some("2026-10-05T18:21:02Z"), None));
    // An old profile copied in later is not moved.
    for (path, bytes) in users_profile().files() {
        fs.seed_file(path, &bytes);
    }
    assert_eq!(start(&l, &fs).migration, Migration::Current);
    assert_eq!(fs.files()[&old("user-id")], USER_ID);
}

#[test]
fn symlinks_are_moved_not_followed() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    fs.remove_file(&old("user-id")).expect("rm");
    fs.seed_file("/elsewhere/id", USER_ID);
    fs.seed_symlink(old("user-id"), "/elsewhere/id");
    fs.seed_file("/elsewhere/wallets.tsv", b"");
    fs.seed_symlink(old("wallets.tsv"), "../../../../elsewhere/wallets.tsv");
    fs.seed_file("/elsewhere/keys/x.key", b"k");
    fs.seed_symlink(old("keys"), "/elsewhere/keys");
    fs.remove_file(&old("settings.conf")).expect("rm");
    fs.seed_file("/dotfiles/settings.conf", b"quorum = 4\n");
    fs.seed_symlink(old("settings.conf"), "/dotfiles/settings.conf");
    let started = start(&l, &fs);
    done(&started);
    // The absolute link moved as a link; its target is untouched.
    assert!(matches!(fs.kind(&l.user_id_file()), Ok(Kind::Symlink)));
    assert_eq!(fs.read_link(&l.user_id_file()).expect("a link"), PathBuf::from("/elsewhere/id"));
    assert_eq!(fs.files()[Path::new("/elsewhere/id")], USER_ID);
    // The relative link and the linked folder stay.
    assert!(matches!(fs.kind(&old("wallets.tsv")), Ok(Kind::Symlink)));
    assert!(matches!(fs.kind(&old("keys")), Ok(Kind::Symlink)));
    assert_eq!(fs.files()[Path::new("/elsewhere/keys/x.key")], b"k");
    // The linked settings were converted through the link, which stays.
    assert!(matches!(fs.kind(&old("settings.conf")), Ok(Kind::Symlink)));
    assert_eq!(fs.files()[Path::new("/dotfiles/settings.conf")], b"quorum = 4\n");
    assert!(String::from_utf8_lossy(&fs.files()[&l.settings_file()]).contains("quorum = 4"));
    let note = String::from_utf8(fs.files()[&old(MIGRATED_FILE)].clone()).expect("text");
    for line in [
        "  wallets.tsv             a relative link, which would point elsewhere once moved\n",
        "  keys                    a linked folder: left as it is\n",
        "  settings.conf           converted; the link was left\n",
    ] {
        assert!(note.contains(line), "{line:?} in {note}");
    }
}

#[test]
fn a_failed_migration_opens_read_only_and_resumes() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    fs.seed_file(old("palette.css"), b":root { --gaze-bg: #101010; }\n");
    // A file where the themes folder must go: step M2 fails.
    fs.seed_file(l.themes_dir(), b"in the way");
    let started = start(&l, &fs);
    assert!(matches!(started.migration, Migration::Failed { .. }), "{:?}", started.migration);
    assert!(started.reconciled.blocked.all.is_some(), "the session only reads");
    assert!(
        started.events.iter().any(|e| matches!(e.kind, EventKind::MigrationPending { .. }) && e.severity == Severity::Alert),
        "{:?}",
        started.events
    );
    assert!(fs.files().contains_key(&l.migration_dir().join(PLAN_FILE)), "the plan stays");
    assert!(fs.files().contains_key(&l.busy_file()), "the start did not finish");
    // Once the way is clear, the next start finishes the move.
    fs.remove_file(&l.themes_dir()).expect("rm");
    let resumed = start(&l, &fs);
    assert!(done(&resumed).resumed);
    assert_eq!(fs.files()[&l.themes_dir().join("custom.css")], b":root { --gaze-bg: #101010; }\n");
    assert!(!fs.files().contains_key(&l.busy_file()));
}

#[test]
fn a_resumed_migration_keeps_its_backup_folder() {
    let l = xdg();
    let start_fs = users_profile();
    // A start that dies right after the plan's folders are made.
    let crashed = Arc::new(MemFs::clone(&start_fs));
    let total = {
        let whole = Arc::new(MemFs::clone(&start_fs));
        whole.reset_ops();
        start(&l, &whole);
        whole.ops()
    };
    let mut resumed_once = false;
    for k in (0..total).step_by(7) {
        let fs = Arc::new(MemFs::clone(&crashed));
        fs.fail_after(k);
        let _ = start_at(&l, fs.clone(), Access::Write, STAMP);
        fs.revive();
        if !fs.files().contains_key(&l.migration_dir().join(PLAN_FILE)) {
            continue;
        }
        // The next start is a day later; the plan's session is used. (After
        // a crash between the marker and the clean-up, it finds the move
        // done and only removes the plan.)
        let later = start_at(&l, fs.clone(), Access::Write, STAMP + 86_400).expect("the next start");
        assert!(matches!(later.migration, Migration::Done(_) | Migration::Current), "{k}: {:?}", later.migration);
        assert!(!fs.files().contains_key(&l.migration_dir().join(PLAN_FILE)), "{k}: the plan is gone");
        let kept = l.backups(Class::Config).join(SESSION).join("legacy-profile/settings.conf");
        assert_eq!(fs.files().get(&kept).map(Vec::as_slice), Some(TEMPLATE_F6EE26A.as_bytes()), "{k}");
        resumed_once = true;
    }
    assert!(resumed_once, "some crash left a plan to resume");
}

/// A plan file with its fields changed by `change`.
fn plan_file(l: &Layout, fs: &MemFs, change: impl Fn(&mut serde_json::Value)) {
    let mut value = serde_json::to_value(
        plan(l, fs, Path::new(OLD), UNIX_EPOCH + Duration::from_secs(STAMP)).expect("a plan"),
    )
    .expect("a plan serializes");
    change(&mut value);
    fs.seed_file(l.migration_dir().join(PLAN_FILE), &serde_json::to_vec(&value).expect("json"));
}

#[test]
fn a_plan_from_other_roots_is_not_replayed() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    plan_file(&l, &fs, |v| v["roots"]["data"] = serde_json::Value::from("/elsewhere/data"));
    let started = start(&l, &fs);
    match &started.migration {
        Migration::Failed { error } => assert!(error.contains("planned for other folders"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs.files()[&old("user-id")], USER_ID, "nothing moved");
}

#[test]
fn a_newer_plan_is_not_replayed() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    plan_file(&l, &fs, |v| v["format"] = serde_json::Value::from(PLAN_FORMAT + 1));
    match start(&l, &fs).migration {
        Migration::Failed { error } => assert!(error.contains("a newer F1R3Gaze started the move"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs.files()[&old("user-id")], USER_ID, "nothing moved");
}

#[test]
fn a_newer_marker_opens_read_only() {
    let l = xdg();
    let fs = MemFs::new();
    fs.seed_file(l.marker_file(), br#"{"layout":2}"#);
    let trace = Arc::new(TraceFs::new(Arc::new(fs)));
    let started = start_at(&l, trace.clone(), Access::Write, STAMP).expect("a start");
    assert_eq!(started.migration, Migration::Newer { layout: 2 });
    assert!(started.reconciled.blocked.all.is_some());
    let writes = writes_of(&trace);
    let busy = l.busy_file();
    assert!(writes.iter().all(|(_, p)| *p == busy || l.data.starts_with(p) || *p == l.data), "only the flag: {writes:?}");
}

#[test]
fn detection_needs_a_signature() {
    let l = xdg();
    let report = Report::silent();
    // Folder names alone are not a profile.
    let fs = MemFs::new();
    fs.seed_dir(old("store"));
    fs.seed_dir(old("cache/ab"));
    assert_eq!(detect(&l, &fs, &report), Ok(Detected::Fresh));
    assert!(report.events().is_empty());
    // Keys alone are reported, not moved.
    fs.seed_file(old(&format!("keys/{}.key", "c".repeat(64))), b"k");
    assert_eq!(detect(&l, &fs, &report), Ok(Detected::Fresh));
    assert_eq!(report.events().len(), 1);
    assert!(report.events()[0].message.contains("holds wallet keys"), "{:?}", report.events());
    // One signature file is enough.
    fs.seed_file(old("wallet-active"), b"");
    assert_eq!(detect(&l, &fs, &report), Ok(Detected::Legacy { root: PathBuf::from(OLD) }));
    // With two old profiles the first is moved, and the other reported.
    let mut two = l.clone();
    two.legacy = vec![PathBuf::from("/a"), PathBuf::from(OLD)];
    fs.seed_file("/a/user-id", USER_ID);
    let report = Report::silent();
    assert_eq!(detect(&two, &fs, &report), Ok(Detected::Legacy { root: PathBuf::from("/a") }));
    assert!(report.events().iter().any(|e| e.message.contains("was not moved")), "{:?}", report.events());
}

#[test]
fn migrated_txt_never_holds_urls() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    fs.seed_file(
        old("settings.conf"),
        b"home = https://home.example\nobservers = https://o.example\nquorum = 0\nembers_api = http://e.example\n",
    );
    done(&start(&l, &fs));
    let note = String::from_utf8(fs.files()[&old(MIGRATED_FILE)].clone()).expect("text");
    assert!(!note.contains("://"), "{note}");
    assert!(!note.contains("example"), "no address, title or value: {note}");
    assert!(note.contains("  shard.quorum\n"), "an unusable setting is named: {note}");
    assert!(!note.contains("= 0"), "never its value: {note}");
}

#[test]
fn moved_files_are_private() {
    let l = xdg();
    let fs = Arc::new(users_profile());
    fs.seed_file(old("grants.tsv"), b"");
    fs.set_mode(&old("grants.tsv"), 0o400).expect("chmod");
    done(&start(&l, &fs));
    assert_eq!(fs.mode(&l.user_id_file()).expect("mode"), Some(0o600), "0644 tightened");
    assert_eq!(fs.mode(&l.grants_file()).expect("mode"), Some(0o400), "only ever tightened");
}

#[test]
fn workspace_splits_for_each_theme() {
    let l = xdg();
    let custom_css = l.themes_dir().join("custom.css");
    let palette = b":root { --gaze-bg: #101010; }\n".to_vec();
    // (theme, palette.css, custom.css already there, expected theme setting)
    type Case<'a> = (&'a str, bool, Option<&'a [u8]>, Option<&'a str>);
    let cases: [Case<'_>; 7] = [
        ("dark", false, None, None),
        ("light", false, None, Some("default-light")),
        ("custom", true, None, Some("custom")),
        ("custom", true, Some(&palette), Some("custom")),
        ("custom", false, None, None),
        ("custom", true, Some(b"other"), None),
        ("sepia", false, None, None),
    ];
    for (theme, with_palette, existing, expected) in cases {
        let what = format!("{theme}, palette {with_palette}, custom.css {existing:?}");
        let fs = Arc::new(users_profile());
        fs.seed_file(old("workspace.json"), &workspace(theme));
        if with_palette {
            fs.seed_file(old("palette.css"), &palette);
        }
        if let Some(bytes) = existing {
            fs.seed_file(&custom_css, bytes);
        }
        let started = start(&l, &fs);
        done(&started);
        let files = fs.files();
        let text = String::from_utf8(files[&l.settings_file()].clone()).expect("text");
        match expected {
            Some(name) => assert!(text.contains(&format!("[appearance]\ntheme = \"{name}\"\n")), "{what}: {text}"),
            None => assert!(!text.contains("\ntheme = "), "{what}: {text}"),
        }
        let (session, history, _) = serde_json::from_slice::<UiState>(&workspace(theme)).expect("a workspace").split();
        assert_eq!(files[&l.session_file()], session.to_bytes(), "{what}");
        assert_eq!(files[&l.history_file()], history.to_bytes(), "{what}");
    }
    // A workspace that cannot be read: kept, and nothing converted from it.
    let fs = Arc::new(users_profile());
    fs.seed_file(old("workspace.json"), b"{\"tabs\":");
    let started = start(&l, &fs);
    done(&started);
    let files = fs.files();
    assert_eq!(files[&l.backups(Class::State).join(SESSION).join("legacy-profile/workspace.json")], b"{\"tabs\":");
    assert_eq!(files[&l.session_file()], SessionState::default().to_bytes(), "reconcile's default");
    let note = String::from_utf8(files[&old(MIGRATED_FILE)].clone()).expect("text");
    assert!(note.contains("workspace.json could not be read"), "{note}");
}

// ── Crashes ──────────────────────────────────────────────────────────────

/// Where each old file may be once its plan has run: its own place, where
/// it moves, or where it is kept.
fn places(p: &Plan, before: &BTreeMap<PathBuf, Vec<u8>>) -> BTreeMap<PathBuf, Vec<PathBuf>> {
    let mut places: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for path in before.keys().filter(|p| p.starts_with(OLD)) {
        places.insert(path.clone(), vec![path.clone()]);
    }
    for action in &p.actions {
        match action {
            Action::Move { src, dst, .. } => {
                if let Some(at) = places.get_mut(Path::new(src)) {
                    at.push(PathBuf::from(dst));
                }
            }
            Action::MoveCacheShard { src, dst } => {
                for (path, at) in places.iter_mut() {
                    if let Ok(below) = path.strip_prefix(src) {
                        at.push(Path::new(dst).join(below));
                    }
                }
            }
            _ => {}
        }
    }
    places
}

/// The model's invariants at a crash state of a migration:
/// - `NoLoss`: every old file's bytes are at its own place, its new one, or
///   where it is kept;
/// - `NoOverwrite`: what was in the new folders before is unchanged;
/// - `PublishedComplete`: every planned destination holds what it should,
///   never garbage or a partial copy;
/// - `MarkerImpliesComplete`: once the marker is there, every action is
///   done.
fn check_migration(p: &Plan, before: &BTreeMap<PathBuf, Vec<u8>>, l: &Layout, fs: &MemFs, when: &str) {
    let files = fs.files();
    for (path, at) in places(p, before) {
        let bytes = &before[&path];
        assert!(at.iter().any(|place| files.get(place) == Some(bytes)), "{when}: {} lost", path.display());
    }
    for (path, bytes) in before.iter().filter(|(p, _)| !p.starts_with(OLD)) {
        assert_eq!(files.get(path), Some(bytes), "{when}: {} changed", path.display());
    }
    let marked = files.get(&l.marker_file()).and_then(|m| Marker::read(m).ok()).is_some_and(|m| m.migrated_from.is_some());
    for action in &p.actions {
        match action {
            Action::Write { dst, text, .. } => {
                let now = files.get(Path::new(dst));
                let taken = before.contains_key(Path::new(dst));
                assert!(taken || now.is_none() || now == Some(&text.as_bytes().to_vec()), "{when}: {dst} holds {:?}", now.map(|b| String::from_utf8_lossy(b).into_owned()));
                assert!(!marked || taken || now.is_some(), "{when}: marked, but {dst} is missing");
            }
            Action::Move { src, dst, .. } => {
                let original = before.get(Path::new(src));
                let now = files.get(Path::new(dst));
                let taken = before.contains_key(Path::new(dst));
                assert!(taken || now.is_none() || now == original, "{when}: {dst} holds {:?}", now.map(|b| String::from_utf8_lossy(b).into_owned()));
                assert!(now != Some(&GARBAGE.to_vec()), "{when}: {dst} holds garbage");
                assert!(!marked || taken || (now == original && !files.contains_key(Path::new(src))), "{when}: marked, but {src} is not moved");
            }
            _ => {}
        }
    }
}

/// The profile apart from this process's own temporary files, which a real
/// restart, being another process, sweeps.
fn comparable(files: &BTreeMap<PathBuf, Vec<u8>>) -> BTreeMap<PathBuf, Vec<u8>> {
    let own = std::process::id();
    files
        .iter()
        .filter(|(p, _)| p.file_name().and_then(gaze_fs::temp_owner) != Some(own))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

/// An old profile with a moved, a converted and a kept file, keys, and a
/// cache shard.
fn crash_profile() -> MemFs {
    let fs = users_profile();
    let (key, hex, _) = wallet(1);
    fs.seed_file(old(&format!("keys/{key}")), hex.as_bytes());
    fs.seed_file(old("palette.css"), b":root { --gaze-bg: #101010; }\n");
    fs.seed_file(old("cache/ab/x"), b"blob");
    fs
}

#[test]
fn a_crash_at_any_step_resumes_and_loses_nothing() {
    let l = xdg();
    type Shape = (&'static str, fn() -> MemFs);
    let shapes: [Shape; 4] = [
        ("one device", crash_profile),
        ("one device, ntfs", || crash_profile().ntfs()),
        ("another device", || crash_profile().with_device(OLD)),
        ("another device, ntfs", || crash_profile().with_device(OLD).ntfs()),
    ];
    for (name, shape) in shapes {
        let start_fs = shape();
        let before = start_fs.files();
        let planned = plan(&l, &start_fs, Path::new(OLD), UNIX_EPOCH + Duration::from_secs(STAMP)).expect("a plan");
        let finished = Arc::new(shape());
        done(&start(&l, &finished));
        let final_files = comparable(&finished.files());
        let posix = !name.ends_with("ntfs");
        let checked = every_crash(
            &start_fs,
            |fs| start_at(&l, fs.clone(), Access::Write, STAMP),
            |crash, fs| {
                let when = format!("{name}: {crash}");
                check_migration(&planned, &before, &l, fs, &when);
                let next = Arc::new(MemFs::clone(fs));
                let started = start_at(&l, next.clone(), Access::Write, STAMP)
                    .unwrap_or_else(|e| panic!("{when}: the next start fails: {e}"));
                assert!(
                    matches!(started.migration, Migration::Done(_) | Migration::Current),
                    "{when}: {:?}",
                    started.migration
                );
                if posix {
                    assert_eq!(next.pending(), Vec::<String>::new(), "{when}");
                }
                assert_eq!(comparable(&next.files()), final_files, "{when}: the next start");
            },
        );
        eprintln!("{name}: {checked} crash states checked");
        assert!(checked >= 300, "{name}: {checked}");
    }
}

#[test]
fn two_crashes_never_lose_an_item() {
    // The model's shape: u moves on one file system, k to another, s is
    // converted (its output so) and kept.
    let l = xdg();
    let shape = || {
        let fs = MemFs::new().with_device(xdg().keys_dir());
        let (key, hex, _) = wallet(1);
        fs.seed_file(old("user-id"), USER_ID);
        fs.seed_file(old(&format!("keys/{key}")), hex.as_bytes());
        fs.seed_file(old("settings.conf"), b"quorum = 3\n");
        fs
    };
    let start_fs = shape();
    let before = start_fs.files();
    let planned = plan(&l, &start_fs, Path::new(OLD), UNIX_EPOCH + Duration::from_secs(STAMP)).expect("a plan");
    let checked = every_two_crashes(
        &start_fs,
        |_| {},
        |fs| start_at(&l, fs.clone(), Access::Write, STAMP),
        |crash, fs| check_migration(&planned, &before, &l, fs, &crash.to_string()),
    );
    eprintln!("{checked} states after two crashes checked");
    assert!(checked >= 1_000, "{checked}");
}

#[test]
fn a_file_that_appears_meanwhile_is_never_replaced() {
    // Another program creates the converted session and the moved user id
    // just before F1R3Gaze publishes its own: they are conflicts, and what
    // the program wrote is kept.
    // What the program writes is valid, so start-up's repair keeps it too.
    let l = xdg();
    let session = SessionState {
        sidebar_open: true,
        ..SessionState::default()
    }
    .to_bytes();
    for (dst, theirs) in [(l.session_file(), session), (l.user_id_file(), b"ffffffffffffffffffffffffffffffff".to_vec())] {
        let mem = Arc::new(users_profile());
        let target = dst.clone();
        let inner = mem.clone();
        let written = theirs.clone();
        let appeared = std::sync::atomic::AtomicBool::new(false);
        let racing = Arc::new(TraceFs::with_hook(mem.clone(), move |t| {
            let publishes = matches!(t.op, "hard_link" | "rename") && t.to.as_deref() == Some(target.as_path());
            if publishes && !appeared.swap(true, std::sync::atomic::Ordering::SeqCst) {
                inner.seed_file(&target, &written);
            }
        }));
        let started = start_at(&l, racing, Access::Write, STAMP).expect("a start");
        let summary = done(&started).clone();
        let what = dst.display().to_string();
        assert_eq!(mem.files()[&dst], theirs, "{what}: never replaced");
        assert_eq!(summary.conflicts.len(), 1, "{what}: {summary:?}");
        // The old file is still somewhere: in place, or in the backups.
        let files = mem.files();
        assert!(files.contains_key(&old("user-id")) || dst != l.user_id_file(), "{what}");
        assert!(
            files.contains_key(&l.backups(Class::State).join(SESSION).join("legacy-profile/workspace.json")),
            "{what}"
        );
    }
}
