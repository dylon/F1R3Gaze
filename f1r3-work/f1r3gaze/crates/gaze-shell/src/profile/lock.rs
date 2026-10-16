//! The instance lock: one F1R3Gaze at a time writes a profile.
//!
//! Two locks are taken, in this order:
//!
//! 1. **The anchor**, `<data>/instance.lock`, guards the data root, which
//!    holds what two writers could damage: the site stores are append-only
//!    logs. Every process that writes the same data root meets here,
//!    whatever its `$XDG_RUNTIME_DIR` (a cron job, `su`), and no cleaner of
//!    temporary files looks there.
//! 2. **The runtime lock**, `<runtime>/instance.lock`, is in the folder the
//!    platform keeps for such files (`$XDG_RUNTIME_DIR` on Linux, `$TMPDIR`
//!    on macOS). On Linux it has the sticky bit, which the XDG Base
//!    Directory specification names as the way to keep a cleaner away. If a
//!    cleaner removes it anyway, the anchor still holds.
//!
//! They are released in the reverse order, so a process starting meanwhile
//! never takes the anchor and then finds the runtime lock still held.
//!
//! The operating system releases a lock when its process ends, however it
//! ends, so a crash never leaves a stale lock. Each lock file, and an
//! `instance.pid` beside it, says who holds it: the process id, what it runs,
//! since when, and its data root. The `instance.pid` files exist because
//! Windows forbids reading a locked file's bytes.
//!
//! A lock that cannot be taken because the file system does not support
//! locks is reported, and the other one guards alone. When neither can be
//! taken, or the anchor cannot be created at all (a read-only file system,
//! no permission), the profile can only be read.

use super::layout::Layout;
use super::report::{Event, EventKind, Report, Severity};
use crate::display::utc_date;
use gaze_fs::{Fs, Perm, StdFs};
#[cfg(unix)]
use gaze_fs::PRIVATE_FILE_MODE;
use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The name of the file beside each lock that says who holds it.
pub const HOLDER_FILE: &str = "instance.pid";

/// How long a held lock is tried again before it counts as held. A lock is
/// not always free the moment its holder ends:
/// - on Windows, "the time it takes for the operating system to unlock
///   these locks depends upon available system resources" (LockFileEx);
/// - on Unix, a process the holder was starting keeps a copy of the lock's
///   descriptor until it runs its program, and the lock lasts while any
///   copy is open (File::lock: "along with any other file descriptors/
///   handles duplicated or inherited from it").
///
/// So a quick restart is not told that another F1R3Gaze holds the profile.
pub const HELD_RETRY: Duration = Duration::from_secs(1);

/// How often a held lock is tried within [`HELD_RETRY`].
const HELD_POLL: Duration = Duration::from_millis(50);

/// The runtime lock's mode on Linux: owner read and write, and the sticky
/// bit, which tells cleaners of `$XDG_RUNTIME_DIR` to leave it.
#[cfg(target_os = "linux")]
const STICKY_PRIVATE_FILE_MODE: u32 = 0o1600;

/// What the holder of a lock runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The browser's window.
    Window,
    /// `f1r3gaze --headless`.
    Headless,
    /// A command that writes the profile, like `wallet new`.
    Command(&'static str),
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Window => "window",
            Mode::Headless => "headless",
            Mode::Command(name) => name,
        }
    }
}

/// Who holds a lock, as its files say. Any field may be missing: they are
/// written just after the lock is taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holder {
    pub pid: Option<u32>,
    pub mode: Option<String>,
    /// When it started, in UTC (`2026-10-05T18:21:02Z`).
    pub started: Option<String>,
    /// Its data root.
    pub data: Option<String>,
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("%0A", "\n").replace("%0D", "\r").replace("%25", "%")
}

impl Holder {
    /// The holder's lines: `key=value`, with `%`, CR and LF percent-escaped.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(128);
        if let Some(pid) = self.pid {
            out.push_str(&format!("pid={pid}\n"));
        }
        for (key, value) in [("mode", &self.mode), ("started", &self.started), ("data", &self.data)] {
            if let Some(value) = value {
                out.push_str(&format!("{key}={}\n", escape(value)));
            }
        }
        out
    }

    /// Reads [`Holder::render`]'s lines; anything else is skipped.
    pub fn parse(text: &str) -> Holder {
        let mut holder = Holder::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("pid", pid)) => holder.pid = pid.parse().ok(),
                Some(("mode", mode)) => holder.mode = Some(unescape(mode)),
                Some(("started", started)) => holder.started = Some(unescape(started)),
                Some(("data", data)) => holder.data = Some(unescape(data)),
                _ => {}
            }
        }
        holder
    }
}

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.pid {
            Some(pid) => write!(f, "process {pid}")?,
            None => write!(f, "another process")?,
        }
        match (&self.mode, &self.started) {
            (Some(mode), Some(started)) => write!(f, " ({mode}, since {started})"),
            (Some(mode), None) => write!(f, " ({mode})"),
            (None, Some(started)) => write!(f, " (since {started})"),
            (None, None) => Ok(()),
        }
    }
}

/// A time as UTC in RFC 3339 form, to the second.
pub fn utc_time(unix_seconds: u64) -> String {
    let (year, month, day) = utc_date(unix_seconds);
    let s = unix_seconds % 86_400;
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", s / 3_600, s / 60 % 60, s % 60)
}

/// Why the profile cannot be locked for writing.
#[derive(Debug)]
pub enum LockError {
    /// Another F1R3Gaze holds `lock`.
    Held { lock: PathBuf, holder: Holder },
    /// The anchor cannot be created or opened for writing: a read-only file
    /// system, or no permission. The profile can only be read.
    ReadOnly { lock: PathBuf, error: io::Error },
    /// Neither lock can be taken here; two copies of F1R3Gaze could not be
    /// kept apart, so the profile can only be read.
    Unavailable { errors: Vec<(PathBuf, io::Error)> },
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Held { lock, holder } => write!(
                f,
                "another F1R3Gaze is using this profile: {holder}; its lock is {}",
                lock.display()
            ),
            LockError::ReadOnly { lock, error } => {
                write!(f, "{} cannot be written ({error}); the profile can only be read", lock.display())
            }
            LockError::Unavailable { errors } => {
                write!(f, "no lock can be taken on this profile")?;
                for (lock, error) in errors {
                    write!(f, "; {}: {error}", lock.display())?;
                }
                write!(f, "; it can only be read, so that two copies of F1R3Gaze cannot damage it")
            }
        }
    }
}

impl std::error::Error for LockError {}

/// The locks this process holds; dropping it releases them, the runtime
/// lock first.
#[derive(Debug)]
pub struct InstanceLock {
    /// In the order taken.
    held: Vec<(PathBuf, File)>,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        self.release_all();
    }
}

enum Take {
    Taken(File),
    Held,
    Failed(io::Error),
}

/// Tries to lock the file at `path`, trying again for up to `patience`
/// while it is held. Its folders are made through `fs`.
///
/// Was `take_patiently(path, patience)`, which made them through `StdFs`
/// whatever file system start-up ran on: a traced start then missed the
/// lock's operations, and a real kill at the k-th operation no longer lined
/// up with the k-th record of its trace (ledger S8, part 2).
fn take_patiently_in(fs: &dyn Fs, path: &Path, patience: Duration) -> Take {
    let deadline = Instant::now() + patience;
    loop {
        match take_in(fs, path) {
            Take::Held if Instant::now() < deadline => std::thread::sleep(HELD_POLL),
            taken => return taken,
        }
    }
}

// Disabled: its only caller was `take_patiently`, which now passes start-up's
// file system to `take_in` (see `take_patiently_in`).
// /// Opens (creating it, owner-only) and tries to lock the file at `path`.
// fn take(path: &Path) -> Take {
//     take_in(&StdFs, path)
// }

/// [`take`], with the lock's folder made through `fs`. The folders are
/// created durably: the anchor's folder is the data root, which the first
/// start creates here, and later steps find it there and sync nothing above
/// it (ledger S6, part 2).
fn take_in(fs: &dyn Fs, path: &Path) -> Take {
    let dir = match gaze_fs::parent(path) {
        Ok(dir) => dir,
        Err(e) => return Take::Failed(e),
    };
    // Was `StdFs.create_dir_all(dir)`: a power cut after the first start
    // could lose the new data root with everything start-up wrote in it.
    if let Err(e) = gaze_fs::create_dir_durably(fs, dir) {
        return Take::Failed(e);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_FILE_MODE);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) => return Take::Failed(e),
    };
    match file.try_lock() {
        Ok(()) => Take::Taken(file),
        Err(TryLockError::WouldBlock) => Take::Held,
        Err(TryLockError::Error(e)) => Take::Failed(e),
    }
}

/// Who holds the lock at `lock`: its `instance.pid`, or, where a locked
/// file can be read, the lock file itself.
pub fn read_holder(lock: &Path) -> Holder {
    read_holder_in(&StdFs, lock)
}

/// [`read_holder`], reading through `fs`.
pub fn read_holder_in(fs: &dyn Fs, lock: &Path) -> Holder {
    let beside = lock.with_file_name(HOLDER_FILE);
    match fs.read(&beside).or_else(|_| fs.read(lock)) {
        Ok(bytes) => Holder::parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Holder::default(),
    }
}

/// Whether a failure to open the anchor means the profile cannot be written
/// at all, rather than that this file system has no locks.
fn means_read_only(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::ReadOnlyFilesystem | io::ErrorKind::PermissionDenied)
}

/// Writes `info` into a lock file this process holds.
fn write_into(mut file: &File, info: &str) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(info.as_bytes())
}

impl InstanceLock {
    /// Takes the anchor, then the runtime lock, for `mode`, and records who
    /// holds them. Problems that leave the profile guarded are reported;
    /// the others are the error.
    pub fn acquire(layout: &Layout, mode: Mode, now: SystemTime, report: &Report) -> Result<InstanceLock, LockError> {
        InstanceLock::acquire_in(&StdFs, layout, mode, now, report)
    }

    /// [`InstanceLock::acquire`] on the file system start-up runs on: the
    /// lock's folders, the holder's `instance.pid` and the reading of
    /// another holder's go through `fs`. Locking itself is a call on the
    /// open file, which no `Fs` models.
    pub fn acquire_in(
        fs: &dyn Fs,
        layout: &Layout,
        mode: Mode,
        now: SystemTime,
        report: &Report,
    ) -> Result<InstanceLock, LockError> {
        InstanceLock::acquire_within(fs, layout, mode, now, report, HELD_RETRY)
    }

    /// [`InstanceLock::acquire_in`], trying a held lock again for
    /// `patience`.
    pub fn acquire_within(
        fs: &dyn Fs,
        layout: &Layout,
        mode: Mode,
        now: SystemTime,
        report: &Report,
        patience: Duration,
    ) -> Result<InstanceLock, LockError> {
        if layout.runtime_is_fallback {
            report.push(Event::new(
                EventKind::RuntimeFallback,
                Severity::Notice,
                &layout.runtime,
                format!(
                    "$XDG_RUNTIME_DIR is not set to an absolute path, so the instance lock is kept in {}",
                    layout.runtime.display()
                ),
            ));
        }
        let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let holder = Holder {
            pid: Some(std::process::id()),
            mode: Some(mode.name().to_string()),
            started: Some(utc_time(seconds)),
            data: Some(layout.data.to_string_lossy().into_owned()),
        };
        let mut lock = InstanceLock { held: Vec::with_capacity(2) };
        let mut unsupported = Vec::new();
        for (path, anchor) in [(layout.anchor_lock_file(), true), (layout.lock_file(), false)] {
            match take_patiently_in(fs, &path, patience) {
                Take::Taken(file) => {
                    #[cfg(target_os = "linux")]
                    if !anchor {
                        use std::os::unix::fs::PermissionsExt;
                        if let Err(e) = file.set_permissions(std::fs::Permissions::from_mode(STICKY_PRIVATE_FILE_MODE)) {
                            report.push(Event::new(
                                EventKind::LockUnavailable { error: e.to_string() },
                                Severity::Notice,
                                &path,
                                format!(
                                    "{} cannot be marked sticky ({e}); a cleaner may remove it, and the lock in the data folder still holds",
                                    path.display()
                                ),
                            ));
                        }
                    }
                    lock.held.push((path, file));
                }
                // Returning drops `lock`, releasing what it holds.
                Take::Held => {
                    let holder = read_holder_in(fs, &path);
                    return Err(LockError::Held { lock: path, holder });
                }
                Take::Failed(error) if anchor && means_read_only(&error) => {
                    return Err(LockError::ReadOnly { lock: path, error });
                }
                Take::Failed(error) => unsupported.push((path, error)),
            }
        }
        if lock.held.is_empty() {
            return Err(LockError::Unavailable { errors: unsupported });
        }
        for (path, error) in &unsupported {
            report.push(Event::new(
                EventKind::LockUnavailable { error: error.to_string() },
                Severity::Warning,
                path,
                format!(
                    "{} cannot be locked here ({error}); the other instance lock guards the profile alone",
                    path.display()
                ),
            ));
        }
        let info = holder.render();
        for (path, file) in &lock.held {
            let beside = path.with_file_name(HOLDER_FILE);
            // Was `gaze_fs::write_atomic`, which synced the temporary file
            // and the folder: four fsyncs a start for a record that any
            // crash ends (ledger S15, part 2, V1). Readers still never see
            // half of it.
            // let written = write_into(file, &info)
            //     .and_then(|()| gaze_fs::write_atomic(fs, &beside, info.as_bytes(), Perm::Private));
            let written = write_into(file, &info)
                .and_then(|()| gaze_fs::replace_unsynced(fs, &beside, info.as_bytes(), Perm::Private));
            if let Err(e) = written {
                report.push(Event::new(
                    EventKind::Other,
                    Severity::Notice,
                    path,
                    format!(
                        "who holds {} cannot be recorded ({e}); a second F1R3Gaze is still refused, without the holder's name",
                        path.display()
                    ),
                ));
            }
        }
        Ok(lock)
    }

    /// The lock files held, in the order taken.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.held.iter().map(|(path, _)| path.as_path())
    }

    /// Releases the locks, the last taken first; returns their files in
    /// the order released. Dropping the lock does the same.
    pub fn release(mut self) -> Vec<PathBuf> {
        self.release_all()
    }

    fn release_all(&mut self) -> Vec<PathBuf> {
        let mut released = Vec::with_capacity(self.held.len());
        while let Some((path, file)) = self.held.pop() {
            drop(file);
            released.push(path);
        }
        released
    }
}

#[cfg(test)]
mod tests;
