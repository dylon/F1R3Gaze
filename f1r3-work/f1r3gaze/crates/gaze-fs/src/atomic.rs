//! Writes built from the operations of [`Fs`], so that a crash at any point,
//! including a power cut, leaves either the old content or the new one at a
//! name, never a mix and never nothing where something was.
//!
//! Each sequence is the one the start-up model checks
//! (docs/storage/tla/ProfileStartup.tla):
//! - a file's data is synced before its name is published (fault
//!   `no-fsync-before-publish`);
//! - a new name is made durable, by syncing its directory, before an old
//!   name for the same data is removed (fault `no-dir-fsync-before-unlink`);
//! - nothing is ever replaced that the caller did not mean to replace
//!   (fault `replace-destination`).

use crate::fs::{Fs, Kind, PRIVATE_FILE_MODE};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The permissions [`write_atomic`] gives a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perm {
    /// Owner read and write only (0600 on Unix), from the moment the file
    /// exists, so a secret is never briefly readable by others.
    Private,
    /// The mode of the file being replaced, so a user's own choice survives
    /// an edit F1R3Gaze makes. A new file gets 0600.
    Preserve,
    /// Readable by everyone, as the user's umask allows (0666 minus the
    /// umask, like `std::fs::write`): for files made to be published, such
    /// as `f1r3c`'s site output, never for anything in the profile.
    Shared,
}

/// What [`Perm::Shared`] asks for; the process's umask takes its bits away.
const SHARED_FILE_MODE: u32 = 0o666;

/// Symbolic links followed by [`resolve_link`] before giving up.
const MAX_LINKS: usize = 40;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The process id that temporary names carry, and that tells this process's
/// temporary files from those of writers that died. Tests that simulate a
/// restart within one process give each simulated process its own id with
/// [`with_process_id`].
pub fn process_id() -> u32 {
    #[cfg(any(test, feature = "testing"))]
    if let Some(id) = SIMULATED_PROCESS.with(std::cell::Cell::get) {
        return id;
    }
    std::process::id()
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static SIMULATED_PROCESS: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// Runs `work` on this thread as if this process's id were `id`: a restart
/// simulated within one test process is another process, whose temporary
/// files the next start sweeps, as a real restart's would be.
#[cfg(any(test, feature = "testing"))]
pub fn with_process_id<T>(id: u32, work: impl FnOnce() -> T) -> T {
    let before = SIMULATED_PROCESS.with(|cell| cell.replace(Some(id)));
    struct Restore(Option<u32>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SIMULATED_PROCESS.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(before);
    work()
}

/// The directory holding `path`: the current directory for a bare file name
/// such as `app.knf`.
pub fn parent(path: &Path) -> io::Result<&Path> {
    match path.parent() {
        Some(dir) if dir.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(dir) => Ok(dir),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )),
    }
}

/// A fresh temporary name next to `path`: `.<name>.tmp-<pid>-<n>`. The pid
/// lets a later start tell a crashed process's temporary files from its
/// own ([`temp_owner`]).
pub fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut temp = std::ffi::OsString::with_capacity(name.len() + 32);
    temp.push(".");
    temp.push(name);
    temp.push(format!(".tmp-{}-{n}", process_id()));
    Ok(parent(path)?.join(temp))
}

/// The pid in a temporary name that [`temp_path`] made, if `name` is one.
pub fn temp_owner(name: &OsStr) -> Option<u32> {
    let rest = name.to_str()?.strip_prefix('.')?;
    let (_, tail) = rest.rsplit_once(".tmp-")?;
    let (pid, n) = tail.split_once('-')?;
    n.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// The file a write to `path` changes: `path` itself, or, if it is a
/// symbolic link, what the chain of links ends at. Writing through a link
/// keeps the link, which is how dotfile managers link `settings.toml` into
/// place.
pub fn resolve_link(fs: &dyn Fs, path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs.kind(&current) {
            Ok(Kind::Symlink) => {
                let target = fs.read_link(&current)?;
                current = match target.is_absolute() {
                    true => target,
                    false => parent(&current)?.join(target),
                };
            }
            Ok(_) => return Ok(current),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "{}: more than {MAX_LINKS} levels of symbolic links",
        path.display()
    )))
}

/// Replaces the content of `path` (or creates it) atomically and durably:
/// a temporary file in the same directory, written and synced, renamed over
/// `path`, then the directory synced. A crash at any point leaves the old
/// content or the new one. On an error the temporary file is removed.
pub fn write_atomic(fs: &dyn Fs, path: &Path, bytes: &[u8], perm: Perm) -> io::Result<()> {
    let target = resolve_link(fs, path)?;
    let dir = parent(&target)?;
    let mode = match perm {
        Perm::Private => PRIVATE_FILE_MODE,
        Perm::Shared => SHARED_FILE_MODE,
        Perm::Preserve => match fs.mode(&target) {
            Ok(Some(mode)) => mode,
            Ok(None) => PRIVATE_FILE_MODE,
            Err(e) if e.kind() == io::ErrorKind::NotFound => PRIVATE_FILE_MODE,
            Err(e) => return Err(e),
        },
    };
    let temp = temp_path(&target)?;
    let written = fs
        .create_new(&temp, bytes, mode)
        .and_then(|()| fs.sync_file(&temp))
        .and_then(|()| fs.rename(&temp, &target));
    if let Err(e) = written {
        discard(fs, &temp);
        return Err(e);
    }
    fs.sync_dir(dir)
}

/// Replaces the content of `path` (or creates it) atomically for readers,
/// without making it durable: a temporary file in the same directory,
/// renamed over `path`, and nothing synced. A reader never sees part of
/// the bytes, and a process that dies at any point leaves the old content
/// or the new one; after a power cut `path` may hold the old content, the
/// new, or whatever the file system kept of the new file's data. For a
/// record that any crash makes meaningless, such as who holds the instance
/// lock, the syncs of [`write_atomic`] cost time and buy nothing (ledger
/// S15, part 2, V1). On an error the temporary file is removed.
pub fn replace_unsynced(fs: &dyn Fs, path: &Path, bytes: &[u8], perm: Perm) -> io::Result<()> {
    let target = resolve_link(fs, path)?;
    let mode = match perm {
        Perm::Private => PRIVATE_FILE_MODE,
        Perm::Shared => SHARED_FILE_MODE,
        Perm::Preserve => match fs.mode(&target) {
            Ok(Some(mode)) => mode,
            Ok(None) => PRIVATE_FILE_MODE,
            Err(e) if e.kind() == io::ErrorKind::NotFound => PRIVATE_FILE_MODE,
            Err(e) => return Err(e),
        },
    };
    let temp = temp_path(&target)?;
    let written = fs.create_new(&temp, bytes, mode).and_then(|()| fs.rename(&temp, &target));
    if let Err(e) = written {
        discard(fs, &temp);
        return Err(e);
    }
    Ok(())
}

/// Writes a new file at `path`, which must not exist: the bytes go into a
/// temporary file next to it, created with the final mode, synced, and read
/// back, which is then published under `path` without replacing anything
/// ([`publish_no_replace`]). A crash leaves either no file at `path` or the
/// whole file, and a file that appears at `path` meanwhile is never
/// overwritten (`AlreadyExists`). `Preserve` means `Private` here: there is
/// no file whose mode to keep.
pub fn write_new(fs: &dyn Fs, path: &Path, bytes: &[u8], perm: Perm) -> io::Result<()> {
    let mode = match perm {
        Perm::Private | Perm::Preserve => PRIVATE_FILE_MODE,
        Perm::Shared => SHARED_FILE_MODE,
    };
    let temp = temp_path(path)?;
    let written = fs
        .create_new(&temp, bytes, mode)
        .and_then(|()| fs.sync_file(&temp))
        .and_then(|()| match fs.read(&temp)? == bytes {
            true => Ok(()),
            false => Err(io::Error::other(format!("{} did not read back the same", temp.display()))),
        })
        .and_then(|()| publish_no_replace(fs, &temp, path));
    if let Err(e) = written {
        discard(fs, &temp);
        return Err(e);
    }
    Ok(())
}

/// Gives the file at `from` the name `to`, which must be free, then removes
/// the name `from`. Nothing at `to` is ever replaced: `AlreadyExists`.
///
/// With hard links the steps are: link `to`; sync `to`'s directory; unlink
/// `from`; sync `from`'s directory. A crash in between leaves both names
/// on one file, which a later start recognises (the same content at both)
/// and finishes. Where the file system has no hard links (FAT, exFAT) it
/// checks that `to` is free and renames; the instance lock keeps every
/// other F1R3Gaze process out of the gap between the check and the rename.
/// Across file systems it fails with `CrossesDevices`; [`move_no_replace`]
/// then copies.
pub fn publish_no_replace(fs: &dyn Fs, from: &Path, to: &Path) -> io::Result<()> {
    let from_dir = parent(from)?;
    let to_dir = parent(to)?;
    match fs.hard_link(from, to) {
        Ok(()) => {
            fs.sync_dir(to_dir)?;
            fs.remove_file(from)?;
            fs.sync_dir(from_dir)
        }
        Err(e) if links_unsupported(&e) => {
            match fs.kind(to) {
                Ok(_) => return Err(already_exists(to)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            fs.rename(from, to)?;
            fs.sync_dir(to_dir)?;
            match from_dir == to_dir {
                true => Ok(()),
                false => fs.sync_dir(from_dir),
            }
        }
        Err(e) => Err(e),
    }
}

/// link(2) fails with `EPERM` on file systems without hard links (Linux
/// vfat, exFAT), `ENOTSUP` on some others, and `ERROR_INVALID_FUNCTION` on
/// Windows FAT volumes. A real permission problem also reports
/// `PermissionDenied`; the rename that follows then fails the same way.
fn links_unsupported(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
    )
}

fn already_exists(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} already exists", path.display()),
    )
}

/// Copies `from` to the free name `to`: into a private temporary file next
/// to `to`, synced, compared with `from` byte for byte, then published
/// with [`publish_no_replace`]. `from` is left as it is. On an error the
/// temporary file is removed.
pub fn copy_verified(fs: &dyn Fs, from: &Path, to: &Path) -> io::Result<()> {
    let temp = temp_path(to)?;
    let copied = fs
        .copy_new(from, &temp, PRIVATE_FILE_MODE)
        .and_then(|()| fs.sync_file(&temp))
        .and_then(|()| match same_contents(fs, &temp, from)? {
            true => Ok(()),
            false => Err(io::Error::other(format!(
                "the copy of {} did not read back the same",
                from.display()
            ))),
        })
        .and_then(|()| publish_no_replace(fs, &temp, to));
    if let Err(e) = copied {
        discard(fs, &temp);
        return Err(e);
    }
    Ok(())
}

/// Moves the file at `from` to the free name `to`, on the same file system
/// with [`publish_no_replace`], across file systems by [`copy_verified`]
/// followed by removing `from`. Nothing at `to` is ever replaced.
pub fn move_no_replace(fs: &dyn Fs, from: &Path, to: &Path) -> io::Result<()> {
    match publish_no_replace(fs, from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_verified(fs, from, to)?;
            fs.remove_file(from)?;
            fs.sync_dir(parent(from)?)
        }
        other => other,
    }
}

/// Whether two files hold the same bytes.
pub fn same_contents(fs: &dyn Fs, a: &Path, b: &Path) -> io::Result<bool> {
    fs.same_contents(a, b)
}

/// Creates the directory `dir` and its missing parents (0700 on Unix;
/// directories that exist are left as they are), then syncs the parent of
/// each directory it created, oldest first, so every new name is durable
/// before anything is published inside it. When `dir` exists already, it
/// only looks: no directory is created or synced. Returns the directories
/// created, parents first.
pub fn create_dir_durably(fs: &dyn Fs, dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut missing = Vec::with_capacity(4);
    for ancestor in dir.ancestors() {
        match fs.kind(ancestor) {
            Ok(Kind::Dir) => break,
            // A link to a folder is followed by create_dir_all as by every
            // other use of the path.
            Ok(Kind::Symlink) if fs.list(ancestor).is_ok() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    format!("{} is in the way of a folder", ancestor.display()),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push(ancestor.to_path_buf()),
            Err(e) => return Err(e),
        }
    }
    if missing.is_empty() {
        return Ok(missing);
    }
    // The one place folders are made: each new one's parent is synced
    // right below.
    #[allow(clippy::disallowed_methods)]
    fs.create_dir_all(dir)?;
    missing.reverse();
    for created in &missing {
        fs.sync_dir(parent(created)?)?;
    }
    Ok(missing)
}

/// Removes the temporary files that other processes' writes left in `dir`
/// (a crash between creating a temporary file and publishing it), then
/// syncs `dir` if it removed any, so that start-up reaches ready with
/// nothing pending (the model's `ReadyIsDurable`). This process's own
/// temporary files are left alone. Returns how many were removed.
pub fn sweep_temps(fs: &dyn Fs, dir: &Path) -> io::Result<usize> {
    let own = process_id();
    let mut removed = 0;
    for name in fs.list(dir)? {
        match temp_owner(&name) {
            Some(pid) if pid != own => {
                let path = dir.join(&name);
                if fs.kind(&path)? == Kind::File {
                    fs.remove_file(&path)?;
                    removed += 1;
                }
            }
            _ => {}
        }
    }
    if removed > 0 {
        fs.sync_dir(dir)?;
    }
    Ok(removed)
}

/// The temporary files [`sweep_temps`] would remove from `dir`, removing
/// nothing: what a session that only reads reports (`f1r3gaze profile
/// check`).
pub fn stale_temps(fs: &dyn Fs, dir: &Path) -> io::Result<Vec<PathBuf>> {
    let own = process_id();
    let mut stale = Vec::new();
    for name in fs.list(dir)? {
        match temp_owner(&name) {
            Some(pid) if pid != own => {
                let path = dir.join(&name);
                if fs.kind(&path)? == Kind::File {
                    stale.push(path);
                }
            }
            _ => {}
        }
    }
    stale.sort();
    Ok(stale)
}

/// The temporary files [`sweep_temps_of`] would remove from `dir` for the
/// name `name`, removing nothing.
pub fn stale_temps_of(fs: &dyn Fs, dir: &Path, name: &OsStr) -> io::Result<Vec<PathBuf>> {
    let own = process_id();
    let mut prefix = std::ffi::OsString::with_capacity(name.len() + 6);
    prefix.push(".");
    prefix.push(name);
    prefix.push(".tmp-");
    let prefix = prefix.to_string_lossy().into_owned();
    let mut stale = Vec::new();
    for entry in fs.list(dir)? {
        match (entry.to_string_lossy().starts_with(&prefix), temp_owner(&entry)) {
            (true, Some(pid)) if pid != own => {
                let path = dir.join(&entry);
                if fs.kind(&path)? == Kind::File {
                    stale.push(path);
                }
            }
            _ => {}
        }
    }
    stale.sort();
    Ok(stale)
}

/// Removes, from `dir`, the temporary files of one name: those [`temp_path`]
/// made for `dir/<name>` (`.<name>.tmp-<pid>-<n>`) in processes other than
/// this one; then syncs `dir` if it removed any. For a folder that holds
/// other programs' files too, such as a linked `settings.toml`'s target
/// folder, or a folder whose other temporary files must stay, such as a
/// backup session's. Returns how many it removed.
pub fn sweep_temps_of(fs: &dyn Fs, dir: &Path, name: &OsStr) -> io::Result<usize> {
    let own = process_id();
    let mut prefix = std::ffi::OsString::with_capacity(name.len() + 6);
    prefix.push(".");
    prefix.push(name);
    prefix.push(".tmp-");
    let prefix = prefix.to_string_lossy().into_owned();
    let mut removed = 0;
    for entry in fs.list(dir)? {
        match (entry.to_string_lossy().starts_with(&prefix), temp_owner(&entry)) {
            (true, Some(pid)) if pid != own => {
                let path = dir.join(&entry);
                if fs.kind(&path)? == Kind::File {
                    fs.remove_file(&path)?;
                    removed += 1;
                }
            }
            _ => {}
        }
    }
    if removed > 0 {
        fs.sync_dir(dir)?;
    }
    Ok(removed)
}

/// Removes a temporary file after a failed write and syncs its directory,
/// so the removal is not left pending. Errors are ignored: the next start
/// sweeps whatever is left.
fn discard(fs: &dyn Fs, temp: &Path) {
    if fs.remove_file(temp).is_ok()
        && let Ok(dir) = parent(temp)
    {
        let _ = fs.sync_dir(dir);
    }
}
