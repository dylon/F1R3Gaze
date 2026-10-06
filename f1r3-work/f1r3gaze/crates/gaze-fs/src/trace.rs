//! [`TraceFs`]: a file system that forwards every operation to another one
//! and records it. It serves tests ("a second start only reads") and the
//! checking of real runs against the TLA+ model (docs/storage/tla/, ledger
//! S8), for which it can also append each record to a file as a line of
//! JSON.

use crate::fs::{Fs, Kind};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// The 64-bit FNV-1a hash of `bytes`: a short, stable fingerprint of the
/// content a write put in a file.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One operation a [`TraceFs`] forwarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Traced {
    /// Its place in the sequence, from 0.
    pub n: u64,
    /// The `Fs` method's name, or `note`.
    pub op: &'static str,
    pub path: PathBuf,
    /// The second path of `copy_new`, `hard_link`, `rename` and
    /// `same_contents`.
    pub to: Option<PathBuf>,
    /// For `create_new`: the length and fingerprint of the bytes written.
    pub len: Option<u64>,
    pub fnv: Option<u64>,
    /// For `create_new`, `copy_new` and `set_mode`: the mode asked for.
    pub mode: Option<u32>,
    /// For `same_contents`, when it succeeded: the answer.
    pub equal: Option<bool>,
    /// For `note`: what was noted.
    pub what: Option<String>,
    /// The error's kind, if the operation failed.
    pub error: Option<io::ErrorKind>,
}

/// The operations that can change a file system or make a change durable.
const WRITING: [&str; 11] = [
    "create_dir_all",
    "create_dir",
    "create_new",
    "copy_new",
    "sync_file",
    "sync_dir",
    "hard_link",
    "rename",
    "remove_file",
    "remove_dir",
    "set_mode",
];

/// `text` as a JSON string, quotes included.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Traced {
    fn of(op: &'static str, path: &Path) -> Traced {
        Traced {
            n: 0,
            op,
            path: path.to_path_buf(),
            to: None,
            len: None,
            fnv: None,
            mode: None,
            equal: None,
            what: None,
            error: None,
        }
    }

    fn to(mut self, to: &Path) -> Traced {
        self.to = Some(to.to_path_buf());
        self
    }

    fn failed<T>(mut self, result: &io::Result<T>) -> Traced {
        self.error = result.as_ref().err().map(io::Error::kind);
        self
    }

    /// Whether the operation could change the file system, or make a change
    /// durable, had it succeeded.
    pub fn writes(&self) -> bool {
        WRITING.contains(&self.op)
    }

    /// The record as one line of JSON (without the line break).
    pub fn json(&self) -> String {
        let mut line = format!("{{\"n\":{},\"op\":{}", self.n, json_string(self.op));
        if self.op != "note" {
            line.push_str(&format!(",\"path\":{}", json_string(&self.path.to_string_lossy())));
        }
        if let Some(to) = &self.to {
            line.push_str(&format!(",\"to\":{}", json_string(&to.to_string_lossy())));
        }
        if let Some(len) = self.len {
            line.push_str(&format!(",\"len\":{len}"));
        }
        if let Some(fnv) = self.fnv {
            line.push_str(&format!(",\"fnv\":\"{fnv:016x}\""));
        }
        if let Some(mode) = self.mode {
            line.push_str(&format!(",\"mode\":{mode}"));
        }
        if let Some(equal) = self.equal {
            line.push_str(&format!(",\"equal\":{equal}"));
        }
        if let Some(what) = &self.what {
            line.push_str(&format!(",\"what\":{}", json_string(what)));
        }
        match self.error {
            None if self.op == "note" => {}
            None => line.push_str(",\"ok\":true"),
            Some(kind) => line.push_str(&format!(",\"ok\":false,\"err\":{}", json_string(&format!("{kind:?}")))),
        }
        line.push('}');
        line
    }
}

/// What a test runs before an operation is forwarded: it sees the
/// operation, and may change the file system under it, as another program
/// would.
type Hook = Box<dyn Fn(&Traced) + Send + Sync>;

/// A file system that forwards to another and records every operation.
pub struct TraceFs {
    inner: Arc<dyn Fs + Send + Sync>,
    log: Mutex<Vec<Traced>>,
    file: Option<Mutex<File>>,
    hook: Option<Hook>,
}

impl TraceFs {
    pub fn new(inner: Arc<dyn Fs + Send + Sync>) -> TraceFs {
        TraceFs {
            inner,
            log: Mutex::new(Vec::with_capacity(256)),
            file: None,
            hook: None,
        }
    }

    /// A trace that runs `hook` before forwarding each operation: a test's
    /// way to make something happen between two of them, such as a file
    /// appearing at a name another program creates.
    pub fn with_hook(inner: Arc<dyn Fs + Send + Sync>, hook: impl Fn(&Traced) + Send + Sync + 'static) -> TraceFs {
        TraceFs {
            hook: Some(Box::new(hook)),
            ..TraceFs::new(inner)
        }
    }

    /// Runs the hook, if there is one, before `traced` is forwarded.
    fn before(&self, traced: &Traced) {
        if let Some(hook) = &self.hook {
            hook(traced);
        }
    }

    /// A trace that also appends each record to the file at `path`, one
    /// line of JSON each. The file is created owner-only if it is missing.
    pub fn appending_to(inner: Arc<dyn Fs + Send + Sync>, path: &Path) -> io::Result<TraceFs> {
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(crate::PRIVATE_FILE_MODE);
        }
        let file = options.open(path)?;
        Ok(TraceFs {
            file: Some(Mutex::new(file)),
            ..TraceFs::new(inner)
        })
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Traced>> {
        self.log.lock().expect("the trace's lock is never poisoned")
    }

    /// Every operation so far, in order.
    pub fn log(&self) -> Vec<Traced> {
        self.lock().clone()
    }

    /// Forgets what was recorded (the file keeps its lines).
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn record(&self, mut traced: Traced) {
        let mut log = self.lock();
        traced.n = log.len() as u64;
        if let Some(file) = &self.file {
            let mut file = file.lock().expect("the trace file's lock is never poisoned");
            // A trace that cannot be written is a broken test set-up, not a
            // storage failure: say so loudly.
            writeln!(file, "{}", traced.json()).expect("the trace file can be written");
        }
        log.push(traced);
    }
}

impl Fs for TraceFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.before(&Traced::of("read", path));
        let result = self.inner.read(path);
        self.record(Traced::of("read", path).failed(&result));
        result
    }

    fn kind(&self, path: &Path) -> io::Result<Kind> {
        self.before(&Traced::of("kind", path));
        let result = self.inner.kind(path);
        self.record(Traced::of("kind", path).failed(&result));
        result
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.before(&Traced::of("read_link", path));
        let result = self.inner.read_link(path);
        self.record(Traced::of("read_link", path).failed(&result));
        result
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<OsString>> {
        self.before(&Traced::of("list", dir));
        let result = self.inner.list(dir);
        self.record(Traced::of("list", dir).failed(&result));
        result
    }

    // It forwards what its caller asked for, and records it.
    #[allow(clippy::disallowed_methods)]
    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        self.before(&Traced::of("create_dir_all", dir));
        let result = self.inner.create_dir_all(dir);
        self.record(Traced::of("create_dir_all", dir).failed(&result));
        result
    }

    fn create_dir(&self, dir: &Path) -> io::Result<()> {
        self.before(&Traced::of("create_dir", dir));
        let result = self.inner.create_dir(dir);
        self.record(Traced::of("create_dir", dir).failed(&result));
        result
    }

    fn create_new(&self, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        self.before(&Traced::of("create_new", path));
        let result = self.inner.create_new(path, bytes, mode);
        let mut traced = Traced::of("create_new", path).failed(&result);
        traced.len = Some(bytes.len() as u64);
        traced.fnv = Some(fnv1a64(bytes));
        traced.mode = Some(mode);
        self.record(traced);
        result
    }

    fn copy_new(&self, from: &Path, to: &Path, mode: u32) -> io::Result<()> {
        self.before(&Traced::of("copy_new", from).to(to));
        let result = self.inner.copy_new(from, to, mode);
        let mut traced = Traced::of("copy_new", from).to(to).failed(&result);
        traced.mode = Some(mode);
        self.record(traced);
        result
    }

    fn same_contents(&self, a: &Path, b: &Path) -> io::Result<bool> {
        self.before(&Traced::of("same_contents", a).to(b));
        let result = self.inner.same_contents(a, b);
        let mut traced = Traced::of("same_contents", a).to(b).failed(&result);
        traced.equal = result.as_ref().ok().copied();
        self.record(traced);
        result
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        self.before(&Traced::of("sync_file", path));
        let result = self.inner.sync_file(path);
        self.record(Traced::of("sync_file", path).failed(&result));
        result
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        self.before(&Traced::of("sync_dir", dir));
        let result = self.inner.sync_dir(dir);
        self.record(Traced::of("sync_dir", dir).failed(&result));
        result
    }

    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.before(&Traced::of("hard_link", from).to(to));
        let result = self.inner.hard_link(from, to);
        self.record(Traced::of("hard_link", from).to(to).failed(&result));
        result
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.before(&Traced::of("rename", from).to(to));
        let result = self.inner.rename(from, to);
        self.record(Traced::of("rename", from).to(to).failed(&result));
        result
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.before(&Traced::of("remove_file", path));
        let result = self.inner.remove_file(path);
        self.record(Traced::of("remove_file", path).failed(&result));
        result
    }

    fn remove_dir(&self, dir: &Path) -> io::Result<()> {
        self.before(&Traced::of("remove_dir", dir));
        let result = self.inner.remove_dir(dir);
        self.record(Traced::of("remove_dir", dir).failed(&result));
        result
    }

    fn note(&self, what: &str) {
        self.inner.note(what);
        let mut traced = Traced::of("note", Path::new(""));
        traced.what = Some(what.to_string());
        self.record(traced);
    }

    fn mode(&self, path: &Path) -> io::Result<Option<u32>> {
        self.before(&Traced::of("mode", path));
        let result = self.inner.mode(path);
        self.record(Traced::of("mode", path).failed(&result));
        result
    }

    fn len(&self, path: &Path) -> io::Result<u64> {
        self.before(&Traced::of("len", path));
        let result = self.inner.len(path);
        self.record(Traced::of("len", path).failed(&result));
        result
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        self.before(&Traced::of("set_mode", path));
        let result = self.inner.set_mode(path, mode);
        let mut traced = Traced::of("set_mode", path).failed(&result);
        traced.mode = Some(mode);
        self.record(traced);
        result
    }
}
