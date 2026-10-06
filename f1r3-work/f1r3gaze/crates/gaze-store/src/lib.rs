//! `gaze-store` — the `store` capability (spec §8.2).
//!
//! One append-only file per origin. Records:
//!
//! ```text
//! 0x01 put  LEB key-len, key (UTF-8), LEB value-len, value (K1G encoding)
//! 0x02 del  LEB key-len, key
//! ```
//!
//! On open the file is replayed into memory. A torn final record (a crash
//! mid-append), or a record that cannot be read, ends the replay: the
//! caller's [`Salvage`] keeps a copy of the whole file first, and only then
//! is the file cut back to the records before it. If the copy cannot be
//! made, the store is not opened and the file is left as it is. A file that
//! cannot be read at all is never treated as empty.
//!
//! Appends are not synced, as with a browser's relaxed IndexedDB
//! durability: a process crash loses nothing, and a power cut may lose the
//! last records, which the next open drops as a torn tail. When dead bytes
//! exceed live bytes plus 64 KiB the file is compacted by writing a fresh
//! one and putting it in place with `gaze_fs::write_atomic`, synced, so a
//! power cut never leaves an empty or half-written store.
//!
//! Values are any closed normal form, except that unforgeable names are
//! refused anywhere inside them: a name must not outlive the tab that minted
//! it. Keys are strings of at most 1 KiB. The quota counts key and value
//! bytes.

#![forbid(unsafe_code)]
// Tests write their fixtures directly (clippy.toml's disallowed-methods
// apply to the code they test).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

use gaze_fs::{Perm, StdFs};
use k1ndl1ng_norm::term::leb;
use k1ndl1ng_norm::{Name, Node, Norm};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MAX_KEY: usize = 1024;
const COMPACT_SLACK: u64 = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    Quota,
    Key,
    /// The value holds an unforgeable name.
    Unforgeable,
    Io(String),
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::Quota => "quota",
            StoreError::Key | StoreError::Unforgeable => "type",
            StoreError::Io(_) => "io",
        }
    }
}

/// Does a term contain an unforgeable name anywhere, including inside quotes?
pub fn has_unforgeable(t: &Norm) -> bool {
    let mut procs = vec![t.clone()];
    let mut names: Vec<Name> = Vec::new();
    loop {
        while let Some(n) = names.pop() {
            match n {
                Name::Unforgeable(_) => return true,
                Name::Quote(q) => procs.push(q),
                _ => {}
            }
        }
        let Some(p) = procs.pop() else { return false };
        match p.node() {
            Node::Par(v) => procs.extend(v.iter().cloned()),
            Node::Send { chan, args, .. } => {
                names.push(chan.clone());
                procs.extend(args.iter().cloned());
            }
            Node::Receive { binds, body } => {
                for b in binds {
                    names.push(b.chan.clone());
                }
                procs.push(body.clone());
            }
            Node::New { body, .. } => procs.push(body.clone()),
            Node::Eval(n) => names.push(n.clone()),
            Node::Coll { items, .. } => procs.extend(items.iter().cloned()),
            Node::CollRest { items, rest, .. } => {
                procs.extend(items.iter().cloned());
                procs.push(rest.clone());
            }
            _ => {}
        }
    }
}

pub struct OriginStore {
    path: PathBuf,
    file: File,
    map: BTreeMap<String, Vec<u8>>,
    live: u64,
    file_len: u64,
    quota: u64,
}

fn read_leb(b: &[u8], i: &mut usize) -> Option<usize> {
    let mut out = 0usize;
    for s in 0..5 {
        let c = *b.get(*i)?;
        *i += 1;
        out |= ((c & 0x7f) as usize) << (7 * s);
        if c & 0x80 == 0 {
            return Some(out);
        }
    }
    None
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Io(e.to_string())
}

/// A store file whose end cannot be replayed, before anything is cut from
/// it.
#[derive(Debug)]
pub struct Damage<'a> {
    pub path: &'a Path,
    /// The whole file as it was read.
    pub bytes: &'a [u8],
    /// How many leading bytes hold readable records, and stay.
    pub kept: u64,
    /// Whether the rest is only an unfinished final record (a crash during
    /// an append) rather than a record that cannot be read.
    pub torn_tail: bool,
}

/// Keeps a copy of a damaged store file before [`OriginStore::open`] cuts
/// it back. An error leaves the file untouched and the store unopened.
pub trait Salvage {
    fn keep(&self, damage: &Damage<'_>) -> Result<(), String>;
}

/// Keeps no copy: for stores whose damage is not worth keeping, and tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct Discard;

impl Salvage for Discard {
    fn keep(&self, _damage: &Damage<'_>) -> Result<(), String> {
        Ok(())
    }
}

/// One record of the log.
enum Record {
    Put(String, Vec<u8>),
    Del(String),
}

/// Why the replay stopped before the end of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// The bytes end inside a record: a crash during an append.
    Torn,
    /// A record that cannot be read: an unknown tag, a key that is not
    /// UTF-8, or a length that does not fit.
    Unreadable,
}

/// The record at `bytes[at..]`, and where the next one starts.
fn record(bytes: &[u8], at: usize) -> Result<(Record, usize), Stop> {
    let mut j = at;
    let op = *bytes.get(j).ok_or(Stop::Torn)?;
    j += 1;
    if !matches!(op, 1 | 2) {
        return Err(Stop::Unreadable);
    }
    let key_len = leb_at(bytes, &mut j)?;
    let key = bytes.get(j..j + key_len).ok_or(Stop::Torn)?;
    let key = std::str::from_utf8(key).map_err(|_| Stop::Unreadable)?.to_string();
    j += key_len;
    match op {
        1 => {
            let value_len = leb_at(bytes, &mut j)?;
            let value = bytes.get(j..j + value_len).ok_or(Stop::Torn)?.to_vec();
            j += value_len;
            Ok((Record::Put(key, value), j))
        }
        _ => Ok((Record::Del(key), j)),
    }
}

/// A LEB128 length at `bytes[*j..]`: `Torn` if the bytes end inside it,
/// `Unreadable` if it is longer than five bytes.
fn leb_at(bytes: &[u8], j: &mut usize) -> Result<usize, Stop> {
    match read_leb(bytes, j) {
        Some(n) => Ok(n),
        None if *j >= bytes.len() => Err(Stop::Torn),
        None => Err(Stop::Unreadable),
    }
}

impl OriginStore {
    /// Opens the store at `path`, creating it (and its directory, owner-only)
    /// if it does not exist. A damaged file is handed to `salvage` before it
    /// is cut back to its readable records; see the module documentation.
    pub fn open(
        path: impl AsRef<Path>,
        quota: u64,
        salvage: &dyn Salvage,
    ) -> Result<OriginStore, StoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(d) = path.parent() {
            gaze_fs::create_dir_durably(&StdFs, d).map_err(io)?;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io(e)),
        };
        let mut map = BTreeMap::new();
        let mut good = 0;
        let mut stop = None;
        while good < bytes.len() {
            match record(&bytes, good) {
                Ok((Record::Put(k, v), next)) => {
                    map.insert(k, v);
                    good = next;
                }
                Ok((Record::Del(k), next)) => {
                    map.remove(&k);
                    good = next;
                }
                Err(why) => {
                    stop = Some(why);
                    break;
                }
            }
        }
        if let Some(why) = stop {
            let damage = Damage {
                path: &path,
                bytes: &bytes,
                kept: good as u64,
                torn_tail: why == Stop::Torn,
            };
            salvage.keep(&damage).map_err(|e| {
                StoreError::Io(format!(
                    "{}: damaged after byte {good}, and could not be saved before repair, so it was left as it is: {e}",
                    path.display()
                ))
            })?;
            // Cut back to the readable records before appending more.
            let f = OpenOptions::new().write(true).open(&path).map_err(io)?;
            f.set_len(good as u64).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path).map_err(io)?;
        let live = map.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum();
        Ok(OriginStore {
            path,
            file,
            map,
            live,
            file_len: good as u64,
            quota,
        })
    }

    pub fn get(&self, k: &str) -> Option<Norm> {
        Norm::decode(self.map.get(k)?).ok()
    }

    pub fn list(&self, prefix: &str) -> Vec<String> {
        self.map.range(prefix.to_string()..).take_while(|(k, _)| k.starts_with(prefix)).map(|(k, _)| k.clone()).collect()
    }

    pub fn used(&self) -> u64 {
        self.live
    }

    pub fn put(&mut self, k: &str, v: &Norm) -> Result<(), StoreError> {
        if k.is_empty() || k.len() > MAX_KEY {
            return Err(StoreError::Key);
        }
        if has_unforgeable(v) {
            return Err(StoreError::Unforgeable);
        }
        let enc = v.encode();
        let old = self.map.get(k).map(|o| (k.len() + o.len()) as u64).unwrap_or(0);
        let new_live = self.live - old + (k.len() + enc.len()) as u64;
        if new_live > self.quota {
            return Err(StoreError::Quota);
        }
        let mut rec = vec![1u8];
        leb(k.len() as u32, &mut rec);
        rec.extend_from_slice(k.as_bytes());
        leb(enc.len() as u32, &mut rec);
        rec.extend_from_slice(enc);
        self.append(&rec)?;
        self.map.insert(k.to_string(), enc.to_vec());
        self.live = new_live;
        self.maybe_compact()
    }

    pub fn del(&mut self, k: &str) -> Result<(), StoreError> {
        let Some(old) = self.map.remove(k) else { return Ok(()) };
        self.live -= (k.len() + old.len()) as u64;
        let mut rec = vec![2u8];
        leb(k.len() as u32, &mut rec);
        rec.extend_from_slice(k.as_bytes());
        self.append(&rec)?;
        self.maybe_compact()
    }

    fn append(&mut self, rec: &[u8]) -> Result<(), StoreError> {
        self.file.write_all(rec).map_err(io)?;
        self.file.flush().map_err(io)?;
        self.file_len += rec.len() as u64;
        Ok(())
    }

    fn maybe_compact(&mut self) -> Result<(), StoreError> {
        if self.file_len <= 2 * self.live + COMPACT_SLACK {
            return Ok(());
        }
        let mut out = Vec::new();
        for (k, v) in &self.map {
            out.push(1u8);
            leb(k.len() as u32, &mut out);
            out.extend_from_slice(k.as_bytes());
            leb(v.len() as u32, &mut out);
            out.extend_from_slice(v);
        }
        // The compacted file replaces the log whole, durably: never empty,
        // never half-written, whatever the moment of a power cut.
        gaze_fs::write_atomic(&StdFs, &self.path, &out, Perm::Private).map_err(io)?;
        self.file = OpenOptions::new().append(true).open(&self.path).map_err(io)?;
        self.file_len = out.len() as u64;
        Ok(())
    }

    /// Serve one page request: `("get", k, ret)`, `("put", k, v, ack)`,
    /// `("del", k, ack)`, `("list", prefix, ret)`. Returns the reply datum
    /// for the last argument's name, if the request carried one.
    pub fn serve(&mut self, args: &[Norm]) -> Option<Norm> {
        let ok = |v: Norm| Norm::tuple(vec![Norm::str("ok"), v]);
        let err = |c: &str, d: &str| Norm::tuple(vec![Norm::str("err"), Norm::str(c), Norm::str(d)]);
        let verb = args.first()?.as_str()?;
        let key = args.get(1).and_then(|k| k.as_str());
        Some(match (verb, key) {
            ("get", Some(k)) => ok(self.get(k).unwrap_or_else(Norm::nil)),
            ("put", Some(k)) => match args.get(2).map(|v| self.put(k, v)) {
                Some(Ok(())) => ok(Norm::nil()),
                Some(Err(e)) => err(e.code(), k),
                None => err("type", verb),
            },
            ("del", Some(k)) => match self.del(k) {
                Ok(()) => ok(Norm::nil()),
                Err(e) => err(e.code(), k),
            },
            ("list", Some(p)) => ok(Norm::list(self.list(p).iter().map(|k| Norm::str(k)).collect())),
            ("get" | "put" | "del" | "list", None) => err("type", verb),
            _ => err("verb", verb),
        })
    }
}

/// Where each origin's file lives: `<root>/<hex of BLAKE2b-256(site)>.gzs`.
pub fn path_for(root: &Path, site: &str) -> PathBuf {
    let h = k1ndl1ng_norm::hash::blake2b_256(site.as_bytes()).0;
    let name: String = h.iter().map(|b| format!("{b:02x}")).collect();
    root.join(format!("{name}.gzs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k1ndl1ng_norm::CollKind;
    use std::cell::RefCell;

    fn tmp(name: &str) -> PathBuf {
        gaze_fs::scratch_dir(&format!("gaze-store-{name}")).join("store/s.gzs")
    }

    /// What a salvage was handed: how much is kept, whether the tail is only
    /// torn, the bytes, and the file's length on disk at that moment.
    #[derive(Debug, PartialEq, Eq)]
    struct Seen(u64, bool, Vec<u8>, u64);

    /// Records what it was handed, and answers as told.
    struct Recorder {
        seen: RefCell<Vec<Seen>>,
        answer: Result<(), String>,
    }

    impl Recorder {
        fn new(answer: Result<(), String>) -> Recorder {
            Recorder { seen: RefCell::new(Vec::new()), answer }
        }
    }

    impl Salvage for Recorder {
        fn keep(&self, damage: &Damage<'_>) -> Result<(), String> {
            let on_disk = std::fs::metadata(damage.path).expect("the damaged file").len();
            self.seen
                .borrow_mut()
                .push(Seen(damage.kept, damage.torn_tail, damage.bytes.to_vec(), on_disk));
            self.answer.clone()
        }
    }

    #[test]
    fn put_get_list_del_persist() {
        let p = tmp("basic");
        {
            let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
            s.put("todo/1", &Norm::str("milk")).expect("put");
            s.put("todo/2", &Norm::list(vec![Norm::int(1), Norm::bool(true)])).expect("put");
            s.put("other", &Norm::nil()).expect("put");
            s.del("other").expect("del");
            assert_eq!(s.list("todo/"), vec!["todo/1", "todo/2"]);
        }
        let s = OriginStore::open(&p, 1 << 20, &Discard).expect("reopen");
        assert_eq!(s.get("todo/1"), Some(Norm::str("milk")));
        assert!(s.get("other").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_store_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("private");
        OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        let dir = p.parent().expect("parent");
        let mode = std::fs::metadata(dir).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn names_quota_and_torn_tails() {
        let p = tmp("rules");
        let mut s = OriginStore::open(&p, 64, &Discard).expect("open");
        let name = Norm::eval(Name::Unforgeable([7; 32]));
        assert_eq!(s.put("k", &Norm::list(vec![name])), Err(StoreError::Unforgeable));
        assert_eq!(s.put("k", &Norm::str(&"x".repeat(100))), Err(StoreError::Quota));
        s.put("k", &Norm::int(1)).expect("put");
        drop(s);
        // A torn record at the end is dropped, not fatal.
        let mut f = OpenOptions::new().append(true).open(&p).expect("append");
        f.write_all(&[1, 5, b'a']).expect("tear");
        drop(f);
        let mut s = OriginStore::open(&p, 64, &Discard).expect("reopen");
        assert_eq!(s.get("k"), Some(Norm::int(1)));
        s.put("j", &Norm::int(2)).expect("put");
        drop(s);
        assert_eq!(OriginStore::open(&p, 64, &Discard).expect("reopen").get("j"), Some(Norm::int(2)));
    }

    #[test]
    fn a_torn_tail_is_kept_before_it_is_dropped() {
        let p = tmp("torn");
        let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        s.put("k", &Norm::int(1)).expect("put");
        drop(s);
        let good = std::fs::metadata(&p).expect("stat").len();
        let mut f = OpenOptions::new().append(true).open(&p).expect("append");
        f.write_all(&[1, 5, b'a']).expect("tear");
        drop(f);
        let whole = std::fs::read(&p).expect("read");
        let recorder = Recorder::new(Ok(()));
        let s = OriginStore::open(&p, 1 << 20, &recorder).expect("reopen");
        assert_eq!(s.get("k"), Some(Norm::int(1)));
        assert_eq!(*recorder.seen.borrow(), [Seen(good, true, whole.clone(), whole.len() as u64)]);
        assert_eq!(std::fs::metadata(&p).expect("stat").len(), good);
    }

    #[test]
    fn an_unreadable_record_is_kept_before_the_file_is_cut() {
        let p = tmp("unreadable");
        let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        s.put("a", &Norm::int(1)).expect("put");
        drop(s);
        let good = std::fs::metadata(&p).expect("stat").len();
        // An unknown tag, then a record that would have been readable.
        let mut damaged = std::fs::read(&p).expect("read");
        damaged.extend_from_slice(&[9, 1, b'x']);
        damaged.extend_from_slice(&[1, 1, b'b', 1, 0]);
        std::fs::write(&p, &damaged).expect("damage");
        let recorder = Recorder::new(Ok(()));
        let s = OriginStore::open(&p, 1 << 20, &recorder).expect("reopen");
        assert_eq!(s.get("a"), Some(Norm::int(1)));
        assert!(s.get("b").is_none(), "nothing after an unreadable record is replayed");
        // The salvage saw the whole file, still whole on disk.
        assert_eq!(
            *recorder.seen.borrow(),
            [Seen(good, false, damaged.clone(), damaged.len() as u64)]
        );
        assert_eq!(std::fs::metadata(&p).expect("stat").len(), good);
    }

    #[test]
    fn a_failed_salvage_leaves_the_file_alone() {
        let p = tmp("refused");
        let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        s.put("a", &Norm::int(1)).expect("put");
        drop(s);
        let mut damaged = std::fs::read(&p).expect("read");
        damaged.extend_from_slice(&[9, 9, 9]);
        std::fs::write(&p, &damaged).expect("damage");
        let refused = OriginStore::open(&p, 1 << 20, &Recorder::new(Err("disk full".into())));
        match refused {
            Err(StoreError::Io(why)) => assert!(why.contains("disk full"), "{why}"),
            Err(other) => panic!("unexpected error {other:?}"),
            Ok(_) => panic!("the store opened although it could not be saved"),
        }
        assert_eq!(std::fs::read(&p).expect("read"), damaged);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_store_is_not_opened_as_empty() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("no-access");
        let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        s.put("a", &Norm::int(1)).expect("put");
        drop(s);
        let before = std::fs::read(&p).expect("read");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read(&p).is_ok() {
            // Running with privileges that ignore file modes.
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).expect("chmod");
            return;
        }
        let refused = OriginStore::open(&p, 1 << 20, &Discard);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert!(matches!(refused, Err(StoreError::Io(_))), "an unreadable store must not open");
        assert_eq!(std::fs::read(&p).expect("read"), before);
    }

    #[test]
    fn compaction_keeps_the_live_set() {
        let p = tmp("compact");
        let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        for i in 0..5000 {
            s.put("counter", &Norm::int(i)).expect("put");
        }
        assert!(std::fs::metadata(&p).expect("stat").len() < 80 * 1024);
        drop(s);
        let s = OriginStore::open(&p, 1 << 20, &Discard).expect("reopen");
        assert_eq!(s.get("counter"), Some(Norm::int(4999)));
        let leftovers: Vec<_> = std::fs::read_dir(p.parent().expect("parent"))
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(leftovers, [std::ffi::OsString::from("s.gzs")], "no temporary file is left");
    }

    #[test]
    fn protocol() {
        let p = tmp("proto");
        let mut s = OriginStore::open(&p, 1 << 20, &Discard).expect("open");
        let r = s.serve(&[Norm::str("put"), Norm::str("a"), Norm::int(3)]).expect("reply");
        assert_eq!(r, Norm::tuple(vec![Norm::str("ok"), Norm::nil()]));
        let r = s.serve(&[Norm::str("get"), Norm::str("a")]).expect("reply");
        assert_eq!(r, Norm::tuple(vec![Norm::str("ok"), Norm::int(3)]));
        let r = s.serve(&[Norm::str("zap"), Norm::str("a")]).expect("reply");
        assert_eq!(r.as_coll(CollKind::Tuple).expect("a tuple")[1].as_str(), Some("verb"));
    }
}
