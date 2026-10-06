//! Backups of files before F1R3Gaze replaces or cuts them.
//!
//! A backup goes to `<class root>/backups/<UTC time>[.n]/<path below the
//! root>`: one folder per start (a *session*), named by the time it began,
//! claimed with an exclusive `create_dir` so two processes never share one,
//! and the file at its own relative path inside it, `.n` appended if that
//! name is taken. A backup never replaces anything.
//!
//! [`Backups::copy_into`] keeps a durable copy of a file that is then
//! repaired in place: a damaged managed file at start-up (the model's
//! `Create(Bak)`, `FsyncFile`, `FsyncDir`, before the repair is renamed over
//! the file; docs/storage/tla/ProfileStartup.tla), or a site's store. The
//! file is never absent, and its content is never only in a backup.
//!
//! [`Backups::move_into`] and [`Backups::move_into_at`] move the file
//! itself, so the backup has exactly its bytes and mode, durably before
//! they return. Start-up no longer uses them: moving a damaged file away
//! and then writing its repair leaves it absent in between (the model's
//! fault `move-then-write`; ledger S2, part 3). They are kept, with their
//! tests, off the start-up path (ledger S6, part 2).

use super::layout::{Class, Layout};
use super::report::{Event, EventKind, Report, Severity};
use crate::display::utc_date;
use gaze_fs::{Fs, Kind, PRIVATE_FILE_MODE};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The name of a backup session: the UTC time as `YYYY-MM-DDTHH-MM-SSZ`,
/// without `:`, so it is a valid file name on Windows.
pub fn backup_stamp(unix_seconds: u64) -> String {
    let (year, month, day) = utc_date(unix_seconds);
    let s = unix_seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}Z",
        s / 3_600,
        s / 60 % 60,
        s % 60
    )
}

/// The Unix time a session folder's name stands for (`.n` suffix allowed);
/// `None` for any other name.
pub fn stamp_seconds(name: &str) -> Option<u64> {
    let stamp = name.split_once('.').map_or(name, |(stamp, n)| match n.parse::<u32>() {
        Ok(_) => stamp,
        Err(_) => "",
    });
    let b = stamp.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b'-' || b[16] != b'-' || b[19] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<u32>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    let seconds = days * 86_400 + i64::from(hour * 3_600 + minute * 60 + second);
    u64::try_from(seconds).ok().filter(|&s| backup_stamp(s) == stamp)
}

/// Days since 1970-01-01 of a UTC civil date: the inverse of
/// [`crate::display::utc_date`], Howard Hinnant's `days_from_civil`
/// (<http://howardhinnant.github.io/date_algorithms.html#days_from_civil>).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400; // [0, 399]
    let shifted_month = (i64::from(month) + 9) % 12; // March = 0
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Creates `dir` and its missing parents, then syncs every directory from
/// its parent up to `root`, so the new entries are durable before anything
/// is put in them.
pub fn create_durably(fs: &dyn Fs, dir: &Path, root: &Path) -> io::Result<()> {
    // Every folder from `dir`'s parent up to `root` is synced right below.
    #[allow(clippy::disallowed_methods)]
    fs.create_dir_all(dir)?;
    let mut current = dir;
    while current != root {
        let Some(parent) = current.parent() else { break };
        fs.sync_dir(parent)?;
        if !parent.starts_with(root) {
            break;
        }
        current = parent;
    }
    Ok(())
}

/// The backups of one start. Clones share the start's session folders, so
/// a store opened late in the session backs up into the same folder as
/// start-up did.
#[derive(Clone)]
pub struct Backups {
    layout: Arc<Layout>,
    fs: Arc<dyn Fs + Send + Sync>,
    stamp: String,
    sessions: Arc<Mutex<BTreeMap<Class, PathBuf>>>,
}

impl std::fmt::Debug for Backups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backups").field("stamp", &self.stamp).finish_non_exhaustive()
    }
}

impl Backups {
    pub fn new(layout: Arc<Layout>, fs: Arc<dyn Fs + Send + Sync>, now: SystemTime) -> Backups {
        let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        Backups {
            layout,
            fs,
            stamp: backup_stamp(seconds),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// This start's backup folder for `class`, claimed on first use.
    fn session(&self, class: Class) -> io::Result<PathBuf> {
        let mut sessions = self.sessions.lock().expect("the backups' lock is never poisoned");
        if let Some(session) = sessions.get(&class) {
            return Ok(session.clone());
        }
        let base = self.layout.backups(class);
        create_durably(self.fs.as_ref(), &base, self.layout.root(class))?;
        for n in 0u32.. {
            let name = match n {
                0 => self.stamp.clone(),
                n => format!("{}.{n}", self.stamp),
            };
            let session = base.join(name);
            match self.fs.create_dir(&session) {
                Ok(()) => {
                    self.fs.sync_dir(&base)?;
                    sessions.insert(class, session.clone());
                    return Ok(session);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        unreachable!("u32 session numbers run out only after 4 billion backups in one second")
    }

    /// A free name for `relative` in this start's session of `class`, its
    /// parent directories created durably.
    fn free_destination(&self, class: Class, relative: &Path) -> io::Result<PathBuf> {
        let session = self.session(class)?;
        let wanted = session.join(relative);
        let dir = gaze_fs::parent(&wanted)?.to_path_buf();
        create_durably(self.fs.as_ref(), &dir, &session)?;
        let name = wanted.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        for n in 0u32.. {
            let candidate = match n {
                0 => wanted.clone(),
                n => {
                    let mut numbered = name.clone();
                    numbered.push(format!(".{n}"));
                    dir.join(numbered)
                }
            };
            match self.fs.kind(&candidate) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(candidate),
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        unreachable!("u32 suffixes run out only after 4 billion backups of one file")
    }

    fn class_of(&self, path: &Path) -> io::Result<(Class, PathBuf)> {
        self.layout.class_of(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not in the profile", path.display()),
            )
        })
    }

    /// Moves the file at `path`, which is under one of the profile's roots,
    /// into the backups; returns where it went.
    pub fn move_into(&self, path: &Path) -> io::Result<PathBuf> {
        let (class, relative) = self.class_of(path)?;
        self.move_into_at(path, class, &relative)
    }

    /// Moves the file at `path` (anywhere, such as an old profile's
    /// `settings.conf`) into `class`'s backups at `relative`.
    pub fn move_into_at(&self, path: &Path, class: Class, relative: &Path) -> io::Result<PathBuf> {
        let destination = self.free_destination(class, relative)?;
        gaze_fs::move_no_replace(self.fs.as_ref(), path, &destination)?;
        Ok(destination)
    }

    /// Keeps a copy of `bytes`, the content of `path` (under one of the
    /// profile's roots), in the backups, owner-only and synced; `path` is
    /// left as it is. Returns where the copy is.
    pub fn copy_into(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        let (class, relative) = self.class_of(path)?;
        let destination = self.free_destination(class, &relative)?;
        self.fs.create_new(&destination, bytes, PRIVATE_FILE_MODE)?;
        self.fs.sync_file(&destination)?;
        self.fs.sync_dir(gaze_fs::parent(&destination)?)?;
        Ok(destination)
    }
}

/// Keeps a damaged site store in the data backups before gaze-store cuts
/// it back to its readable records, and reports it.
pub struct StoreSalvage {
    backups: Backups,
    report: Report,
    /// The site the store belongs to, for the report.
    site: String,
}

impl StoreSalvage {
    pub fn new(backups: Backups, report: Report, site: impl Into<String>) -> StoreSalvage {
        StoreSalvage {
            backups,
            report,
            site: site.into(),
        }
    }
}

impl gaze_store::Salvage for StoreSalvage {
    fn keep(&self, damage: &gaze_store::Damage<'_>) -> Result<(), String> {
        let copy = self
            .backups
            .copy_into(damage.path, damage.bytes)
            .map_err(|e| format!("{} cannot be backed up: {e}", damage.path.display()))?;
        // A record cut short is what a crash during an append leaves; a
        // record that cannot be read is damage.
        let (severity, ending) = match damage.torn_tail {
            true => (Severity::Notice, "a record cut short by a crash"),
            false => (Severity::Warning, "records that cannot be read"),
        };
        self.report.push(
            Event::new(
                EventKind::Salvaged {
                    site: self.site.clone(),
                    kept: damage.kept,
                    torn_tail: damage.torn_tail,
                },
                severity,
                damage.path,
                format!(
                    "The data {} stores ended in {ending}, which was dropped; the whole file is kept in {}",
                    self.site,
                    copy.display()
                ),
            )
            .with_backup(copy),
        );
        Ok(())
    }
}

/// One start's backups of one class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub class: Class,
    pub path: PathBuf,
    /// When it began, if its name says.
    pub seconds: Option<u64>,
    pub files: usize,
    pub bytes: u64,
}

/// Every backup session of every class, oldest first.
pub fn list_sessions(layout: &Layout, fs: &dyn Fs) -> io::Result<Vec<Session>> {
    let mut sessions = Vec::new();
    let mut seen = Vec::with_capacity(Class::ALL.len());
    for class in Class::ALL {
        let base = layout.backups(class);
        // macOS and Windows put several classes under one folder; list each
        // backups folder once.
        if seen.contains(&base) {
            continue;
        }
        seen.push(base.clone());
        let names = match fs.list(&base) {
            Ok(names) => names,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for name in names {
            let path = base.join(&name);
            if fs.kind(&path)? != Kind::Dir {
                continue;
            }
            let (files, bytes) = tree_size(fs, &path)?;
            sessions.push(Session {
                class,
                seconds: stamp_seconds(&name.to_string_lossy()),
                path,
                files,
                bytes,
            });
        }
    }
    sessions.sort_by(|a, b| (a.seconds, &a.path).cmp(&(b.seconds, &b.path)));
    Ok(sessions)
}

fn tree_size(fs: &dyn Fs, dir: &Path) -> io::Result<(usize, u64)> {
    let (mut files, mut bytes) = (0, 0);
    for name in fs.list(dir)? {
        let path = dir.join(name);
        match fs.kind(&path)? {
            Kind::Dir => {
                let (f, b) = tree_size(fs, &path)?;
                files += f;
                bytes += b;
            }
            Kind::File => {
                files += 1;
                bytes += fs.len(&path)?;
            }
            Kind::Symlink | Kind::Other => {}
        }
    }
    Ok((files, bytes))
}

/// Removes a directory and everything in it, then syncs its parent.
fn remove_tree(fs: &dyn Fs, dir: &Path) -> io::Result<()> {
    for name in fs.list(dir)? {
        let path = dir.join(name);
        match fs.kind(&path)? {
            Kind::Dir => remove_tree(fs, &path)?,
            _ => fs.remove_file(&path)?,
        }
    }
    fs.remove_dir(dir)?;
    fs.sync_dir(gaze_fs::parent(dir)?)
}

/// Removes the backup sessions that began more than `older_than` before
/// `now`, as the user asked (`f1r3gaze profile backups prune`). Sessions
/// whose name gives no time are kept. Returns the ones removed.
pub fn prune_sessions(layout: &Layout, fs: &dyn Fs, older_than: Duration, now: SystemTime) -> io::Result<Vec<Session>> {
    let now = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let limit = now.saturating_sub(older_than.as_secs());
    let mut removed = Vec::new();
    for session in list_sessions(layout, fs)? {
        if let Some(seconds) = session.seconds
            && seconds < limit
        {
            remove_tree(fs, &session.path)?;
            removed.push(session);
        }
    }
    Ok(removed)
}

/// Removes every backup of the browsing history (`history.json`, and an old
/// profile's `workspace.json`, which held it too): "Clear history" means
/// all of it. Removes the session folders this empties. Returns how many
/// files it removed.
pub fn forget_history_backups(layout: &Layout, fs: &dyn Fs) -> io::Result<usize> {
    let mut removed = 0;
    for session in list_sessions(layout, fs)?.into_iter().filter(|s| s.class == Class::State) {
        removed += remove_history_files(fs, &session.path)?;
        if is_empty_tree(fs, &session.path)? {
            remove_tree(fs, &session.path)?;
        }
    }
    Ok(removed)
}

fn is_history_file(name: &str) -> bool {
    let base = match name.rsplit_once('.') {
        Some((base, n)) if n.parse::<u32>().is_ok() => base,
        _ => name,
    };
    matches!(base, "history.json" | "workspace.json")
}

fn remove_history_files(fs: &dyn Fs, dir: &Path) -> io::Result<usize> {
    let mut removed = 0;
    for name in fs.list(dir)? {
        let path = dir.join(&name);
        match fs.kind(&path)? {
            Kind::Dir => removed += remove_history_files(fs, &path)?,
            Kind::File if is_history_file(&name.to_string_lossy()) => {
                fs.remove_file(&path)?;
                removed += 1;
            }
            _ => {}
        }
    }
    if removed > 0 {
        fs.sync_dir(dir)?;
    }
    Ok(removed)
}

fn is_empty_tree(fs: &dyn Fs, dir: &Path) -> io::Result<bool> {
    for name in fs.list(dir)? {
        let path = dir.join(name);
        match fs.kind(&path)? {
            Kind::Dir if is_empty_tree(fs, &path)? => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_fs::MemFs;

    fn backups(l: &Layout, fs: &Arc<MemFs>, seconds: u64) -> Backups {
        let shared: Arc<dyn Fs + Send + Sync> = fs.clone();
        Backups::new(Arc::new(l.clone()), shared, at(seconds))
    }

    fn layout() -> Layout {
        Layout::portable(Path::new("/p"))
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn stamps_are_utc_and_valid_file_names_everywhere() {
        assert_eq!(backup_stamp(0), "1970-01-01T00-00-00Z");
        assert_eq!(backup_stamp(1_791_224_462), "2026-10-05T18-21-02Z");
        assert_eq!(backup_stamp(951_782_400), "2000-02-29T00-00-00Z");
        for seconds in [0, 59, 86_399, 951_782_400, 1_791_224_462, 4_102_444_799] {
            let stamp = backup_stamp(seconds);
            assert!(!stamp.contains(':'));
            assert_eq!(stamp_seconds(&stamp), Some(seconds), "{stamp}");
            assert_eq!(stamp_seconds(&format!("{stamp}.3")), Some(seconds));
        }
        for other in ["", "notes", "2026-13-05T18-21-02Z", "2026-02-30T00-00-00Z", "2026-10-05T18-21-02Z.x", "2026-10-05 18-21-02Z"] {
            assert_eq!(stamp_seconds(other), None, "{other}");
        }
    }

    #[test]
    fn a_moved_backup_keeps_its_place_below_the_root_and_never_overwrites() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        fs.seed_file("/p/state/session.json", b"one");
        let backups = backups(&l, &fs, 1_791_224_462);
        let first = backups.move_into(Path::new("/p/state/session.json")).expect("backup");
        assert_eq!(first, Path::new("/p/state/backups/2026-10-05T18-21-02Z/session.json"));
        fs.seed_file("/p/state/session.json", b"two");
        let second = backups.move_into(Path::new("/p/state/session.json")).expect("backup");
        assert_eq!(second, Path::new("/p/state/backups/2026-10-05T18-21-02Z/session.json.1"));
        let files = fs.files();
        assert_eq!(files[&first], b"one");
        assert_eq!(files[&second], b"two");
        assert!(!files.contains_key(Path::new("/p/state/session.json")));
        // Everything is durable: a power cut keeps both backups.
        fs.power_cut(|_| false);
        assert_eq!(fs.files().get(&second).map(Vec::as_slice), Some(&b"two"[..]));
    }

    #[test]
    fn two_starts_in_one_second_get_two_sessions() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        fs.seed_file("/p/config/settings.toml", b"a");
        let first = backups(&l, &fs, 100).move_into(Path::new("/p/config/settings.toml")).expect("backup");
        fs.seed_file("/p/config/settings.toml", b"b");
        let second = backups(&l, &fs, 100).move_into(Path::new("/p/config/settings.toml")).expect("backup");
        assert_eq!(first, Path::new("/p/config/backups/1970-01-01T00-01-40Z/settings.toml"));
        assert_eq!(second, Path::new("/p/config/backups/1970-01-01T00-01-40Z.1/settings.toml"));
    }

    #[test]
    fn a_backup_loses_nothing_at_any_crash() {
        let start = MemFs::new();
        start.seed_file("/p/state/history.json", b"visits");
        let l = layout();
        let total = {
            let probe = Arc::new(start.clone());
            probe.reset_ops();
            backups(&l, &probe, 7).move_into(Path::new("/p/state/history.json")).expect("backup");
            probe.ops()
        };
        for k in 0..=total {
            let fs = Arc::new(start.clone());
            fs.fail_after(k);
            let _ = backups(&l, &fs, 7).move_into(Path::new("/p/state/history.json"));
            fs.revive();
            for keep in fs.power_cuts() {
                // A fork: each power cut starts from the same crash.
                let cut = MemFs::clone(&fs);
                cut.power_cut(|i| keep[i]);
                let kept = cut.files().values().filter(|bytes| bytes.as_slice() == b"visits").count();
                assert!(kept >= 1, "crash after {k} operations, power cut keeping {keep:?}: the history is lost");
            }
        }
    }

    #[test]
    fn a_copy_leaves_the_original_in_place() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        fs.seed_file("/p/data/site-data/ab.gzs", b"records");
        let copy = backups(&l, &fs, 0)
            .copy_into(Path::new("/p/data/site-data/ab.gzs"), b"records")
            .expect("copy");
        assert_eq!(copy, Path::new("/p/data/backups/1970-01-01T00-00-00Z/site-data/ab.gzs"));
        assert_eq!(fs.files()[Path::new("/p/data/site-data/ab.gzs")], b"records");
        assert_eq!(fs.mode(&copy).expect("mode"), Some(PRIVATE_FILE_MODE));
    }

    #[test]
    fn files_outside_the_profile_are_backed_up_where_asked() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        fs.seed_file("/old/f1r3gaze/settings.conf", b"observers =\n");
        let kept = backups(&l, &fs, 0)
            .move_into_at(Path::new("/old/f1r3gaze/settings.conf"), Class::Config, Path::new("legacy-profile/settings.conf"))
            .expect("backup");
        assert_eq!(kept, Path::new("/p/config/backups/1970-01-01T00-00-00Z/legacy-profile/settings.conf"));
        assert!(backups(&l, &fs, 0).move_into(Path::new("/elsewhere/x")).is_err());
    }

    #[test]
    fn clearing_history_removes_only_history_backups() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        fs.seed_file("/p/state/backups/1970-01-01T00-00-00Z/history.json", b"h");
        fs.seed_file("/p/state/backups/1970-01-01T00-00-00Z/history.json.1", b"h");
        fs.seed_file("/p/state/backups/1970-01-01T00-00-00Z/session.json", b"s");
        fs.seed_file("/p/state/backups/1970-01-01T00-00-01Z/legacy-profile/workspace.json", b"w");
        fs.seed_file("/p/config/backups/1970-01-01T00-00-00Z/history.json", b"not state");
        assert_eq!(forget_history_backups(&l, fs.as_ref()).expect("forget"), 3);
        let left: Vec<PathBuf> = fs.files().into_keys().collect();
        assert_eq!(
            left,
            [
                PathBuf::from("/p/config/backups/1970-01-01T00-00-00Z/history.json"),
                PathBuf::from("/p/state/backups/1970-01-01T00-00-00Z/session.json"),
            ]
        );
        assert!(!fs.dirs().contains(&PathBuf::from("/p/state/backups/1970-01-01T00-00-01Z")), "an emptied session is removed");
    }

    #[test]
    fn pruning_removes_only_old_sessions() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        let day = 86_400;
        fs.seed_file(format!("/p/config/backups/{}/settings.toml", backup_stamp(10 * day)), b"old");
        fs.seed_file(format!("/p/state/backups/{}/session.json", backup_stamp(29 * day)), b"recent");
        fs.seed_file("/p/state/backups/kept-by-hand/notes.txt", b"mine");
        let listed = list_sessions(&l, fs.as_ref()).expect("list");
        assert_eq!(listed.len(), 3);
        assert_eq!(listed.iter().map(|s| s.files).sum::<usize>(), 3);
        let removed = prune_sessions(&l, fs.as_ref(), Duration::from_secs(14 * day), at(30 * day)).expect("prune");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].class, Class::Config);
        assert_eq!(list_sessions(&l, fs.as_ref()).expect("list").len(), 2);
    }

    #[test]
    fn a_damaged_store_is_copied_and_reported_before_it_is_cut() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        let report = Report::silent();
        let salvage = StoreSalvage::new(backups(&l, &fs, 0), report.clone(), "https://a.example:443");
        let path = Path::new("/p/data/site-data/ab.gzs");
        fs.seed_file(path, b"records and a torn tail");
        let torn = gaze_store::Damage { path, bytes: b"records and a torn tail", kept: 7, torn_tail: true };
        gaze_store::Salvage::keep(&salvage, &torn).expect("kept");
        let copy = PathBuf::from("/p/data/backups/1970-01-01T00-00-00Z/site-data/ab.gzs");
        assert_eq!(fs.files()[&copy], b"records and a torn tail");
        let unreadable = gaze_store::Damage { path, bytes: b"bad", kept: 0, torn_tail: false };
        gaze_store::Salvage::keep(&salvage, &unreadable).expect("kept");
        assert_eq!(fs.files()[&PathBuf::from("/p/data/backups/1970-01-01T00-00-00Z/site-data/ab.gzs.1")], b"bad");
        let events = report.events();
        let got: Vec<(&EventKind, Severity, Option<&Path>)> =
            events.iter().map(|e| (&e.kind, e.severity, e.backup.as_deref())).collect();
        assert_eq!(
            got,
            [
                (
                    &EventKind::Salvaged { site: "https://a.example:443".into(), kept: 7, torn_tail: true },
                    Severity::Notice,
                    Some(copy.as_path())
                ),
                (
                    &EventKind::Salvaged { site: "https://a.example:443".into(), kept: 0, torn_tail: false },
                    Severity::Warning,
                    Some(Path::new("/p/data/backups/1970-01-01T00-00-00Z/site-data/ab.gzs.1"))
                ),
            ]
        );
    }

    #[test]
    fn a_store_that_cannot_be_backed_up_is_left_alone() {
        let fs = Arc::new(MemFs::new());
        let l = layout();
        // The backups folder's place is taken by a file: no backup can be made.
        fs.seed_file("/p/data/backups", b"not a folder");
        let salvage = StoreSalvage::new(backups(&l, &fs, 0), Report::silent(), "s");
        let path = Path::new("/p/data/site-data/ab.gzs");
        let damage = gaze_store::Damage { path, bytes: b"x", kept: 0, torn_tail: false };
        assert!(gaze_store::Salvage::keep(&salvage, &damage).is_err(), "the store is then not opened, so not cut");
    }

    #[test]
    fn a_real_store_keeps_its_records_and_its_damage_is_backed_up() {
        use k1ndl1ng_norm::Norm;
        let root = gaze_fs::scratch_dir("gaze-shell-store-salvage");
        let l = Layout::portable(&root);
        let path = l.site_data_dir().join("ab.gzs");
        {
            let mut store = gaze_store::OriginStore::open(&path, 1 << 20, &gaze_store::Discard).expect("open");
            store.put("todo/1", &Norm::str("milk")).expect("put");
        }
        let mut damaged = std::fs::read(&path).expect("the store");
        damaged.extend_from_slice(&[0xff, 0x00, 0x13]);
        std::fs::write(&path, &damaged).expect("damage the store");
        let report = Report::silent();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(gaze_fs::StdFs);
        let salvage = StoreSalvage::new(Backups::new(Arc::new(l.clone()), fs, at(0)), report.clone(), "https://a.example:443");
        let store = gaze_store::OriginStore::open(&path, 1 << 20, &salvage).expect("open the damaged store");
        assert_eq!(store.get("todo/1"), Some(Norm::str("milk")), "the readable records stay");
        let events = report.events();
        assert_eq!(events.len(), 1, "{events:?}");
        let copy = events[0].backup.clone().expect("a backup");
        assert!(copy.starts_with(l.data.join("backups")), "{}", copy.display());
        assert_eq!(std::fs::read(&copy).expect("the copy"), damaged, "the whole damaged file is kept");
    }
}
