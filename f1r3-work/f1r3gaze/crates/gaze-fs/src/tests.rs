use super::*;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

// ── Crash enumeration ────────────────────────────────────────────────────

// The crash enumeration that was here ("runs `work` on copies of `start`,
// crashed after every possible number of operations, and for each crash
// also every power cut") moved to `crash.rs` (`crate::every_crash`), public
// for the crates that build on gaze-fs.

fn content(fs: &MemFs, path: &str) -> Option<Vec<u8>> {
    fs.files().get(Path::new(path)).cloned()
}

/// A file system with POSIX semantics and one with the NTFS semantics
/// `StdFs` relies on under Windows, each shaped by `shape`.
fn both(shape: impl Fn(MemFs) -> MemFs) -> [(&'static str, MemFs); 2] {
    [("posix", shape(MemFs::new())), ("ntfs", shape(MemFs::new().ntfs()))]
}

#[test]
fn write_atomic_survives_every_crash() {
    for (semantics, start) in both(|fs| fs) {
        start.seed_file("/p/settings.toml", b"old");
        let path = Path::new("/p/settings.toml");
        let checked = every_crash(
            &start,
            |fs| write_atomic(fs.as_ref(), path, b"new", Perm::Preserve),
            |when, fs| match content(fs, "/p/settings.toml") {
                Some(bytes) => {
                    assert!(bytes == b"old" || bytes == b"new", "{semantics}, {when}: {bytes:?}")
                }
                None => panic!("{semantics}, {when}: settings.toml is gone"),
            },
        );
        assert!(checked > 10, "{semantics}: only {checked} crash states");
    }
}

/// `replace_unsynced` is atomic for readers: a process that dies at any
/// point leaves the old content or the new one, whole. Nothing is synced,
/// so a power cut may also leave what the file system kept of the new
/// file's data ([`GARBAGE`] in `MemFs`), but never a missing file where
/// there was one (ledger S15, part 2, V1).
#[test]
fn replace_unsynced_is_atomic_for_readers() {
    for (semantics, start) in both(|fs| fs) {
        start.seed_file("/r/instance.pid", b"old");
        let path = Path::new("/r/instance.pid");
        let checked = every_crash(
            &start,
            |fs| replace_unsynced(fs.as_ref(), path, b"new", Perm::Private),
            |when, fs| {
                let bytes = content(fs, "/r/instance.pid");
                let whole = matches!(bytes.as_deref(), Some(b"old") | Some(b"new"));
                match &when.power {
                    None => assert!(whole, "{semantics}, {when}: {bytes:?}"),
                    Some(_) => assert!(whole || bytes.as_deref() == Some(GARBAGE), "{semantics}, {when}: {bytes:?}"),
                }
            },
        );
        assert!(checked > 3, "{semantics}: only {checked} crash states");
    }
}

/// It replaces the file with the mode asked for, and syncs nothing.
#[test]
fn replace_unsynced_syncs_nothing() {
    let fs = std::sync::Arc::new(MemFs::new());
    fs.seed_file("/r/instance.pid", b"old");
    let trace = TraceFs::new(fs.clone());
    replace_unsynced(&trace, Path::new("/r/instance.pid"), b"new", Perm::Private).expect("replaced");
    let log = trace.log();
    assert!(log.iter().all(|t| t.op != "sync_file" && t.op != "sync_dir"), "{log:#?}");
    assert!(log.iter().any(|t| t.op == "rename" && t.to.as_deref() == Some(Path::new("/r/instance.pid"))), "{log:#?}");
    assert_eq!(content(&fs, "/r/instance.pid").as_deref(), Some(&b"new"[..]));
    assert_eq!(fs.mode(Path::new("/r/instance.pid")).expect("a mode"), Some(PRIVATE_FILE_MODE));
}

/// Once a write returns, a power cut cannot undo it: the caller may act on
/// it, as migration does when it removes a source after publishing its
/// copy.
#[test]
fn completed_writes_survive_any_power_cut() {
    for (semantics, fs) in both(|fs| fs) {
        fs.seed_file("/p/settings.toml", b"old");
        fs.seed_file("/legacy/user-id", b"id");
        fs.seed_dir("/new");
        write_atomic(&fs, Path::new("/p/settings.toml"), b"new", Perm::Preserve).expect("write");
        publish_no_replace(&fs, Path::new("/legacy/user-id"), Path::new("/new/user-id"))
            .expect("publish");
        // The weakest power cut: nothing pending survives.
        fs.power_cut(|_| false);
        assert_eq!(content(&fs, "/p/settings.toml").as_deref(), Some(&b"new"[..]), "{semantics}");
        assert_eq!(content(&fs, "/new/user-id").as_deref(), Some(&b"id"[..]), "{semantics}");
        if semantics == "posix" {
            // Under NTFS the removal is committed by the next file sync on
            // the volume; under POSIX the publish synced it.
            assert_eq!(content(&fs, "/legacy/user-id"), None, "the old name is durably gone");
        }
    }
}

#[test]
fn a_new_file_is_absent_or_complete_after_any_crash() {
    for (semantics, start) in both(|fs| fs) {
        start.seed_dir("/p");
        let path = Path::new("/p/session.json");
        every_crash(
            &start,
            |fs| write_atomic(fs.as_ref(), path, b"{}", Perm::Private),
            |when, fs| {
                if let Some(bytes) = content(fs, "/p/session.json") {
                    assert_eq!(bytes, b"{}", "{semantics}, {when}");
                }
            },
        );
    }
}

#[test]
fn publishing_loses_nothing_and_shows_nothing_partial() {
    for (semantics, start) in both(|fs| fs) {
        start.seed_file("/legacy/user-id", b"0123");
        start.seed_dir("/new");
        every_crash(
            &start,
            |fs| publish_no_replace(fs.as_ref(), Path::new("/legacy/user-id"), Path::new("/new/user-id")),
            |when, fs| {
                let (old, new) = (content(fs, "/legacy/user-id"), content(fs, "/new/user-id"));
                assert!(
                    old.as_deref() == Some(b"0123") || new.as_deref() == Some(b"0123"),
                    "{semantics}, {when}: the content is lost ({old:?}, {new:?})"
                );
                assert!(matches!(new.as_deref(), None | Some(b"0123")), "{semantics}, {when}: {new:?}");
            },
        );
    }
}

#[test]
fn publishing_without_hard_links_loses_nothing() {
    for (semantics, start) in both(MemFs::without_hard_links) {
        start.seed_file("/legacy/grants.tsv", b"g");
        start.seed_dir("/new");
        every_crash(
            &start,
            |fs| publish_no_replace(fs.as_ref(), Path::new("/legacy/grants.tsv"), Path::new("/new/grants.tsv")),
            |when, fs| {
                let (old, new) = (content(fs, "/legacy/grants.tsv"), content(fs, "/new/grants.tsv"));
                assert!(old.is_some() || new.is_some(), "{semantics}, {when}: the content is lost");
                assert!(matches!(new.as_deref(), None | Some(b"g")), "{semantics}, {when}: {new:?}");
            },
        );
    }
}

#[test]
fn moving_across_file_systems_loses_nothing() {
    for (semantics, start) in both(|fs| fs.with_device("/other")) {
        start.seed_file("/legacy/keys/a.key", b"secret");
        start.seed_dir("/other/data/keys");
        every_crash(
            &start,
            |fs| move_no_replace(fs.as_ref(), Path::new("/legacy/keys/a.key"), Path::new("/other/data/keys/a.key")),
            |when, fs| {
                let (old, new) =
                    (content(fs, "/legacy/keys/a.key"), content(fs, "/other/data/keys/a.key"));
                assert!(
                    old.as_deref() == Some(b"secret") || new.as_deref() == Some(b"secret"),
                    "{semantics}, {when}: the key is lost ({old:?}, {new:?})"
                );
                assert!(matches!(new.as_deref(), None | Some(b"secret")), "{semantics}, {when}: {new:?}");
            },
        );
    }
}

#[test]
fn a_verified_copy_leaves_its_source_alone() {
    for (semantics, start) in both(|fs| fs.with_device("/other")) {
        start.seed_file("/a/store.gzs", b"records");
        start.seed_dir("/other/b");
        every_crash(
            &start,
            |fs| copy_verified(fs.as_ref(), Path::new("/a/store.gzs"), Path::new("/other/b/store.gzs")),
            |when, fs| {
                assert_eq!(
                    content(fs, "/a/store.gzs").as_deref(),
                    Some(&b"records"[..]),
                    "{semantics}, {when}"
                );
                let copy = content(fs, "/other/b/store.gzs");
                assert!(matches!(copy.as_deref(), None | Some(b"records")), "{semantics}, {when}: {copy:?}");
            },
        );
    }
}

#[test]
fn a_copy_that_reads_back_wrong_is_never_published() {
    for (semantics, fs) in both(|fs| fs.with_device("/other").with_faulty_copies()) {
        fs.seed_file("/legacy/keys/a.key", b"secret");
        fs.seed_dir("/other/keys");
        let refused =
            move_no_replace(&fs, Path::new("/legacy/keys/a.key"), Path::new("/other/keys/a.key"))
                .expect_err("a copy that differs is refused");
        assert!(refused.to_string().contains("did not read back the same"), "{semantics}: {refused}");
        assert_eq!(content(&fs, "/legacy/keys/a.key").as_deref(), Some(&b"secret"[..]));
        assert!(fs.files().keys().all(|path| !path.starts_with("/other")), "{:?}", fs.files());
        if semantics == "posix" {
            assert_eq!(fs.pending_len(), 0, "the discarded copy's removal is durable");
        }
    }
}

#[test]
fn nothing_is_ever_replaced() {
    for (_, fs) in both(|fs| fs)
        .into_iter()
        .chain(both(MemFs::without_hard_links))
        .chain(both(|fs| fs.with_device("/other")))
    {
        fs.seed_file("/legacy/user-id", b"ours");
        fs.seed_file("/new/user-id", b"theirs");
        fs.seed_file("/other/user-id", b"theirs");
        for to in ["/new/user-id", "/other/user-id"] {
            let refused = move_no_replace(&fs, Path::new("/legacy/user-id"), Path::new(to))
                .expect_err("an existing destination is refused");
            assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists, "{to}: {refused}");
            assert_eq!(content(&fs, to).as_deref(), Some(&b"theirs"[..]));
            assert_eq!(content(&fs, "/legacy/user-id").as_deref(), Some(&b"ours"[..]));
        }
    }
}

/// The crash enumeration sees what it is meant to: a publish that skips the
/// file sync leaves garbage under a POSIX power cut, and never under a
/// process crash alone (the model's controls). Under NTFS semantics the
/// rename itself syncs the file it names (W2), so no garbage appears.
#[test]
fn the_crash_enumeration_catches_a_missing_fsync() {
    fn publish_unsynced(fs: &dyn Fs, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let temp = temp_path(path)?;
        fs.create_new(&temp, bytes, PRIVATE_FILE_MODE)?;
        fs.rename(&temp, path)?;
        fs.sync_dir(parent(path)?)
    }
    for (semantics, start) in both(|fs| fs) {
        start.seed_file("/p/history.json", b"old");
        let path = Path::new("/p/history.json");
        // The checks run on every core, so they count with atomics.
        let (after_power_cut, after_process_crash) = (AtomicUsize::new(0), AtomicUsize::new(0));
        every_crash(
            &start,
            |fs| publish_unsynced(fs.as_ref(), path, b"new"),
            |when, fs| {
                if content(fs, "/p/history.json").as_deref() == Some(GARBAGE) {
                    match when.power {
                        Some(_) => after_power_cut.fetch_add(1, Ordering::Relaxed),
                        None => after_process_crash.fetch_add(1, Ordering::Relaxed),
                    };
                }
            },
        );
        let (after_power_cut, after_process_crash) = (after_power_cut.into_inner(), after_process_crash.into_inner());
        assert_eq!(after_process_crash, 0, "{semantics}");
        match semantics {
            "posix" => assert!(after_power_cut > 0, "the enumeration missed the missing fsync"),
            _ => assert_eq!(after_power_cut, 0, "{semantics}: naming a file syncs it"),
        }
    }
}

// ── MemFs itself ─────────────────────────────────────────────────────────

#[test]
fn a_power_cut_keeps_only_what_was_synced() {
    let fs = MemFs::new();
    fs.seed_dir("/d");
    fs.create_new(Path::new("/d/a"), b"a", PRIVATE_FILE_MODE).expect("create a");
    fs.sync_file(Path::new("/d/a")).expect("sync a");
    fs.create_new(Path::new("/d/b"), b"b", PRIVATE_FILE_MODE).expect("create b");
    assert_eq!(fs.pending_len(), 2);
    fs.power_cut(|_| false);
    assert!(fs.files().is_empty(), "no name was durable");
    fs.create_new(Path::new("/d/a"), b"a", PRIVATE_FILE_MODE).expect("create a again");
    fs.create_new(Path::new("/d/b"), b"b", PRIVATE_FILE_MODE).expect("create b again");
    fs.sync_file(Path::new("/d/a")).expect("sync a");
    fs.sync_dir(Path::new("/d")).expect("sync d");
    assert_eq!(fs.pending_len(), 0);
    fs.power_cut(|_| true);
    assert_eq!(content(&fs, "/d/a").as_deref(), Some(&b"a"[..]));
    assert_eq!(content(&fs, "/d/b").as_deref(), Some(GARBAGE), "b's data was never synced");
}

#[test]
fn syncing_one_directory_leaves_the_others_pending() {
    let fs = MemFs::new();
    fs.seed_dir("/x");
    fs.seed_dir("/y");
    fs.create_new(Path::new("/x/f"), b"f", PRIVATE_FILE_MODE).expect("create x/f");
    fs.create_new(Path::new("/y/g"), b"g", PRIVATE_FILE_MODE).expect("create y/g");
    fs.sync_dir(Path::new("/x")).expect("sync x");
    assert_eq!(fs.pending_len(), 1);
    fs.power_cut(|_| false);
    assert!(content(&fs, "/x/f").is_some());
    assert!(content(&fs, "/y/g").is_none());
}

#[test]
fn renaming_a_directory_moves_everything_below_it() {
    let fs = MemFs::new();
    fs.seed_file("/cache/ab/abcd", b"blob");
    fs.seed_dir("/new");
    fs.rename(Path::new("/cache/ab"), Path::new("/new/ab")).expect("rename");
    assert_eq!(content(&fs, "/new/ab/abcd").as_deref(), Some(&b"blob"[..]));
    assert!(content(&fs, "/cache/ab/abcd").is_none());
    assert_eq!(fs.kind(Path::new("/new/ab")).expect("kind"), Kind::Dir);
}

#[test]
fn a_dead_process_changes_nothing_more() {
    let fs = MemFs::new();
    fs.seed_dir("/d");
    fs.fail_after(1);
    fs.create_new(Path::new("/d/a"), b"a", PRIVATE_FILE_MODE).expect("the first operation runs");
    let refused = fs.create_new(Path::new("/d/b"), b"b", PRIVATE_FILE_MODE);
    assert!(refused.is_err());
    fs.revive();
    assert!(content(&fs, "/d/a").is_some());
    assert!(content(&fs, "/d/b").is_none());
}

#[test]
fn creating_a_directory_claims_its_name() {
    let dir = scratch_dir("gaze-fs-claim");
    let claimed = dir.join("2026-10-05T12-00-00Z");
    StdFs.create_dir(&claimed).expect("the first claim");
    assert_eq!(StdFs.create_dir(&claimed).expect_err("taken").kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        StdFs.create_dir(&dir.join("missing/child")).expect_err("no parent").kind(),
        io::ErrorKind::NotFound
    );
    let fs = MemFs::new();
    fs.seed_dir("/b");
    fs.create_dir(Path::new("/b/s")).expect("claim");
    assert_eq!(fs.create_dir(Path::new("/b/s")).expect_err("taken").kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs.create_dir(Path::new("/x/y")).expect_err("no parent").kind(), io::ErrorKind::NotFound);
}

#[test]
fn links_across_file_systems_are_refused() {
    let fs = MemFs::new().with_device("/other");
    fs.seed_file("/a/f", b"f");
    fs.seed_dir("/other");
    let refused = fs.hard_link(Path::new("/a/f"), Path::new("/other/f")).expect_err("cross-device");
    assert_eq!(refused.kind(), io::ErrorKind::CrossesDevices);
}

// ── The real file system ────────────────────────────────────────────────

#[test]
fn atomic_writes_replace_the_whole_file_and_leave_no_temporary() {
    let dir = scratch_dir("gaze-fs-atomic");
    let path = dir.join("settings.toml");
    write_atomic(&StdFs, &path, b"first", Perm::Private).expect("create");
    write_atomic(&StdFs, &path, b"second, longer", Perm::Private).expect("replace");
    assert_eq!(std::fs::read(&path).expect("read"), b"second, longer");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .expect("list")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["settings.toml"]);
}

#[cfg(unix)]
#[test]
fn private_files_and_directories_are_owner_only_from_creation() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("gaze-fs-modes");
    let nested = dir.join("a/b");
    StdFs.create_dir_all(&nested).expect("create");
    for created in [dir.join("a"), nested.clone()] {
        let mode = std::fs::metadata(&created).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", created.display());
    }
    // An existing directory keeps its mode.
    std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    StdFs.create_dir_all(&nested).expect("again");
    let mode = std::fs::metadata(&nested).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    // A new file is 0600 before a byte is written.
    let file = nested.join("key");
    StdFs.create_new(&file, b"", PRIVATE_FILE_MODE).expect("create");
    let mode = std::fs::metadata(&file).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    write_atomic(&StdFs, &nested.join("wallets.tsv"), b"w", Perm::Private).expect("write");
    let mode = std::fs::metadata(nested.join("wallets.tsv")).expect("stat").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn shared_files_follow_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("gaze-fs-shared");
    let path = dir.join("manifest.rho");
    write_atomic(&StdFs, &path, b"site", Perm::Shared).expect("write");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    // Whatever the umask, the owner can read and write, and the group and
    // others get no more than read and write.
    assert_eq!(mode & 0o600, 0o600, "{mode:o}");
    assert_eq!(mode & 0o111, 0, "{mode:o}");
}

#[cfg(unix)]
#[test]
fn preserve_keeps_the_replaced_files_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("gaze-fs-preserve");
    let path = dir.join("settings.toml");
    std::fs::write(&path, b"old").expect("seed");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
    write_atomic(&StdFs, &path, b"new", Perm::Preserve).expect("replace");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
    // A read-only file is still replaced, keeping its mode.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).expect("chmod");
    write_atomic(&StdFs, &path, b"newer", Perm::Preserve).expect("replace read-only");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!((mode, std::fs::read(&path).expect("read")), (0o400, b"newer".to_vec()));
}

#[cfg(unix)]
#[test]
fn writes_go_through_symbolic_links_and_keep_them() {
    let dir = scratch_dir("gaze-fs-symlink");
    let target = dir.join("dotfiles/settings.toml");
    std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    std::fs::write(&target, b"old").expect("seed");
    let link = dir.join("settings.toml");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    write_atomic(&StdFs, &link, b"new", Perm::Preserve).expect("write");
    assert!(std::fs::symlink_metadata(&link).expect("lstat").file_type().is_symlink());
    assert_eq!(std::fs::read(&target).expect("read"), b"new");
}

#[test]
fn publishing_refuses_an_existing_name_and_keeps_both() {
    let dir = scratch_dir("gaze-fs-publish");
    let (from, to) = (dir.join("from"), dir.join("to"));
    std::fs::write(&from, b"ours").expect("seed");
    std::fs::write(&to, b"theirs").expect("seed");
    let refused = publish_no_replace(&StdFs, &from, &to).expect_err("refused");
    assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&to).expect("read"), b"theirs");
    assert_eq!(std::fs::read(&from).expect("read"), b"ours");
    std::fs::remove_file(&to).expect("free the name");
    publish_no_replace(&StdFs, &from, &to).expect("publish");
    assert_eq!(std::fs::read(&to).expect("read"), b"ours");
    assert!(!from.exists());
}

#[test]
fn verified_copies_compare_large_files_in_chunks() {
    let dir = scratch_dir("gaze-fs-copy");
    let from = dir.join("store.gzs");
    let mut bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&from, &bytes).expect("seed");
    copy_verified(&StdFs, &from, &dir.join("copy.gzs")).expect("copy");
    assert!(same_contents(&StdFs, &from, &dir.join("copy.gzs")).expect("compare"));
    *bytes.last_mut().expect("non-empty") ^= 1;
    std::fs::write(dir.join("other.gzs"), &bytes).expect("seed");
    assert!(!same_contents(&StdFs, &from, &dir.join("other.gzs")).expect("compare"));
    std::fs::write(dir.join("short.gzs"), &bytes[..1000]).expect("seed");
    assert!(!same_contents(&StdFs, &from, &dir.join("short.gzs")).expect("compare"));
}

#[test]
fn a_bare_file_name_lives_in_the_current_directory() {
    assert_eq!(parent(Path::new("app.knf")).expect("parent"), Path::new("."));
    assert_eq!(parent(Path::new("site/app.knf")).expect("parent"), Path::new("site"));
    assert!(parent(Path::new("/")).is_err());
    let temp = temp_path(Path::new("app.knf")).expect("temp");
    assert_eq!(temp.parent(), Some(Path::new(".")));
}

#[test]
fn temporary_names_carry_their_process() {
    let temp = temp_path(Path::new("/p/history.json")).expect("temp");
    let name = temp.file_name().expect("name");
    assert_eq!(temp_owner(name), Some(std::process::id()));
    assert!(name.to_string_lossy().starts_with(".history.json.tmp-"));
    for other in ["history.json", ".history.json", ".x.tmp-", ".x.tmp-12", ".x.tmp-a-1", "x.tmp-1-2"] {
        assert_eq!(temp_owner(std::ffi::OsStr::new(other)), None, "{other}");
    }
    assert_eq!(temp_owner(std::ffi::OsStr::new(".x.tmp-41-7")), Some(41));
}

#[test]
fn sweeping_removes_only_other_processes_temporary_files() {
    let fs = MemFs::new();
    fs.seed_file("/d/session.json", b"{}");
    fs.seed_file("/d/.session.json.tmp-1-0", b"half");
    let ours = temp_path(Path::new("/d/session.json")).expect("temp");
    fs.seed_file(&ours, b"mine");
    assert_eq!(sweep_temps(&fs, Path::new("/d")).expect("sweep"), 1);
    let names: Vec<PathBuf> = fs.files().into_keys().collect();
    let mut expected = vec![ours, PathBuf::from("/d/session.json")];
    expected.sort();
    assert_eq!(names, expected);
    assert_eq!(fs.pending_len(), 0, "the sweep was made durable");
}

// ── Lines and scratch directories ───────────────────────────────────────

#[test]
fn usable_lines_are_kept_byte_for_byte() {
    let bytes = b"good 1\r\n\nbad\n\xff\xfe\ngood 2";
    let check = check_lines(bytes, |line| match line.starts_with("good") {
        true => Ok(()),
        false => Err(format!("not good: {line}")),
    });
    assert_eq!(check.kept, b"good 1\r\n\ngood 2");
    assert_eq!(
        check.dropped,
        [(3, "not good: bad".to_string()), (4, "not UTF-8".to_string())]
    );
    assert!(!check.is_clean());
    assert!(check_lines(b"good\n", |_| Ok(())).is_clean());
}

#[test]
fn scratch_directories_prefer_tmpdir_then_the_target_directory() {
    let exe = Path::new("/work/target/debug/deps/gaze_fs-123");
    let is_target = |dir: &Path| dir == Path::new("/work/target");
    assert_eq!(
        scratch_base(Some(std::ffi::OsStr::new("/scratch/tmp")), exe, is_target),
        Path::new("/scratch/tmp/f1r3gaze-tests")
    );
    assert_eq!(
        scratch_base(Some(std::ffi::OsStr::new("relative")), exe, is_target),
        Path::new("/work/target/scratch")
    );
    assert_eq!(scratch_base(None, exe, is_target), Path::new("/work/target/scratch"));
    assert_eq!(
        scratch_base(None, exe, |_| false),
        Path::new("/work/target/debug/deps/scratch")
    );
}

#[test]
fn write_new_survives_every_crash() {
    for (semantics, start) in both(|fs| fs) {
        start.seed_dir("/p");
        let path = Path::new("/p/session.json");
        let checked = every_crash(
            &start,
            |fs| write_new(fs.as_ref(), path, b"{\"version\":1}", Perm::Private),
            |when, fs| {
                let files = fs.files();
                match files.get(path).map(Vec::as_slice) {
                    None | Some(b"{\"version\":1}") => {}
                    Some(other) => panic!("{semantics}, {when}: {other:?} at the new name"),
                }
            },
        );
        assert!(checked > 10, "{semantics}: {checked}");
        // A finished write is durable: a power cut that keeps nothing
        // pending keeps the file. (Under NTFS the temporary file's final
        // unlink may stay pending: a power cut can only bring the temporary
        // file back, and the next start sweeps it.)
        let done = MemFs::clone(&start);
        write_new(&done, path, b"{\"version\":1}", Perm::Private).expect("write");
        if semantics == "posix" {
            assert_eq!(done.pending_len(), 0, "{semantics}");
        }
        assert_eq!(done.mode(path).expect("mode"), Some(PRIVATE_FILE_MODE));
        let cut = MemFs::clone(&done);
        cut.power_cut(|_| false);
        assert_eq!(content(&cut, "/p/session.json").as_deref(), Some(&b"{\"version\":1}"[..]), "{semantics}");
    }
}

#[test]
fn write_new_never_replaces() {
    let fs = MemFs::new();
    fs.seed_file("/p/user-id", b"mine");
    let error = write_new(&fs, Path::new("/p/user-id"), b"another", Perm::Private).expect_err("taken");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(content(&fs, "/p/user-id").as_deref(), Some(&b"mine"[..]));
    let names: Vec<PathBuf> = fs.files().into_keys().collect();
    assert_eq!(names, [PathBuf::from("/p/user-id")], "no temporary file is left behind");
    // Without hard links, the name is checked before the rename.
    let fat = MemFs::new().without_hard_links();
    fat.seed_file("/p/user-id", b"mine");
    assert!(write_new(&fat, Path::new("/p/user-id"), b"another", Perm::Private).is_err());
    assert_eq!(content(&fat, "/p/user-id").as_deref(), Some(&b"mine"[..]));
}

#[test]
fn trace_fs_is_transparent() {
    let inner = std::sync::Arc::new(MemFs::new());
    inner.seed_file("/p/a", b"one");
    let traced = TraceFs::new(inner.clone());
    write_atomic(&traced, Path::new("/p/a"), b"two", Perm::Private).expect("write");
    traced.note("barrier-begin");
    assert_eq!(traced.read(Path::new("/p/a")).expect("read"), b"two");
    assert!(traced.read(Path::new("/p/missing")).is_err());
    assert_eq!(content(&inner, "/p/a").as_deref(), Some(&b"two"[..]), "every call reaches the file system");
    let log = traced.log();
    let ops: Vec<&str> = log.iter().map(|t| t.op).collect();
    assert_eq!(&ops[ops.len() - 3..], ["note", "read", "read"]);
    assert!(log.iter().any(|t| t.op == "create_new" && t.fnv == Some(fnv1a64(b"two"))));
    assert!(log.iter().any(|t| t.op == "rename" && t.to.as_deref() == Some(Path::new("/p/a"))));
    assert!(log.iter().all(|t| t.n == log.iter().position(|u| std::ptr::eq(u, t)).expect("in the log") as u64));
    let missing = log.last().expect("a record");
    assert_eq!(missing.error, Some(io::ErrorKind::NotFound));
    assert_eq!(missing.json(), format!("{{\"n\":{},\"op\":\"read\",\"path\":\"/p/missing\",\"ok\":false,\"err\":\"NotFound\"}}", missing.n));
    let note = log.iter().find(|t| t.op == "note").expect("the note");
    assert_eq!(note.json(), format!("{{\"n\":{},\"op\":\"note\",\"what\":\"barrier-begin\"}}", note.n));
    assert!(log.iter().filter(|t| t.writes()).all(|t| t.op != "read"));
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325, "FNV-1a's offset basis");
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn two_crashes_are_enumerated() {
    // write_atomic twice in a row, crashed twice: the file always holds one
    // whole version.
    let start = MemFs::new();
    start.seed_file("/p/window.json", b"0");
    let path = Path::new("/p/window.json");
    let checked = every_two_crashes(
        &start,
        |_| {},
        |fs| write_atomic(fs.as_ref(), path, b"1", Perm::Private),
        |when, fs| match content(fs, "/p/window.json").as_deref() {
            Some(b"0") | Some(b"1") => {}
            other => panic!("{when}: {other:?}"),
        },
    );
    assert!(checked > 50, "{checked}");
}

#[test]
fn new_folders_are_durable_and_existing_ones_cost_nothing() {
    let fs = MemFs::new();
    fs.seed_dir("/home/u");
    let data = Path::new("/home/u/.local/share/f1r3fly-io/f1r3gaze/data");
    let created = create_dir_durably(&fs, data).expect("the folders are created");
    let expected: Vec<PathBuf> = [
        "/home/u/.local",
        "/home/u/.local/share",
        "/home/u/.local/share/f1r3fly-io",
        "/home/u/.local/share/f1r3fly-io/f1r3gaze",
        "/home/u/.local/share/f1r3fly-io/f1r3gaze/data",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(created, expected);
    // Every new name is durable: whatever a power cut keeps, they are there.
    assert_eq!(fs.pending_len(), 0);
    fs.power_cut(|_| false);
    assert!(fs.dirs().iter().any(|d| d == data));
    // A folder that exists costs one look, and nothing is synced.
    fs.reset_ops();
    assert_eq!(create_dir_durably(&fs, data).expect("it exists"), Vec::<PathBuf>::new());
    assert_eq!(fs.ops(), 1);
    // Something else in the way is an error, and nothing is created.
    fs.seed_file("/home/u/blocked", b"x");
    let error = create_dir_durably(&fs, Path::new("/home/u/blocked/data")).expect_err("a file is in the way");
    assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
}

/// MemFs follows a link only as a path's last name, so links inside a path
/// are checked on the real file system.
#[cfg(unix)]
#[test]
fn a_linked_folder_is_used_as_the_folder() {
    let dir = scratch_dir("gaze-fs-linked-folder");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("elsewhere")).expect("scratch");
    std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("linked")).expect("link");
    let created = create_dir_durably(&StdFs, &dir.join("linked/data/wallet")).expect("through the link");
    assert_eq!(created, [dir.join("linked/data"), dir.join("linked/data/wallet")]);
    assert!(dir.join("elsewhere/data/wallet").is_dir());
    assert!(std::fs::symlink_metadata(dir.join("linked")).expect("the link").file_type().is_symlink());
    assert_eq!(create_dir_durably(&StdFs, &dir.join("linked")).expect("the link itself"), Vec::<PathBuf>::new());
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn sweeping_one_name_leaves_every_other_file() {
    let fs = MemFs::new();
    let other = std::process::id().wrapping_add(1);
    let own = std::process::id();
    let dead = format!("/d/.settings.toml.tmp-{other}-3");
    let ours = format!("/d/.settings.toml.tmp-{own}-3");
    let elsewhere = format!("/d/.vimrc.tmp-{other}-3");
    let similar = format!("/d/.settings.toml.bak.tmp-{other}-3");
    for path in [&dead, &ours, &elsewhere, &similar] {
        fs.seed_file(path, b"x");
    }
    fs.seed_file("/d/settings.toml", b"[appearance]\n");
    let removed = sweep_temps_of(&fs, Path::new("/d"), std::ffi::OsStr::new("settings.toml")).expect("the sweep");
    assert_eq!(removed, 1);
    let files = fs.files();
    assert!(!files.contains_key(Path::new(&dead)));
    for kept in [&ours, &elsewhere, &similar] {
        assert!(files.contains_key(Path::new(kept)), "{kept} is kept");
    }
    assert_eq!(fs.pending_len(), 0, "the removal is synced");
    assert_eq!(sweep_temps_of(&fs, Path::new("/d"), std::ffi::OsStr::new("settings.toml")).expect("again"), 0);
}

/// A session that only reads lists what the sweeps would remove, and the
/// sweeps then remove exactly that.
#[test]
fn stale_temporary_files_are_listed_without_removing_them() {
    let fs = std::sync::Arc::new(MemFs::new());
    let other = std::process::id().wrapping_add(1);
    let own = std::process::id();
    let dead = PathBuf::from(format!("/d/.settings.toml.tmp-{other}-3"));
    let dead_too = PathBuf::from(format!("/d/.vimrc.tmp-{other}-4"));
    let ours = PathBuf::from(format!("/d/.settings.toml.tmp-{own}-3"));
    let folder = PathBuf::from(format!("/d/.cache.tmp-{other}-5"));
    for path in [&dead, &dead_too, &ours] {
        fs.seed_file(path, b"x");
    }
    fs.seed_dir(&folder);
    fs.seed_file("/d/settings.toml", b"[appearance]\n");
    let trace = TraceFs::new(fs.clone());
    let all = stale_temps(&trace, Path::new("/d")).expect("listed");
    assert_eq!(all, [dead.clone(), dead_too.clone()], "other processes' files only, sorted");
    let named = stale_temps_of(&trace, Path::new("/d"), std::ffi::OsStr::new("settings.toml")).expect("listed");
    assert_eq!(named, std::slice::from_ref(&dead));
    assert!(trace.log().iter().all(|t| !t.writes()), "listing writes nothing: {:#?}", trace.log());
    assert_eq!(sweep_temps_of(fs.as_ref(), Path::new("/d"), std::ffi::OsStr::new("settings.toml")).expect("swept"), named.len());
    assert_eq!(sweep_temps(fs.as_ref(), Path::new("/d")).expect("swept"), all.len() - named.len());
    let files = fs.files();
    assert!(!files.contains_key(&dead) && !files.contains_key(&dead_too));
    assert!(files.contains_key(&ours), "this process's own file stays");
    assert!(fs.dirs().contains(&folder), "a folder is never swept");
}

#[test]
fn a_power_cut_never_keeps_a_folders_entries_without_the_folder() {
    let fs = MemFs::new();
    fs.seed_dir("/home/u");
    fs.create_dir_all(Path::new("/home/u/a/b")).expect("two folders");
    fs.create_new(Path::new("/home/u/a/b/f"), b"x", PRIVATE_FILE_MODE).expect("a file");
    assert_eq!(fs.pending(), ["+/home/u/a (sync /home/u)", "+/home/u/a/b (sync /home/u/a)", "+/home/u/a/b/f (sync /home/u/a/b)"]);
    // Keep the entries of a and b, but not a itself: nothing below a is
    // reachable, so nothing below it survives.
    fs.power_cut(|i| i != 0);
    assert_eq!(fs.dirs(), [PathBuf::from("/"), PathBuf::from("/home"), PathBuf::from("/home/u")]);
    assert!(fs.files().is_empty());
}

#[test]
fn every_crash_point_is_checked_once_on_every_core() {
    // A work of n operations, n more than twice the cores, so every core
    // takes several points: each crash point 0..=n must be checked exactly
    // once as a process crash, whichever core takes it.
    let cores = std::thread::available_parallelism().map_or(1, |c| c.get());
    let n = (4 * cores).max(400);
    let start = MemFs::new();
    start.seed_dir("/p");
    let seen = std::sync::Mutex::new(Vec::new());
    let checked = every_crash(
        &start,
        |fs| (0..n).try_for_each(|_| fs.kind(Path::new("/p")).map(drop)),
        |when, _| {
            if when.power.is_none() {
                seen.lock().expect("not poisoned").push(when.after[0]);
            }
        },
    );
    let mut seen = seen.into_inner().expect("not poisoned");
    seen.sort_unstable();
    let expected: Vec<(usize, usize)> = (0..=n).map(|k| (k, n)).collect();
    assert_eq!(seen, expected);
    // Reads leave nothing pending, so each point has one power cut too
    // (keeping the empty set): two states per point.
    assert_eq!(checked, 2 * (n + 1));
}

#[test]
fn a_simulated_process_has_its_own_temporary_files() {
    let fs = MemFs::new();
    fs.seed_dir("/d");
    // A write by "process 7" that died: its temporary file stays.
    let temp = with_process_id(7, || {
        let temp = temp_path(Path::new("/d/x")).expect("a name");
        fs.create_new(&temp, b"half", PRIVATE_FILE_MODE).expect("created");
        temp
    });
    assert_eq!(temp_owner(temp.file_name().expect("a name")), Some(7));
    // Process 7 itself does not sweep it; process 8, its restart, does.
    assert_eq!(with_process_id(7, || sweep_temps(&fs, Path::new("/d"))).expect("sweep"), 0);
    assert_eq!(with_process_id(8, || sweep_temps(&fs, Path::new("/d"))).expect("sweep"), 1);
    // Outside the simulation, the real process id is used again.
    assert_eq!(process_id(), std::process::id());
}
