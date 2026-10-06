//! The `f1r3gaze` command against real folders (ledger S14): `paths` and
//! `profile check` write nothing, the instance lock keeps a second writer
//! out while readers go on, a first start makes a private skeleton and a
//! second writes nothing, an old profile is moved, and the recovery
//! commands (`trust`, `profile backups`) change only what they say.
//!
//! Every run is on a portable profile (`--profile`) under the scratch
//! folder, with `HOME`, every XDG variable and `F1R3GAZE_PROFILE` pointed
//! away from the user's own: no run can find, or move, a real profile.
// The cases write their fixtures directly.
#![allow(clippy::disallowed_methods)]

use gaze_shell::profile::layout::Layout;
use gaze_shell::profile::lock::{InstanceLock, Mode};
use gaze_shell::profile::report::Report;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

/// The F1R3Gaze built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_f1r3gaze");

/// A scratch folder for one test, holding the profile root `profile/` and
/// an isolated home `home/`.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = gaze_fs::scratch_dir(&format!("gaze-shell-cli-{name}"));
        std::fs::create_dir_all(dir.join("home")).expect("home/");
        Scratch { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.join("profile")
    }

    fn layout(&self) -> Layout {
        Layout::portable(&self.root())
    }

    /// A settings file that starts no event thread: no observers.
    fn quiet_settings(&self) {
        let l = self.layout();
        std::fs::create_dir_all(&l.config).expect("config/");
        std::fs::write(l.settings_file(), "[shard]\nobservers = []\n").expect("settings.toml");
    }

    /// Runs `f1r3gaze --profile <root> args…`, isolated.
    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(BIN);
        command.arg("--profile").arg(self.root()).args(args);
        self.isolate(&mut command);
        command.output().expect("f1r3gaze runs")
    }

    /// Runs `f1r3gaze args…` without `--profile`, isolated.
    fn run_bare(&self, args: &[&str]) -> Output {
        let mut command = Command::new(BIN);
        command.args(args);
        self.isolate(&mut command);
        command.output().expect("f1r3gaze runs")
    }

    fn isolate(&self, command: &mut Command) {
        let home = self.dir.join("home");
        command.env("HOME", &home).env("TMPDIR", self.dir.join("home")).env_remove("F1R3GAZE_PROFILE");
        for var in [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_DIRS",
            "XDG_DATA_DIRS",
            "F1R3GAZE_CRASH_AT",
            "F1R3GAZE_STORAGE_TRACE",
        ] {
            command.env_remove(var);
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// What a file or folder is, for comparing trees: size, mode, inode,
/// modification time and content.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    dir: bool,
    len: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    inode: u64,
    modified: Option<SystemTime>,
    bytes: Option<Vec<u8>>,
}

/// How folders are compared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Folders {
    /// With their modification times: nothing at all may change.
    Exactly,
    /// Without them: a start creates and removes its busy flag, and
    /// rewrites who holds the lock, in `data/` and `runtime/`.
    ButTimes,
}

/// Everything under `root`, by path; `skip` leaves files out by name.
fn tree(root: &Path, skip: &[&str], folders: Folders) -> BTreeMap<PathBuf, Entry> {
    let mut all = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(names) = std::fs::read_dir(&dir) else { continue };
        for entry in names.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip.contains(&name.as_str()) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).expect("stat");
            if meta.is_dir() {
                stack.push(path.clone());
            }
            all.insert(
                path.clone(),
                Entry {
                    dir: meta.is_dir(),
                    len: meta.len(),
                    #[cfg(unix)]
                    mode: std::os::unix::fs::PermissionsExt::mode(&meta.permissions()),
                    #[cfg(unix)]
                    inode: std::os::unix::fs::MetadataExt::ino(&meta),
                    modified: match (meta.is_dir(), folders) {
                        (true, Folders::ButTimes) => None,
                        _ => meta.modified().ok(),
                    },
                    bytes: meta.is_file().then(|| std::fs::read(&path).expect("read")),
                },
            );
        }
    }
    all
}

/// The paths whose entries differ between two trees.
fn differences(before: &BTreeMap<PathBuf, Entry>, after: &BTreeMap<PathBuf, Entry>) -> Vec<String> {
    let mut paths: Vec<&PathBuf> = before.keys().chain(after.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .map(|p| match (before.get(p), after.get(p)) {
            (Some(_), None) => format!("removed {}", p.display()),
            (None, Some(_)) => format!("added {}", p.display()),
            _ => format!("changed {}", p.display()),
        })
        .collect()
}

/// The lock files, which every start rewrites (who holds the profile).
const LOCK_FILES: [&str; 2] = ["instance.lock", "instance.pid"];

/// An old single-folder profile laid flat at the portable root.
fn legacy_root(s: &Scratch) {
    let root = s.root();
    std::fs::create_dir_all(&root).expect("the root");
    std::fs::write(root.join("settings.conf"), "# F1R3Gaze settings. Lists are comma-separated.\nobservers =\n")
        .expect("settings.conf");
    std::fs::write(root.join("user-id"), "5f1c2d3e4b5a69788796a5b4c3d2e1f0").expect("user-id");
    std::fs::write(
        root.join("workspace.json"),
        r#"{"theme":"dark","panel":"tabs","tabs":[{"url":"https://example.org/","title":"Example"}],"visits":[]}"#,
    )
    .expect("workspace.json");
}

#[test]
fn paths_names_the_roots_and_creates_nothing() {
    let s = Scratch::new("paths");
    let out = s.run(&["paths"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let printed = text(&out.stdout);
    for class in ["config", "data", "state", "cache", "runtime"] {
        let line = format!("{class}\t{}\n", s.root().join(class).display());
        assert!(printed.contains(&line), "{line:?} in {printed}");
    }
    assert!(printed.contains(&format!("legacy\t{}\n", s.root().display())), "{printed}");
    assert!(!s.root().exists(), "nothing was made");
    // Without --profile: the platform's folders, under the isolated home.
    let bare = s.run_bare(&["paths"]);
    assert!(bare.status.success(), "{}", text(&bare.stderr));
    let home = s.dir.join("home");
    for line in text(&bare.stdout).lines() {
        let (_, path) = line.split_once('\t').expect("class<TAB>path");
        let system = path.starts_with("/etc/") || path.starts_with("/usr/") || path.starts_with("/Library/");
        assert!(Path::new(path).starts_with(&home) || system, "{line} is outside the test's home");
    }
}

#[test]
fn profile_check_writes_nothing() {
    let s = Scratch::new("check");
    legacy_root(&s);
    let before = tree(&s.dir, &[], Folders::Exactly);
    let out = s.run(&["profile", "check"]);
    assert_eq!(out.status.code(), Some(1), "a start would change something: {}", text(&out.stdout));
    let printed = text(&out.stdout);
    assert!(printed.contains("an old profile at"), "{printed}");
    assert!(printed.contains("would be moved"), "{printed}");
    assert!(printed.contains("move   "), "the plan is listed: {printed}");
    let after = tree(&s.dir, &[], Folders::Exactly);
    assert_eq!(differences(&before, &after), Vec::<String>::new(), "nothing changed: names, modes, inodes, times, bytes");
    // Once moved, a check finds nothing to do.
    let moved = s.run(&["wallet", "list"]);
    assert!(moved.status.success(), "{}", text(&moved.stderr));
    let again = s.run(&["profile", "check"]);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again.stdout));
    assert!(text(&again.stdout).contains("a start would change nothing"), "{}", text(&again.stdout));
}

#[test]
fn a_held_profile_refuses_a_second_writer_naming_the_holder() {
    let s = Scratch::new("held");
    s.quiet_settings();
    let holder = InstanceLock::acquire(&s.layout(), Mode::Window, SystemTime::now(), &Report::silent()).expect("held");
    for args in [&["--headless", "gaze://newtab"][..], &["wallet", "new"], &["trust", "forget", "--all"]] {
        let out = s.run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", text(&out.stdout));
        let said = text(&out.stderr);
        assert!(said.contains(&format!("process {}", std::process::id())), "{args:?}: {said}");
        assert!(said.contains("--profile DIR"), "{args:?}: {said}");
    }
    drop(holder);
}

#[test]
fn read_only_commands_go_on_while_the_profile_is_held() {
    let s = Scratch::new("readers");
    s.quiet_settings();
    assert!(s.run(&["wallet", "list"]).status.success(), "a first start");
    let holder = InstanceLock::acquire(&s.layout(), Mode::Window, SystemTime::now(), &Report::silent()).expect("held");
    let before = tree(&s.root(), &LOCK_FILES, Folders::ButTimes);
    for args in [&["wallet", "list"][..], &["profile", "backups"], &["trust", "list"]] {
        let out = s.run(args);
        assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
        assert!(text(&out.stderr).contains("this session only reads"), "{args:?}: {}", text(&out.stderr));
    }
    let after = tree(&s.root(), &LOCK_FILES, Folders::ButTimes);
    assert_eq!(differences(&before, &after), Vec::<String>::new(), "the readers wrote nothing");
    drop(holder);
}

#[cfg(unix)]
#[test]
fn a_first_start_makes_a_private_skeleton() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("skeleton");
    let out = s.run(&["wallet", "list"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let l = s.layout();
    for dir in l.skeleton() {
        let mode = std::fs::metadata(&dir).expect("made").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", dir.display());
    }
    for (path, entry) in tree(&s.root(), &[], Folders::Exactly) {
        let mode = entry.mode & 0o7777;
        match entry.dir {
            true => assert_eq!(mode & 0o777, 0o700, "{}", path.display()),
            // The runtime lock also has the sticky bit on Linux.
            false if path == l.lock_file() && cfg!(target_os = "linux") => assert_eq!(mode, 0o1600, "{}", path.display()),
            false => assert_eq!(mode, 0o600, "{}", path.display()),
        }
    }
    for file in [l.settings_file(), l.settings_example(), l.user_id_file(), l.marker_file(), l.session_file()] {
        assert!(file.exists(), "{}", file.display());
    }
}

#[test]
fn a_second_start_writes_nothing() {
    let s = Scratch::new("second");
    s.quiet_settings();
    assert!(s.run(&["wallet", "list"]).status.success(), "the first start");
    let after_first = tree(&s.root(), &LOCK_FILES, Folders::ButTimes);
    let out = s.run(&["wallet", "list"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let after_second = tree(&s.root(), &LOCK_FILES, Folders::ButTimes);
    assert_eq!(differences(&after_first, &after_second), Vec::<String>::new(), "only the lock files change");
}

#[test]
fn a_legacy_root_is_moved_not_shadowed() {
    let s = Scratch::new("legacy");
    legacy_root(&s);
    let out = s.run(&["wallet", "list"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let l = s.layout();
    let root = s.root();
    assert!(!root.join("settings.conf").exists(), "no settings.conf beside settings.toml");
    assert!(!root.join("workspace.json").exists());
    assert!(root.join("MIGRATED.txt").exists());
    assert_eq!(std::fs::read(l.user_id_file()).expect("moved"), b"5f1c2d3e4b5a69788796a5b4c3d2e1f0", "byte for byte");
    let settings = std::fs::read_to_string(l.settings_file()).expect("converted");
    assert!(settings.contains("observers = []"), "{settings}");
    assert!(text(&out.stderr).contains("your old profile at"), "{}", text(&out.stderr));
}

#[test]
fn trust_forget_touches_only_this_shard() {
    let s = Scratch::new("trust");
    s.quiet_settings();
    assert!(s.run(&["wallet", "list"]).status.success(), "a first start");
    let l = s.layout();
    std::fs::create_dir_all(l.trust_dir()).expect("trust/");
    std::fs::write(
        l.trust_file(),
        "# F1R3Gaze freshness records: shard, binding, highest finalized block seen\nroot\trho:id:a\t5\nroot\trho:id:b\t7\ntest\trho:id:a\t9\n",
    )
    .expect("records");
    let listed = s.run(&["trust", "list"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    assert!(text(&listed.stdout).contains("2 records of shard root"), "{}", text(&listed.stdout));
    let one = s.run(&["trust", "forget", "rho:id:a"]);
    assert!(one.status.success(), "{}", text(&one.stderr));
    assert!(text(&one.stdout).contains("forgot 1 records of shard root"), "{}", text(&one.stdout));
    let all = s.run(&["trust", "forget", "--all"]);
    assert!(text(&all.stdout).contains("forgot 1 records of shard root"), "{}", text(&all.stdout));
    let left = std::fs::read_to_string(l.trust_file()).expect("records");
    assert!(left.contains("test\trho:id:a\t9"), "another shard's records stay: {left}");
    assert!(!left.contains("root\t"), "{left}");
}

#[test]
fn backups_are_pruned_only_when_asked() {
    let s = Scratch::new("backups");
    s.quiet_settings();
    assert!(s.run(&["wallet", "list"]).status.success(), "a first start");
    let l = s.layout();
    let old = l.config.join("backups/2020-01-01T00-00-00Z/settings.toml");
    let recent = l.state.join("backups/2999-01-01T00-00-00Z/session.json");
    for file in [&old, &recent] {
        std::fs::create_dir_all(file.parent().expect("a session")).expect("a session");
        std::fs::write(file, "kept").expect("a backup");
    }
    let listed = s.run(&["profile", "backups"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    assert!(text(&listed.stdout).contains("2 sessions"), "{}", text(&listed.stdout));
    assert!(old.exists() && recent.exists(), "listing removes nothing");
    let pruned = s.run(&["profile", "backups", "prune", "--older-than", "30"]);
    assert!(pruned.status.success(), "{}", text(&pruned.stderr));
    assert!(text(&pruned.stdout).contains("1 sessions removed"), "{}", text(&pruned.stdout));
    assert!(!old.exists() && recent.exists());
}

#[test]
fn usage_errors_exit_2() {
    let s = Scratch::new("usage");
    for args in [&["--bogus"][..], &["trust"], &["--help"], &["--headless"], &["profile", "nothing"]] {
        let out = s.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("usage: f1r3gaze"), "{args:?}: {}", text(&out.stderr));
    }
    assert!(!s.root().exists(), "a usage error opens nothing");
}
