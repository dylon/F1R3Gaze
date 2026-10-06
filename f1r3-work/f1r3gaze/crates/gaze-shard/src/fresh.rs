//! Freshness records (spec §9.4, resolution step 4; proposal §8.2,
//! "Freshness"): for each binding, the highest finalized block at which the
//! browser has seen it. An answer read at an older block is a replay of
//! superseded state and is refused.
//!
//! A record that lived only in memory would be forgotten at every restart,
//! and a restart would then let an attacker replay a stale binding. So the
//! records are kept on disk ([`FileFreshness`], `data/trust/freshness.tsv`),
//! except for a development shard on this machine ([`MemFreshness`]), which
//! is reset often: persisted records would then refuse every site.

use gaze_fs::{Fs, LineCheck, Perm, StdFs};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where the bridge keeps its records.
pub trait FreshnessLog: Send + Sync + 'static {
    /// The records kept so far: the highest finalized block seen, by binding.
    fn load(&self) -> BTreeMap<String, i64>;
    /// Saves the records after one of them rose.
    fn record(&self, all: &BTreeMap<String, i64>) -> Result<(), String>;
}

/// Records kept only for the life of the process.
#[derive(Default)]
pub struct MemFreshness(Mutex<BTreeMap<String, i64>>);

impl FreshnessLog for MemFreshness {
    fn load(&self) -> BTreeMap<String, i64> {
        self.0.lock().map(|m| m.clone()).unwrap_or_default()
    }

    fn record(&self, all: &BTreeMap<String, i64>) -> Result<(), String> {
        let mut kept = self.0.lock().map_err(|_| "the freshness records' lock is poisoned")?;
        kept.clone_from(all);
        Ok(())
    }
}

/// The first line of the file, which says what the columns hold.
const HEADER: &str = "# F1R3Gaze freshness records: shard, binding, highest finalized block seen\n";

/// Records kept in a file, one line per binding: `shard TAB binding TAB
/// block`, with `%`, tab, carriage return and line feed percent-escaped.
/// Each shard's records are separate: the lines of other shards are kept as
/// they are.
pub struct FileFreshness {
    path: PathBuf,
    shard: String,
    /// The usable lines of other shards, verbatim.
    others: Vec<String>,
    /// This shard's records as read.
    mine: BTreeMap<String, i64>,
    /// Why the file cannot be written, if it could not be read: its records
    /// then live in memory only, and the file is left as it is.
    blocked: Option<String>,
}

impl FileFreshness {
    /// Reads the records at `path` for `shard`. A missing file is empty; a
    /// file that cannot be read is never overwritten (see
    /// [`FileFreshness::blocked`]). Lines that cannot be used are skipped;
    /// start-up has already saved and repaired a damaged file.
    pub fn open(path: impl Into<PathBuf>, shard: &str) -> FileFreshness {
        let path = path.into();
        let (text, blocked) = match StdFs.read(&path) {
            Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
            Err(e) => (String::new(), Some(format!("{}: {e}", path.display()))),
        };
        let mut others = Vec::new();
        let mut mine = BTreeMap::new();
        for line in text.lines() {
            match parse_line(line) {
                Ok(Some((row_shard, binding, block))) if row_shard == shard => {
                    let highest = mine.entry(binding).or_insert(block);
                    *highest = (*highest).max(block);
                }
                Ok(Some(_)) => others.push(line.to_string()),
                Ok(None) | Err(_) => {}
            }
        }
        FileFreshness {
            path,
            shard: shard.to_string(),
            others,
            mine,
            blocked,
        }
    }

    /// The same records, enforced but never saved: for a session that does
    /// not hold the profile's lock, or cannot write the profile. An answer
    /// older than a loaded record is still refused; a record that rises is
    /// kept in memory only, and [`FreshnessLog::record`] says why.
    pub fn read_only(mut self, why: &str) -> FileFreshness {
        self.blocked.get_or_insert_with(|| why.to_string());
        self
    }

    /// Why the records cannot be saved, if so.
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Forgets this shard's records named by `which`, for a shard that was
    /// reset (`f1r3gaze trust forget`). Other shards' lines stay as they
    /// are. Returns how many records were removed, writing the file only if
    /// some were. Refused when the file could not be read.
    pub fn forget(&mut self, which: Forget<'_>) -> Result<usize, String> {
        if let Some(why) = &self.blocked {
            return Err(format!("freshness records cannot be changed: {why}"));
        }
        let before = self.mine.len();
        match which {
            Forget::All => self.mine.clear(),
            Forget::Binding(binding) => {
                self.mine.remove(binding);
            }
        }
        let removed = before - self.mine.len();
        if removed > 0 {
            self.record(&self.mine)?;
        }
        Ok(removed)
    }
}

/// Which records [`FileFreshness::forget`] removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forget<'a> {
    /// Every record of the shard.
    All,
    /// The record of one binding.
    Binding(&'a str),
}

impl FreshnessLog for FileFreshness {
    fn load(&self) -> BTreeMap<String, i64> {
        self.mine.clone()
    }

    fn record(&self, all: &BTreeMap<String, i64>) -> Result<(), String> {
        if let Some(why) = &self.blocked {
            return Err(format!("freshness records are kept in memory only: {why}"));
        }
        let mut text = String::with_capacity(HEADER.len() + 64 * (self.others.len() + all.len()));
        text.push_str(HEADER);
        for line in &self.others {
            text.push_str(line);
            text.push('\n');
        }
        for (binding, block) in all {
            text.push_str(&format!("{}\t{}\t{block}\n", escape(&self.shard), escape(binding)));
        }
        let dir = gaze_fs::parent(&self.path).map_err(|e| e.to_string())?;
        gaze_fs::create_dir_durably(&StdFs, dir)
            .and_then(|_| gaze_fs::write_atomic(&StdFs, &self.path, text.as_bytes(), Perm::Private))
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// Which lines of a freshness file can be used: the header and other
/// comments, and `shard TAB binding TAB block` rows.
pub fn check_records(bytes: &[u8]) -> LineCheck {
    gaze_fs::check_lines(bytes, |line| parse_line(line).map(drop))
}

/// A row as `(shard, binding, block)`, `None` for a comment.
fn parse_line(line: &str) -> Result<Option<(String, String, i64)>, String> {
    if line.starts_with('#') {
        return Ok(None);
    }
    let fields: Vec<&str> = line.split('\t').collect();
    let [shard, binding, block] = fields.as_slice() else {
        return Err(format!("{} fields, not 3", fields.len()));
    };
    let block: i64 = block.parse().map_err(|_| format!("block {block:?} is not a number"))?;
    match block >= 0 {
        true => Ok(Some((unescape(shard)?, unescape(binding)?, block))),
        false => Err(format!("block {block} is negative")),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let code = rest.get(at + 1..at + 3).ok_or("a % escape is cut short")?;
        out.push(match code {
            "25" => '%',
            "09" => '\t',
            "0D" => '\r',
            "0A" => '\n',
            other => return Err(format!("%{other} is not an escape")),
        });
        rest = &rest[at + 3..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn records_round_trip_per_shard() {
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh");
        let path = dir.join("trust/freshness.tsv");
        StdFs.create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let root = FileFreshness::open(&path, "root");
        root.record(&records(&[("rho:serve:a", 12), ("rho:serve:b", 7)])).expect("save root");
        let test = FileFreshness::open(&path, "test");
        assert!(test.load().is_empty(), "another shard's records are separate");
        test.record(&records(&[("rho:serve:a", 99)])).expect("save test");
        assert_eq!(FileFreshness::open(&path, "root").load(), records(&[("rho:serve:a", 12), ("rho:serve:b", 7)]));
        assert_eq!(FileFreshness::open(&path, "test").load(), records(&[("rho:serve:a", 99)]));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.starts_with(HEADER), "{text}");
    }

    #[test]
    fn duplicates_merge_to_the_highest_block() {
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-merge");
        let path = dir.join("freshness.tsv");
        std::fs::write(&path, "root\tb\t5\nroot\tb\t9\nroot\tb\t7\n").expect("seed");
        assert_eq!(FileFreshness::open(&path, "root").load(), records(&[("b", 9)]));
    }

    #[test]
    fn odd_bindings_round_trip() {
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-odd");
        let path = dir.join("freshness.tsv");
        let odd = "tab\there, percent % and\nnew line\r";
        FileFreshness::open(&path, "root").record(&records(&[(odd, 3)])).expect("save");
        assert_eq!(FileFreshness::open(&path, "root").load(), records(&[(odd, 3)]));
    }

    #[test]
    fn damaged_lines_are_found() {
        let check = check_records(b"# header\nroot\tb\t5\nroot\tb\nroot\tc\tx\nroot\td\t-1\nroot\t%zz\t1\n");
        let lines: Vec<usize> = check.dropped.iter().map(|(n, _)| *n).collect();
        assert_eq!(lines, [3, 4, 5, 6]);
        assert_eq!(check.kept, b"# header\nroot\tb\t5\n");
    }

    #[test]
    fn forgetting_removes_only_this_shards_records() {
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-forget");
        let path = dir.join("freshness.tsv");
        FileFreshness::open(&path, "root").record(&records(&[("a", 1), ("b", 2), ("c", 3)])).expect("save root");
        FileFreshness::open(&path, "test").record(&records(&[("a", 9)])).expect("save test");
        let mut root = FileFreshness::open(&path, "root");
        assert_eq!(root.forget(Forget::Binding("b")), Ok(1));
        assert_eq!(FileFreshness::open(&path, "root").load(), records(&[("a", 1), ("c", 3)]));
        let before = std::fs::read(&path).expect("read");
        assert_eq!(root.forget(Forget::Binding("missing")), Ok(0));
        assert_eq!(std::fs::read(&path).expect("read"), before, "nothing forgotten, nothing written");
        assert_eq!(root.forget(Forget::All), Ok(2));
        assert!(FileFreshness::open(&path, "root").load().is_empty());
        assert_eq!(FileFreshness::open(&path, "test").load(), records(&[("a", 9)]), "another shard's records stay");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_records_are_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-blocked");
        let path = dir.join("freshness.tsv");
        std::fs::write(&path, "root\tb\t5\n").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read(&path).is_ok() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
            return;
        }
        let mut log = FileFreshness::open(&path, "root");
        assert!(log.blocked().is_some());
        assert!(log.record(&records(&[("b", 6)])).is_err());
        assert!(log.forget(Forget::All).is_err(), "nothing is forgotten in a file that could not be read");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "root\tb\t5\n");
    }

    /// A read-only session keeps enforcing what the file holds, and never
    /// writes it.
    #[test]
    fn a_read_only_log_enforces_and_never_writes() {
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-read-only");
        let path = dir.join("freshness.tsv");
        FileFreshness::open(&path, "root").record(&records(&[("a", 4), ("b", 9)])).expect("save");
        let before = std::fs::read(&path).expect("read");
        let mut log = FileFreshness::open(&path, "root").read_only("another F1R3Gaze holds the profile");
        assert_eq!(log.load(), records(&[("a", 4), ("b", 9)]), "the records are enforced");
        assert_eq!(log.blocked(), Some("another F1R3Gaze holds the profile"));
        let refused = log.record(&records(&[("a", 5), ("b", 9)])).expect_err("a read-only log never saves");
        assert!(refused.contains("another F1R3Gaze holds the profile"), "{refused}");
        let forgot = log.forget(Forget::All).expect_err("nothing is forgotten read-only");
        assert!(forgot.contains("another F1R3Gaze holds the profile"), "{forgot}");
        assert_eq!(std::fs::read(&path).expect("read"), before, "the file is unchanged");
        let missing = FileFreshness::open(dir.join("missing.tsv"), "root").read_only("read-only");
        assert!(missing.record(&records(&[("a", 1)])).is_err());
        assert!(!dir.join("missing.tsv").exists(), "a missing file stays missing");
    }

    /// A file that could not be read keeps its own reason: being read-only
    /// as well does not hide it.
    #[cfg(unix)]
    #[test]
    fn read_only_keeps_the_first_reason() {
        use std::os::unix::fs::PermissionsExt;
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-read-only-blocked");
        let path = dir.join("freshness.tsv");
        std::fs::write(&path, "root\tb\t5\n").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read(&path).is_ok() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
            return;
        }
        let log = FileFreshness::open(&path, "root").read_only("read-only");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let why = log.blocked().expect("blocked");
        assert!(why.contains("freshness.tsv"), "{why}");
    }

    #[cfg(unix)]
    #[test]
    fn the_records_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = gaze_fs::scratch_dir("gaze-shard-fresh-private");
        let path = dir.join("freshness.tsv");
        FileFreshness::open(&path, "root").record(&records(&[("b", 1)])).expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
