//! An in-memory file system that models crashes, for tests.
//!
//! It keeps two views of every name and every file: what the program sees,
//! and what would survive a power cut. The rules are the start-up model's
//! (docs/storage/tla/ProfileStartup.tla):
//! - A1: each directory operation (create, link, rename, remove) is atomic.
//! - A2: it changes the program's view at once and joins a queue of pending
//!   operations; [`Fs::sync_dir`] makes durable the pending operations that
//!   touch that directory. A power cut keeps the durable names plus any
//!   chosen subset of the pending operations, applied in order.
//! - A3: data written to a file is durable once the file is synced; a file
//!   never synced holds [`GARBAGE`] after a power cut.
//!
//! [`MemFs::ntfs`] switches to the semantics `StdFs` relies on under
//! Windows (W1 and W2 in [`crate::StdFs`]): directory syncs do nothing; a
//! power cut keeps a prefix of each volume's pending operations; syncing a
//! file commits its volume's pending operations; and giving a file a name
//! syncs it, as `StdFs` does there.
//!
//! Two failures are modelled. [`MemFs::fail_after`] makes every operation
//! after the k-th fail, as if the process had died: nothing more changes,
//! and the kernel keeps what it has, pending operations included. Then
//! [`MemFs::power_cut`] drops what is not durable. A test enumerates every
//! k, and every subset of the pending operations, and checks its
//! invariants on what is left.

use crate::fs::{Fs, Kind, PRIVATE_DIR_MODE, PRIVATE_FILE_MODE};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// What a file holds after a power cut if its data was never synced: bytes
/// no parser in F1R3Gaze accepts (they are not UTF-8).
pub const GARBAGE: &[u8] = b"\xffgarbage after a power cut\xff";

/// Symbolic links [`MemFs`] follows before giving up.
const MAX_LINKS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    File(usize),
    Dir(u32),
    Symlink(PathBuf),
}

#[derive(Clone, Debug)]
struct Inode {
    data: Vec<u8>,
    durable: Option<Vec<u8>>,
    mode: u32,
}

/// What a path names once symbolic links are followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    File(usize),
    Dir(u32),
}

/// One directory operation: the directories whose sync makes it durable,
/// and how it changes the names (all at once).
#[derive(Clone, Debug)]
struct Pending {
    dirs: Vec<PathBuf>,
    changes: Vec<(PathBuf, Option<Node>)>,
}

#[derive(Clone, Debug)]
struct State {
    names: BTreeMap<PathBuf, Node>,
    durable: BTreeMap<PathBuf, Node>,
    inodes: Vec<Inode>,
    pending: Vec<Pending>,
    ops: usize,
    fail_after: Option<usize>,
    hard_links: bool,
    devices: Vec<PathBuf>,
    faulty_copies: bool,
    ntfs: bool,
}

/// The in-memory file system. Cloning it copies its whole state, so a test
/// can branch at a crash point.
#[derive(Debug)]
pub struct MemFs {
    state: Mutex<State>,
}

impl Clone for MemFs {
    fn clone(&self) -> MemFs {
        MemFs {
            state: Mutex::new(self.lock().clone()),
        }
    }
}

impl Default for MemFs {
    fn default() -> MemFs {
        MemFs::new()
    }
}

impl MemFs {
    /// An empty file system: only `/`, durable.
    pub fn new() -> MemFs {
        let mut names = BTreeMap::new();
        names.insert(PathBuf::from("/"), Node::Dir(0o755));
        MemFs {
            state: Mutex::new(State {
                durable: names.clone(),
                names,
                inodes: Vec::new(),
                pending: Vec::new(),
                ops: 0,
                fail_after: None,
                hard_links: true,
                devices: Vec::new(),
                faulty_copies: false,
                ntfs: false,
            }),
        }
    }

    /// Puts everything under `root` on another file system: hard links and
    /// renames between it and the rest fail with `CrossesDevices`.
    pub fn with_device(self, root: impl Into<PathBuf>) -> MemFs {
        self.lock().devices.push(root.into());
        self
    }

    /// A file system without hard links (FAT, exFAT): `hard_link` fails with
    /// `Unsupported`.
    pub fn without_hard_links(self) -> MemFs {
        self.lock().hard_links = false;
        self
    }

    /// Windows semantics (W1 and W2 in [`crate::StdFs`]), as described in
    /// the module documentation.
    pub fn ntfs(self) -> MemFs {
        self.lock().ntfs = true;
        self
    }

    /// A file system whose copies come out wrong (the last byte flipped), as
    /// a failing disk or a bad cable would make them: what the read-back
    /// comparison of [`crate::copy_verified`] is for.
    pub fn with_faulty_copies(self) -> MemFs {
        self.lock().faulty_copies = true;
        self
    }

    /// Turns faulty copies (see [`MemFs::with_faulty_copies`]) on or off,
    /// for a test that fails one run and then repeats it on a good disk.
    pub fn set_faulty_copies(&self, faulty: bool) {
        self.lock().faulty_copies = faulty;
    }

    /// Creates `path` holding `bytes`, and every missing parent, all
    /// durable: a starting state for a test. Replaces a file already there.
    pub fn seed_file(&self, path: impl AsRef<Path>, bytes: &[u8]) {
        let path = path.as_ref();
        let mut state = self.lock();
        state.seed_dirs(path.parent().expect("a seeded file has a parent"));
        let inode = state.inodes.len();
        state.inodes.push(Inode {
            data: bytes.to_vec(),
            durable: Some(bytes.to_vec()),
            mode: PRIVATE_FILE_MODE,
        });
        state.names.insert(path.to_path_buf(), Node::File(inode));
        state.durable.insert(path.to_path_buf(), Node::File(inode));
    }

    /// Creates the directory `path` and every missing parent, all durable.
    pub fn seed_dir(&self, path: impl AsRef<Path>) {
        self.lock().seed_dirs(path.as_ref());
    }

    /// Creates a durable symbolic link at `path` pointing to `target`.
    pub fn seed_symlink(&self, path: impl AsRef<Path>, target: impl Into<PathBuf>) {
        let path = path.as_ref();
        let mut state = self.lock();
        state.seed_dirs(path.parent().expect("a seeded link has a parent"));
        let node = Node::Symlink(target.into());
        state.names.insert(path.to_path_buf(), node.clone());
        state.durable.insert(path.to_path_buf(), node);
    }

    /// Every regular file as the program sees it, with its bytes.
    pub fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let state = self.lock();
        state
            .names
            .iter()
            .filter_map(|(path, node)| match node {
                Node::File(inode) => Some((path.clone(), state.inodes[*inode].data.clone())),
                _ => None,
            })
            .collect()
    }

    /// Every directory as the program sees it.
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.lock()
            .names
            .iter()
            .filter_map(|(path, node)| match node {
                Node::Dir(_) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    /// How many operations have run since the last [`MemFs::reset_ops`].
    pub fn ops(&self) -> usize {
        self.lock().ops
    }

    /// Starts counting operations from zero.
    pub fn reset_ops(&self) {
        self.lock().ops = 0;
    }

    /// Makes every operation after the `k`-th fail: the process died there.
    pub fn fail_after(&self, k: usize) {
        let mut state = self.lock();
        state.ops = 0;
        state.fail_after = Some(k);
    }

    /// A new process: operations work again. Pending operations stay
    /// pending, as the kernel still has them.
    pub fn revive(&self) {
        let mut state = self.lock();
        state.fail_after = None;
        state.ops = 0;
    }

    /// How many directory operations are not yet durable.
    pub fn pending_len(&self) -> usize {
        self.lock().pending.len()
    }

    /// The directory operations not yet durable, oldest first: for each, the
    /// names it changes, each with whether it gives the name a node (`true`)
    /// or removes it (`false`). For translating a power cut into the
    /// start-up model's terms (ledger S8).
    pub fn pending_changes(&self) -> Vec<Vec<(PathBuf, bool)>> {
        self.lock()
            .pending
            .iter()
            .map(|op| op.changes.iter().map(|(path, node)| (path.clone(), node.is_some())).collect())
            .collect()
    }

    /// The directory operations not yet durable, oldest first: for each,
    /// the directories whose sync would make it durable and the names it
    /// changes. For a test's failure message.
    pub fn pending(&self) -> Vec<String> {
        self.lock()
            .pending
            .iter()
            .map(|op| {
                let dirs: Vec<String> = op.dirs.iter().map(|d| d.display().to_string()).collect();
                let names: Vec<String> = op
                    .changes
                    .iter()
                    .map(|(path, node)| match node {
                        Some(_) => format!("+{}", path.display()),
                        None => format!("-{}", path.display()),
                    })
                    .collect();
                format!("{} (sync {})", names.join(" "), dirs.join(", "))
            })
            .collect()
    }

    /// Every choice of pending operations a power cut may keep, as one flag
    /// per pending operation: every subset, or under [`MemFs::ntfs`] a
    /// prefix of each volume's operations.
    pub fn power_cuts(&self) -> Vec<Vec<bool>> {
        let state = self.lock();
        let n = state.pending.len();
        match state.ntfs {
            // Every subset: 2^n of them. (Under NTFS a cut keeps a prefix of
            // each volume's operations, so there are only about n of them,
            // and any n is fine.)
            false => {
                assert!(n <= 16, "{n} pending operations: too many power cuts to enumerate");
                (0u32..(1 << n))
                    .map(|subset| (0..n).map(|i| subset & (1 << i) != 0).collect())
                    .collect()
            }
            true => {
                let volumes: Vec<usize> = state.pending.iter().map(|op| state.volume(op)).collect();
                let mut distinct = volumes.clone();
                distinct.sort_unstable();
                distinct.dedup();
                let mut cuts: Vec<Vec<bool>> = vec![vec![false; n]];
                for volume in distinct {
                    let ops: Vec<usize> = (0..n).filter(|&i| volumes[i] == volume).collect();
                    let mut extended = Vec::with_capacity(cuts.len() * (ops.len() + 1));
                    for cut in &cuts {
                        for kept in 0..=ops.len() {
                            let mut next = cut.clone();
                            for &i in &ops[..kept] {
                                next[i] = true;
                            }
                            extended.push(next);
                        }
                    }
                    cuts = extended;
                }
                cuts
            }
        }
    }

    /// A power cut. The names become the durable ones plus the pending
    /// operations `keep` chooses (by index, oldest first), applied in
    /// order; every file holds its synced data, or [`GARBAGE`]. A name whose
    /// folder did not survive is gone too: an entry lives in its folder, so
    /// a folder whose own creation was lost takes its entries with it.
    /// Operations work again afterwards (the machine restarted).
    pub fn power_cut(&self, keep: impl Fn(usize) -> bool) {
        let mut state = self.lock();
        let mut names = state.durable.clone();
        for (index, op) in state.pending.iter().enumerate() {
            if keep(index) {
                apply(&mut names, &op.changes);
            }
        }
        let names = reachable(names);
        state.durable = names.clone();
        state.names = names;
        state.pending.clear();
        for inode in &mut state.inodes {
            inode.data = inode.durable.clone().unwrap_or_else(|| GARBAGE.to_vec());
        }
        state.fail_after = None;
        state.ops = 0;
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("the MemFs lock is never poisoned")
    }

    /// The state for one operation, or the error of a dead process.
    fn op(&self) -> io::Result<MutexGuard<'_, State>> {
        let mut state = self.lock();
        state.ops += 1;
        match state.fail_after {
            Some(k) if state.ops > k => Err(io::Error::other(
                "injected crash: the process died before this operation",
            )),
            _ => Ok(state),
        }
    }
}

impl State {
    fn seed_dirs(&mut self, dir: &Path) {
        for ancestor in dir.ancestors().collect::<Vec<_>>().into_iter().rev() {
            if !self.names.contains_key(ancestor) {
                self.names.insert(ancestor.to_path_buf(), Node::Dir(PRIVATE_DIR_MODE));
                self.durable.insert(ancestor.to_path_buf(), Node::Dir(PRIVATE_DIR_MODE));
            }
        }
    }

    /// Applies a directory operation now and queues it for durability.
    fn dir_op(&mut self, dirs: Vec<PathBuf>, changes: Vec<(PathBuf, Option<Node>)>) {
        apply(&mut self.names, &changes);
        self.pending.push(Pending { dirs, changes });
    }

    /// What a path names after following symbolic links, and the path of
    /// that file or directory.
    fn follow(&self, path: &Path) -> io::Result<(PathBuf, Target)> {
        let mut current = path.to_path_buf();
        for _ in 0..MAX_LINKS {
            match self.names.get(&current) {
                Some(Node::Symlink(target)) => {
                    current = match target.is_absolute() {
                        true => target.clone(),
                        false => current
                            .parent()
                            .map(|dir| dir.join(target))
                            .unwrap_or_else(|| target.clone()),
                    };
                }
                Some(Node::File(inode)) => return Ok((current, Target::File(*inode))),
                Some(Node::Dir(mode)) => return Ok((current, Target::Dir(*mode))),
                None => return Err(not_found(&current)),
            }
        }
        Err(io::Error::other(format!(
            "{}: too many symbolic links",
            path.display()
        )))
    }

    fn file(&self, path: &Path) -> io::Result<usize> {
        match self.follow(path)? {
            (_, Target::File(inode)) => Ok(inode),
            (_, Target::Dir(_)) => Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!("{} is a directory", path.display()),
            )),
        }
    }

    /// `path`'s parent, which must be an existing directory.
    fn parent_dir(&self, path: &Path) -> io::Result<PathBuf> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
        match self.follow(parent) {
            Ok((dir, Target::Dir(_))) => Ok(dir),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", parent.display()),
            )),
            Err(e) => Err(e),
        }
    }

    fn absent(&self, path: &Path) -> io::Result<()> {
        match self.names.contains_key(path) {
            true => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", path.display()),
            )),
            false => Ok(()),
        }
    }

    fn device(&self, path: &Path) -> usize {
        self.devices
            .iter()
            .enumerate()
            .filter(|(_, root)| path.starts_with(root))
            .max_by_key(|(_, root)| root.components().count())
            .map(|(index, _)| index + 1)
            .unwrap_or(0)
    }

    fn same_device(&self, a: &Path, b: &Path) -> io::Result<()> {
        match self.device(a) == self.device(b) {
            true => Ok(()),
            false => Err(io::Error::new(
                io::ErrorKind::CrossesDevices,
                format!("{} and {} are on different file systems", a.display(), b.display()),
            )),
        }
    }

    /// The volume (device) a directory operation is on: that of its first
    /// directory, since operations across volumes are refused.
    fn volume(&self, op: &Pending) -> usize {
        op.dirs.first().map(|dir| self.device(dir)).unwrap_or(0)
    }

    /// Under NTFS semantics, commits every pending operation on `path`'s
    /// volume (W2), in order.
    fn commit_volume(&mut self, path: &Path) {
        let volume = self.device(path);
        let pending = std::mem::take(&mut self.pending);
        let (now, later): (Vec<Pending>, Vec<Pending>) =
            pending.into_iter().partition(|op| self.volume(op) == volume);
        for op in &now {
            apply(&mut self.durable, &op.changes);
        }
        self.pending = later;
    }

    /// What `StdFs` does on Windows after naming a file: sync the file,
    /// which commits its volume's log (W2).
    fn commit_name(&mut self, path: &Path) {
        if self.ntfs
            && let Some(Node::File(inode)) = self.names.get(path).cloned()
        {
            self.inodes[inode].durable = Some(self.inodes[inode].data.clone());
            self.commit_volume(path);
        }
    }

    fn create(&mut self, path: &Path, bytes: Vec<u8>, mode: u32) -> io::Result<()> {
        let dir = self.parent_dir(path)?;
        self.absent(path)?;
        let inode = self.inodes.len();
        self.inodes.push(Inode {
            data: bytes,
            durable: None,
            mode,
        });
        self.dir_op(vec![dir], vec![(path.to_path_buf(), Some(Node::File(inode)))]);
        Ok(())
    }
}

/// Applies one operation's name changes to a view of the names.
fn apply(names: &mut BTreeMap<PathBuf, Node>, changes: &[(PathBuf, Option<Node>)]) {
    for (path, node) in changes {
        match node {
            Some(node) => {
                names.insert(path.clone(), node.clone());
            }
            None => {
                names.remove(path);
            }
        }
    }
}

/// The names whose every ancestor is a folder that is there. Paths order
/// by their components, so each folder comes before what is in it, and one
/// pass suffices.
fn reachable(names: BTreeMap<PathBuf, Node>) -> BTreeMap<PathBuf, Node> {
    let mut kept: BTreeMap<PathBuf, Node> = BTreeMap::new();
    for (path, node) in names {
        let reached = match path.parent() {
            None => true,
            Some(parent) => matches!(kept.get(parent), Some(Node::Dir(_))),
        };
        if reached {
            kept.insert(path, node);
        }
    }
    kept
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{}: no such file or directory", path.display()),
    )
}

impl Fs for MemFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let state = self.op()?;
        let inode = state.file(path)?;
        Ok(state.inodes[inode].data.clone())
    }

    fn kind(&self, path: &Path) -> io::Result<Kind> {
        let state = self.op()?;
        match state.names.get(path) {
            Some(Node::File(_)) => Ok(Kind::File),
            Some(Node::Dir(_)) => Ok(Kind::Dir),
            Some(Node::Symlink(_)) => Ok(Kind::Symlink),
            None => Err(not_found(path)),
        }
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        let state = self.op()?;
        match state.names.get(path) {
            Some(Node::Symlink(target)) => Ok(target.clone()),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a symbolic link", path.display()),
            )),
            None => Err(not_found(path)),
        }
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<OsString>> {
        let state = self.op()?;
        let (dir, target) = state.follow(dir)?;
        match target {
            Target::Dir(_) => Ok(state
                .names
                .keys()
                .filter(|path| path.parent() == Some(dir.as_path()))
                .filter_map(|path| path.file_name().map(|name| name.to_os_string()))
                .collect()),
            _ => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", dir.display()),
            )),
        }
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        let ancestors: Vec<PathBuf> = dir.ancestors().map(Path::to_path_buf).collect();
        for ancestor in ancestors.into_iter().rev() {
            match state.names.get(&ancestor) {
                Some(Node::Dir(_)) => {}
                Some(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        format!("{} is not a directory", ancestor.display()),
                    ));
                }
                None => {
                    let parent = state.parent_dir(&ancestor)?;
                    state.dir_op(
                        vec![parent],
                        vec![(ancestor.clone(), Some(Node::Dir(PRIVATE_DIR_MODE)))],
                    );
                }
            }
        }
        Ok(())
    }

    fn create_dir(&self, dir: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        let parent = state.parent_dir(dir)?;
        state.absent(dir)?;
        state.dir_op(vec![parent], vec![(dir.to_path_buf(), Some(Node::Dir(PRIVATE_DIR_MODE)))]);
        Ok(())
    }

    fn create_new(&self, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        self.op()?.create(path, bytes.to_vec(), mode)
    }

    fn copy_new(&self, from: &Path, to: &Path, mode: u32) -> io::Result<()> {
        let mut state = self.op()?;
        let inode = state.file(from)?;
        let mut data = state.inodes[inode].data.clone();
        if state.faulty_copies
            && let Some(last) = data.last_mut()
        {
            *last ^= 1;
        }
        state.create(to, data, mode)
    }

    fn same_contents(&self, a: &Path, b: &Path) -> io::Result<bool> {
        let state = self.op()?;
        let (first, second) = (state.file(a)?, state.file(b)?);
        Ok(state.inodes[first].data == state.inodes[second].data)
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        let inode = state.file(path)?;
        let data = state.inodes[inode].data.clone();
        state.inodes[inode].durable = Some(data);
        if state.ntfs {
            state.commit_volume(path);
        }
        Ok(())
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        let (dir, target) = state.follow(dir)?;
        match target {
            Target::Dir(_) => {}
            Target::File(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    format!("{} is not a directory", dir.display()),
                ));
            }
        }
        if state.ntfs {
            return Ok(());
        }
        let pending = std::mem::take(&mut state.pending);
        let (durable_now, still_pending): (Vec<Pending>, Vec<Pending>) =
            pending.into_iter().partition(|op| op.dirs.contains(&dir));
        for op in &durable_now {
            apply(&mut state.durable, &op.changes);
        }
        state.pending = still_pending;
        Ok(())
    }

    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        if !state.hard_links {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this file system has no hard links",
            ));
        }
        let inode = match state.names.get(from) {
            Some(Node::File(inode)) => *inode,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a file", from.display()),
                ));
            }
            None => return Err(not_found(from)),
        };
        let dir = state.parent_dir(to)?;
        state.absent(to)?;
        state.same_device(from, to)?;
        state.dir_op(vec![dir], vec![(to.to_path_buf(), Some(Node::File(inode)))]);
        state.commit_name(to);
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        let node = state.names.get(from).cloned().ok_or_else(|| not_found(from))?;
        let to_dir = state.parent_dir(to)?;
        let from_dir = state.parent_dir(from)?;
        state.same_device(from, to)?;
        if let Some(Node::Dir(_)) = state.names.get(to) {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!("{} is a directory", to.display()),
            ));
        }
        // The name, and for a directory every name below it, moves at once.
        let mut changes: Vec<(PathBuf, Option<Node>)> = Vec::new();
        let subtree: Vec<(PathBuf, Node)> = state
            .names
            .iter()
            .filter(|(path, _)| path.starts_with(from) && path.as_path() != from)
            .map(|(path, node)| (path.clone(), node.clone()))
            .collect();
        changes.reserve(2 + 2 * subtree.len());
        changes.push((from.to_path_buf(), None));
        for (path, _) in &subtree {
            changes.push((path.clone(), None));
        }
        changes.push((to.to_path_buf(), Some(node)));
        for (path, node) in subtree {
            let below = path.strip_prefix(from).expect("filtered by prefix");
            changes.push((to.join(below), Some(node)));
        }
        let mut dirs = vec![from_dir];
        if !dirs.contains(&to_dir) {
            dirs.push(to_dir);
        }
        state.dir_op(dirs, changes);
        state.commit_name(to);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        match state.names.get(path) {
            Some(Node::File(_)) | Some(Node::Symlink(_)) => {}
            Some(Node::Dir(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::IsADirectory,
                    format!("{} is a directory", path.display()),
                ));
            }
            None => return Err(not_found(path)),
        }
        let dir = state.parent_dir(path)?;
        state.dir_op(vec![dir], vec![(path.to_path_buf(), None)]);
        Ok(())
    }

    fn remove_dir(&self, dir: &Path) -> io::Result<()> {
        let mut state = self.op()?;
        match state.names.get(dir) {
            Some(Node::Dir(_)) => {}
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    format!("{} is not a directory", dir.display()),
                ));
            }
            None => return Err(not_found(dir)),
        }
        if state.names.keys().any(|path| path.parent() == Some(dir)) {
            return Err(io::Error::new(
                io::ErrorKind::DirectoryNotEmpty,
                format!("{} is not empty", dir.display()),
            ));
        }
        let parent = state.parent_dir(dir)?;
        state.dir_op(vec![parent], vec![(dir.to_path_buf(), None)]);
        Ok(())
    }

    fn mode(&self, path: &Path) -> io::Result<Option<u32>> {
        let state = self.op()?;
        match state.follow(path)? {
            (_, Target::File(inode)) => Ok(Some(state.inodes[inode].mode)),
            (_, Target::Dir(mode)) => Ok(Some(mode)),
        }
    }

    fn len(&self, path: &Path) -> io::Result<u64> {
        let state = self.op()?;
        let inode = state.file(path)?;
        Ok(state.inodes[inode].data.len() as u64)
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        let mut state = self.op()?;
        match state.follow(path)? {
            (_, Target::File(inode)) => {
                state.inodes[inode].mode = mode;
                Ok(())
            }
            (dir, Target::Dir(_)) => {
                state.names.insert(dir.clone(), Node::Dir(mode));
                if state.durable.contains_key(&dir) {
                    state.durable.insert(dir, Node::Dir(mode));
                }
                Ok(())
            }
        }
    }
}
