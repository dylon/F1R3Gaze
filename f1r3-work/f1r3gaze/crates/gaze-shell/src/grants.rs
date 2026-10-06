//! The remembered permission decisions: `permissions/grants.tsv`, one line
//! per decision in `gaze_broker`'s format ([`gaze_broker::format_grant_line`]).
//!
//! The file is written atomically and synced, readable by its owner only. A
//! file that exists but cannot be read is never overwritten: saves are
//! refused until it can be read (see [`FileGrants::blocked`]). Start-up has
//! already saved and repaired a file it could read but not wholly parse;
//! lines that still do not parse are skipped here.

use gaze_broker::{GrantStore, Stored, format_grant_line, parse_grant_line};
use gaze_fs::{Fs, LineCheck, Perm, StdFs};
use std::cell::OnceCell;
use std::path::{Path, PathBuf};

/// Which lines of a grants file can be used.
pub fn check_grants(bytes: &[u8]) -> LineCheck {
    gaze_fs::check_lines(bytes, |line| parse_grant_line(line).map(drop))
}

/// The decisions kept in a file.
pub struct FileGrants {
    path: PathBuf,
    /// Why saves are refused, if they are: set by a load that could not
    /// read the file, or by [`FileGrants::read_only`].
    blocked: OnceCell<String>,
}

impl FileGrants {
    pub fn new(path: impl AsRef<Path>) -> FileGrants {
        FileGrants {
            path: path.as_ref().to_path_buf(),
            blocked: OnceCell::new(),
        }
    }

    /// Decisions that are read but never saved: for a session that does not
    /// hold the profile's lock.
    pub fn read_only(self, why: &str) -> FileGrants {
        let _ = self.blocked.set(why.to_string());
        self
    }

    /// Why decisions are not being saved, if they are not.
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.get().map(String::as_str)
    }
}

impl GrantStore for FileGrants {
    fn load(&self) -> Vec<Stored> {
        match StdFs.read(&self.path) {
            Ok(bytes) => {
                let usable = check_grants(&bytes).kept;
                String::from_utf8_lossy(&usable)
                    .lines()
                    .filter_map(|line| parse_grant_line(line).ok())
                    .collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                let _ = self.blocked.set(format!("{}: {e}", self.path.display()));
                Vec::new()
            }
        }
    }

    fn save(&mut self, all: &[Stored]) -> Result<(), String> {
        if let Some(why) = self.blocked.get() {
            return Err(format!("permission decisions are not saved: {why}"));
        }
        let mut out = String::with_capacity(all.len() * 160);
        for stored in all {
            out.push_str(&format_grant_line(stored));
            out.push('\n');
        }
        let dir = gaze_fs::parent(&self.path).map_err(|e| e.to_string())?;
        gaze_fs::create_dir_durably(&StdFs, dir)
            .and_then(|_| gaze_fs::write_atomic(&StdFs, &self.path, out.as_bytes(), Perm::Private))
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_broker::{Broker, SHARD, ShardClass, Site};
    use gaze_knf::Knf;

    fn knf(src: &str) -> Knf {
        Knf::from_source(src, k1ndl1ng_parse::Level::K1G, &[]).expect("a program")
    }

    #[test]
    fn file_grants_round_trip_through_the_broker() {
        let path = gaze_fs::scratch_dir("gaze-grants-round-trip").join("permissions/grants.tsv");
        let site = Site::of_url("https://a.example/").expect("a site");
        let k = knf("shard!(3)");
        {
            let mut b = Broker::new(FileGrants::new(&path));
            let mut p = b.plan(&site, &k);
            b.answer(&mut p, SHARD, true, true).expect("answer");
            b.install(1, &p);
            b.allow_shard(1, ShardClass::Deploy, Some(k.grant_hash())).expect("allow");
        }
        let b = Broker::new(FileGrants::new(&path));
        assert!(b.plan(&site, &k).granted(SHARD));
        assert!(b.remembered_all()[0].classes.contains(&ShardClass::Deploy));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let names: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("grants.tsv")], "no temporary file is left");
    }

    #[test]
    fn an_unreadable_grants_file_is_never_overwritten() {
        let dir = gaze_fs::scratch_dir("gaze-grants-blocked");
        // A directory where the file should be: present, and unreadable as a file.
        let path = dir.join("grants.tsv");
        std::fs::create_dir(&path).expect("mkdir");
        let mut store = FileGrants::new(&path);
        assert!(store.load().is_empty());
        assert!(store.blocked().is_some());
        let refused = store.save(&[]).expect_err("refused");
        assert!(refused.contains("not saved"), "{refused}");
        assert!(path.is_dir(), "left as it was");
    }

    #[test]
    fn a_read_only_store_saves_nothing() {
        let path = gaze_fs::scratch_dir("gaze-grants-read-only").join("grants.tsv");
        let mut store = FileGrants::new(&path).read_only("another F1R3Gaze holds the profile");
        assert!(store.save(&[]).expect_err("refused").contains("another F1R3Gaze"));
        assert!(!path.exists());
    }

    #[test]
    fn every_usable_grant_line_is_kept() {
        let good = format!("https://a.example\t{}\trho:gaze:net\tallow\t", "ab".repeat(32));
        let bytes = format!("{good}\nbad line\n\n{good}\r\n").into_bytes();
        let check = check_grants(&bytes);
        assert_eq!(check.dropped.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [2]);
        assert_eq!(check.kept, format!("{good}\n\n{good}\r\n").into_bytes());
    }
}
