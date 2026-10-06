//! The operations F1R3Gaze's storage is built from, and their real
//! implementation, [`StdFs`].
//!
//! Each operation is one system call, or as close to one as the platform
//! allows, so that a test file system ([`crate::MemFs`]) can stop a run
//! between any two of them and cut the power.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Owner read and write only, for every file under the data, state, backup
/// and runtime roots.
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// Owner only, for every directory F1R3Gaze creates (XDG Base Directory
/// specification: "an attempt should be made to create it with permission
/// 0700").
pub const PRIVATE_DIR_MODE: u32 = 0o700;

/// What a path names, without following a final symbolic link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

/// The file-system operations, one system call each.
///
/// Paths are absolute. Every operation that creates or removes a name is a
/// *directory operation*: it is atomic, and durable only once the directory
/// holding the name is synced ([`Fs::sync_dir`]). Data written to a file is
/// durable only once the file is synced ([`Fs::sync_file`]). These are the
/// assumptions A1–A3 of the start-up model (docs/storage/README.md, "The
/// formal model").
pub trait Fs {
    /// The whole content of a file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// What `path` names; `NotFound` if nothing.
    fn kind(&self, path: &Path) -> io::Result<Kind>;
    /// The target of a symbolic link.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;
    /// The names in a directory, in no particular order.
    fn list(&self, dir: &Path) -> io::Result<Vec<OsString>>;
    /// Creates a directory and every missing parent, mode
    /// [`PRIVATE_DIR_MODE`] on Unix. Existing directories keep their mode.
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;
    /// Creates the directory `dir`, mode [`PRIVATE_DIR_MODE`] on Unix. Its
    /// parent must exist; `AlreadyExists` if `dir` does, which is how a
    /// process claims a name no other one has.
    fn create_dir(&self, dir: &Path) -> io::Result<()>;
    /// Creates `path`, which must not exist (`AlreadyExists`), holding
    /// `bytes`, with `mode` on Unix from the moment the name exists. Not
    /// synced.
    fn create_new(&self, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()>;
    /// Copies `from` into a new file `to`, like [`Fs::create_new`].
    fn copy_new(&self, from: &Path, to: &Path, mode: u32) -> io::Result<()>;
    /// Whether two files hold the same bytes, read in bounded chunks.
    fn same_contents(&self, a: &Path, b: &Path) -> io::Result<bool>;
    /// Makes a file's data and metadata durable (fsync; `F_FULLFSYNC` on
    /// macOS; `FlushFileBuffers` on Windows).
    fn sync_file(&self, path: &Path) -> io::Result<()>;
    /// Makes every directory operation on names in `dir` durable (fsync of
    /// the directory). A no-op on Windows; see [`StdFs`].
    fn sync_dir(&self, dir: &Path) -> io::Result<()>;
    /// A second name for the file at `from` (link(2)). Fails with
    /// `AlreadyExists` if `to` exists, and never replaces it.
    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// rename(2): moves the name, replacing `to` if it exists. Callers that
    /// must not replace use [`crate::publish_no_replace`].
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Removes an empty directory.
    fn remove_dir(&self, dir: &Path) -> io::Result<()>;
    /// Marks a point in the sequence of operations, such as the start and
    /// the end of the start-up barrier, for a [`crate::TraceFs`] to record.
    /// Does nothing on a real file system.
    fn note(&self, _what: &str) {}
    /// A file's permission bits on Unix; `None` on other platforms.
    fn mode(&self, path: &Path) -> io::Result<Option<u32>>;
    /// A file's length in bytes.
    fn len(&self, path: &Path) -> io::Result<u64>;
    /// Sets a file's permission bits on Unix; nothing elsewhere.
    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()>;
}

/// The real file system.
///
/// **Windows.** The standard library cannot sync a directory there, so
/// [`Fs::sync_dir`] does nothing. Instead, every operation that gives a file
/// a name ([`Fs::hard_link`], [`Fs::rename`]) then syncs that file
/// (`FlushFileBuffers`). Two properties of NTFS make this enough:
/// - W1: NTFS logs metadata changes in order, one log per volume, so a power
///   cut keeps a prefix of a volume's changes, never a later one without an
///   earlier one.
/// - W2: `FlushFileBuffers` on a file commits the volume's log up to that
///   point, so the name just given, and every change before it, is durable.
///
/// Under W1 and W2 a new name is durable before any old name is removed.
/// The start-up model is checked under these semantics too
/// (`MCProfileStartupNtfs.cfg`), and `MemFs::ntfs` reproduces them for the
/// crash tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdFs;

impl Fs for StdFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        crash_point();
        std::fs::read(path)
    }

    fn kind(&self, path: &Path) -> io::Result<Kind> {
        crash_point();
        let meta = std::fs::symlink_metadata(path)?;
        let kind = meta.file_type();
        Ok(match (kind.is_file(), kind.is_dir(), kind.is_symlink()) {
            (true, _, _) => Kind::File,
            (_, true, _) => Kind::Dir,
            (_, _, true) => Kind::Symlink,
            _ => Kind::Other,
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        crash_point();
        std::fs::read_link(path)
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<OsString>> {
        crash_point();
        let entries = std::fs::read_dir(dir)?;
        let mut names = Vec::with_capacity(entries.size_hint().0);
        for entry in entries {
            names.push(entry?.file_name());
        }
        Ok(names)
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        crash_point();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(PRIVATE_DIR_MODE);
        }
        builder.create(dir)
    }

    fn create_dir(&self, dir: &Path) -> io::Result<()> {
        crash_point();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(PRIVATE_DIR_MODE).create(dir)
        }
        // Elsewhere the folder inherits its parent's access list.
        #[cfg(not(unix))]
        {
            std::fs::DirBuilder::new().create(dir)
        }
    }

    fn create_new(&self, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        crash_point();
        let mut file = new_file(path, mode)?;
        io::Write::write_all(&mut file, bytes)
    }

    fn copy_new(&self, from: &Path, to: &Path, mode: u32) -> io::Result<()> {
        crash_point();
        let mut source = File::open(from)?;
        let mut target = new_file(to, mode)?;
        io::copy(&mut source, &mut target).map(drop)
    }

    fn same_contents(&self, a: &Path, b: &Path) -> io::Result<bool> {
        crash_point();
        let (first, second) = (File::open(a)?, File::open(b)?);
        match first.metadata()?.len() == second.metadata()?.len() {
            false => Ok(false),
            true => equal_streams(first, second),
        }
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        crash_point();
        // fsync works on a read-only descriptor, which also syncs a file whose
        // preserved mode denies writing (0400). FlushFileBuffers on Windows
        // needs write access; opening for writing changes nothing.
        let file = match cfg!(windows) {
            true => OpenOptions::new().write(true).open(path)?,
            false => File::open(path)?,
        };
        file.sync_all()
    }

    #[cfg(unix)]
    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        crash_point();
        File::open(dir)?.sync_all()
    }

    #[cfg(not(unix))]
    fn sync_dir(&self, _dir: &Path) -> io::Result<()> {
        crash_point();
        Ok(())
    }

    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        crash_point();
        std::fs::hard_link(from, to)?;
        commit_name(to)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        crash_point();
        rename_retrying(from, to)?;
        commit_name(to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        crash_point();
        std::fs::remove_file(path)
    }

    fn remove_dir(&self, dir: &Path) -> io::Result<()> {
        crash_point();
        std::fs::remove_dir(dir)
    }

    #[cfg(unix)]
    fn mode(&self, path: &Path) -> io::Result<Option<u32>> {
        use std::os::unix::fs::PermissionsExt;
        crash_point();
        Ok(Some(std::fs::metadata(path)?.permissions().mode() & 0o7777))
    }

    #[cfg(not(unix))]
    fn mode(&self, path: &Path) -> io::Result<Option<u32>> {
        crash_point();
        std::fs::metadata(path).map(|_| None)
    }

    fn len(&self, path: &Path) -> io::Result<u64> {
        crash_point();
        std::fs::metadata(path).map(|meta| meta.len())
    }

    #[cfg(unix)]
    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        crash_point();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }

    #[cfg(not(unix))]
    fn set_mode(&self, path: &Path, _mode: u32) -> io::Result<()> {
        crash_point();
        std::fs::metadata(path).map(drop)
    }
}

/// Whether two readers yield the same bytes, compared 64 KiB at a time.
fn equal_streams(a: impl io::Read, b: impl io::Read) -> io::Result<bool> {
    const CHUNK: usize = 64 * 1024;
    let mut a = io::BufReader::with_capacity(CHUNK, a);
    let mut b = io::BufReader::with_capacity(CHUNK, b);
    loop {
        let (left, right) = (io::BufRead::fill_buf(&mut a)?, io::BufRead::fill_buf(&mut b)?);
        let n = left.len().min(right.len());
        match (n, left[..n] == right[..n]) {
            (0, _) => return Ok(left.is_empty() && right.is_empty()),
            (_, false) => return Ok(false),
            (_, true) => {
                io::BufRead::consume(&mut a, n);
                io::BufRead::consume(&mut b, n);
            }
        }
    }
}

/// A new file, created exclusively, with `mode` from the start on Unix.
fn new_file(path: &Path, mode: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path)
}

/// On Windows, makes the name just given to the file at `path` durable by
/// syncing the file (W2 in [`StdFs`]); a directory, which cannot be synced
/// there, is left to NTFS's log. Nothing elsewhere: the caller syncs the
/// directory.
#[cfg(windows)]
fn commit_name(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path)?.is_file() {
        true => OpenOptions::new().write(true).open(path)?.sync_all(),
        false => Ok(()),
    }
}

#[cfg(not(windows))]
#[inline(always)]
fn commit_name(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// rename(2), the one call `StdFs` makes that the workspace's clippy
/// configuration disallows elsewhere (everything else goes through this
/// crate). On Windows an antivirus scanner or the search indexer may hold
/// the target open for a moment, which makes `MoveFileExW` fail with a
/// sharing violation (`PermissionDenied`): retried five times, 20 to 320 ms
/// apart.
#[allow(clippy::disallowed_methods)]
fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0u32;
    loop {
        match std::fs::rename(from, to) {
            Err(e)
                if cfg!(windows)
                    && e.kind() == io::ErrorKind::PermissionDenied
                    && attempt < 5 =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 << attempt));
            }
            other => return other,
        }
    }
}

/// With the `crash-points` feature and `F1R3GAZE_CRASH_AT=k`, the k-th
/// operation (counting from 1) aborts the process: a real crash, with
/// nothing flushed from user space.
#[cfg(feature = "crash-points")]
fn crash_point() {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static AT: OnceLock<Option<usize>> = OnceLock::new();
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let at = AT.get_or_init(|| {
        std::env::var("F1R3GAZE_CRASH_AT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
    });
    if let Some(at) = *at
        && COUNT.fetch_add(1, Ordering::SeqCst) + 1 == at
    {
        std::process::abort();
    }
}

#[cfg(not(feature = "crash-points"))]
#[inline(always)]
fn crash_point() {}
