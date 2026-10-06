//! Real start-ups as behaviours of the start-up model (ledger S8).
//!
//! A start's file operations, recorded by gaze-fs's `TraceFs`, are put into
//! the terms of `docs/storage/tla/ProfileStartup.tla` and written out as a
//! TLA+ module that `ProfileStartupTrace.tla` checks: TLC accepts the trace
//! exactly when the model can do the same operations in the same order.
//!
//! **What is modelled.** The plan's writes, item moves and original moves
//! (the model's `Order`), their temporary files, the marker and its
//! temporary file, the busy flag, the barrier, and the syncs of the folders
//! they are in. Everything else a start does (folders made, the plan,
//! `MIGRATED.txt`, the cache, start-up's repair) is left out: it touches no
//! modelled name. A directory sync of a modelled folder that the model does
//! not need at that point is an *eager* sync, which the model allows
//! anywhere: it only makes pending operations durable.

use super::{Action, Plan, Role};
use crate::profile::layout::Layout;
use gaze_fs::{Traced, fnv1a64};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One thing that happened while a case ran.
#[derive(Clone, Debug)]
pub enum Event {
    /// An operation a `TraceFs` forwarded.
    Op(Traced),
    /// The process died: the next start is another process.
    ProcessCrash,
    /// The power was cut. `pending` is the file system's queue of directory
    /// operations not yet durable (`MemFs::pending_changes`), and `keep`
    /// which of them the cut kept.
    PowerCut { pending: Vec<Vec<(PathBuf, bool)>>, keep: Vec<bool> },
}

/// A case to check: what was on the disk, the plan its first start made,
/// and what happened.
pub struct Case<'a> {
    /// A TLA+ identifier: letters, digits and underscores.
    pub name: &'a str,
    pub layout: &'a Layout,
    pub plan: &'a Plan,
    /// The folders on another volume than the data root.
    pub far: &'a [PathBuf],
    /// Windows semantics (W1, W2).
    pub ntfs: bool,
    /// Copies to another volume may come out wrong.
    pub bad_copies: bool,
    /// The planned destinations that held something before the first start.
    pub taken: &'a [PathBuf],
    pub events: &'a [Event],
}

/// The operations a `TraceFs` records: the `Fs` methods, and `note`. A
/// record read back names one of them.
const OPS: [&str; 19] = [
    "read",
    "kind",
    "read_link",
    "list",
    "create_dir_all",
    "create_dir",
    "create_new",
    "copy_new",
    "same_contents",
    "sync_file",
    "sync_dir",
    "hard_link",
    "rename",
    "remove_file",
    "remove_dir",
    "note",
    "mode",
    "len",
    "set_mode",
];

/// The error kinds a record names, by their `Debug` names. Any other name
/// reads back as `Other`: the conversion asks only whether an operation
/// failed.
fn error_kind(name: &str) -> std::io::ErrorKind {
    use std::io::ErrorKind as K;
    match name {
        "NotFound" => K::NotFound,
        "PermissionDenied" => K::PermissionDenied,
        "AlreadyExists" => K::AlreadyExists,
        "WouldBlock" => K::WouldBlock,
        "NotADirectory" => K::NotADirectory,
        "IsADirectory" => K::IsADirectory,
        "DirectoryNotEmpty" => K::DirectoryNotEmpty,
        "ReadOnlyFilesystem" => K::ReadOnlyFilesystem,
        "StorageFull" => K::StorageFull,
        "CrossesDevices" => K::CrossesDevices,
        "InvalidInput" => K::InvalidInput,
        "InvalidData" => K::InvalidData,
        "TimedOut" => K::TimedOut,
        "WriteZero" => K::WriteZero,
        "Interrupted" => K::Interrupted,
        "Unsupported" => K::Unsupported,
        "UnexpectedEof" => K::UnexpectedEof,
        "OutOfMemory" => K::OutOfMemory,
        _ => K::Other,
    }
}

/// The events of a real run: the lines `TraceFs::appending_to` wrote, one
/// record each (`Traced::json`), and the line `{"op":"crash","kind":"process"}`
/// that the test which killed the process appended after its records.
/// Blank lines are skipped.
pub fn events_of_ndjson(text: &str) -> Result<Vec<Event>, String> {
    use serde_json::Value;
    let mut events = Vec::with_capacity(text.lines().count());
    for (i, line) in text.lines().enumerate() {
        let at = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).map_err(|e| format!("line {at}: {e}"))?;
        let text_of = |name: &str| -> Result<Option<&str>, String> {
            match record.get(name) {
                None => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.as_str())),
                Some(other) => Err(format!("line {at}: {name} is {other}, not a string")),
            }
        };
        let number_of = |name: &str| -> Result<Option<u64>, String> {
            match record.get(name) {
                None => Ok(None),
                Some(value) => value.as_u64().map(Some).ok_or_else(|| format!("line {at}: {name} is {value}, not a count")),
            }
        };
        let op = text_of("op")?.ok_or_else(|| format!("line {at}: no op"))?;
        if op == "crash" {
            match text_of("kind")? {
                Some("process") => events.push(Event::ProcessCrash),
                other => return Err(format!("line {at}: a crash of kind {other:?}")),
            }
            continue;
        }
        let op = OPS
            .iter()
            .copied()
            .find(|known| *known == op)
            .ok_or_else(|| format!("line {at}: {op:?} is no operation of a file system"))?;
        let fnv = match text_of("fnv")? {
            Some(hex) => Some(u64::from_str_radix(hex, 16).map_err(|e| format!("line {at}: fnv {hex:?}: {e}"))?),
            None => None,
        };
        let mode = match number_of("mode")? {
            Some(mode) => Some(u32::try_from(mode).map_err(|_| format!("line {at}: mode {mode} is too large"))?),
            None => None,
        };
        let error = match (record.get("ok"), text_of("err")?) {
            (None, _) if op == "note" => None,
            (Some(Value::Bool(true)), None) => None,
            (Some(Value::Bool(false)), Some(kind)) => Some(error_kind(kind)),
            (ok, err) => return Err(format!("line {at}: ok {ok:?} with err {err:?}")),
        };
        events.push(Event::Op(Traced {
            n: number_of("n")?.ok_or_else(|| format!("line {at}: no n"))?,
            op,
            path: PathBuf::from(text_of("path")?.unwrap_or("")),
            to: text_of("to")?.map(PathBuf::from),
            len: number_of("len")?,
            fnv,
            mode,
            equal: match record.get("equal") {
                None => None,
                Some(Value::Bool(equal)) => Some(*equal),
                Some(other) => return Err(format!("line {at}: equal is {other}, not a boolean")),
            },
            what: text_of("what")?.map(str::to_string),
            error,
        }));
    }
    Ok(events)
}

/// The plan a run's migration replayed: the last note `plan <json>` that
/// `execute` made (the run may have been killed and resumed, replaying the
/// same plan each time).
pub fn plan_of(events: &[Event]) -> Result<Plan, String> {
    let json = events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::Op(t) if t.op == "note" => t.what.as_deref().and_then(|what| what.strip_prefix("plan ")),
            _ => None,
        })
        .ok_or("the run replayed no plan")?;
    serde_json::from_str(json).map_err(|e| format!("the plan the run noted: {e}"))
}

/// A TLA+ string literal.
fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `path` as UTF-8, or an error naming it.
fn text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))
}

/// What the model calls each modelled name.
struct Names {
    /// The plan's actions, as the model's `Order`: (kind, id).
    order: Vec<(&'static str, String)>,
    src: BTreeMap<String, String>,
    dst: BTreeMap<String, String>,
    bak: BTreeMap<String, String>,
    /// Exact names: path → TLA+ expression.
    exact: BTreeMap<PathBuf, String>,
    /// Temporary names: (folder, `.<name>.tmp-`, TLA+ expression, id).
    temps: Vec<(PathBuf, String, String, String)>,
    /// The fingerprint of each write's text, by id.
    texts: BTreeMap<String, u64>,
    /// Every modelled folder.
    folders: BTreeSet<String>,
}

impl Names {
    fn of(layout: &Layout, plan: &Plan) -> Result<Names, String> {
        let mut names = Names {
            order: Vec::with_capacity(plan.actions.len()),
            src: BTreeMap::new(),
            dst: BTreeMap::new(),
            bak: BTreeMap::new(),
            exact: BTreeMap::new(),
            temps: Vec::new(),
            texts: BTreeMap::new(),
            folders: BTreeSet::new(),
        };
        let folder = |path: &str| -> Result<String, String> {
            Path::new(path)
                .parent()
                .map(text)
                .unwrap_or_else(|| Err(format!("{path} has no folder")))
        };
        let temp_of = |path: &str| -> String {
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            format!(".{name}.tmp-")
        };
        let (mut writes, mut items, mut kept) = (0usize, 0usize, 0usize);
        for action in &plan.actions {
            match action {
                Action::Write { dst, text, .. } => {
                    writes += 1;
                    let id = format!("o{writes}");
                    let dir = folder(dst)?;
                    names.exact.insert(PathBuf::from(dst), format!("Dst({})", string(&id)));
                    names.temps.push((PathBuf::from(&dir), temp_of(dst), format!("Tmp({})", string(&id)), id.clone()));
                    names.texts.insert(id.clone(), fnv1a64(text.as_bytes()));
                    names.folders.insert(dir.clone());
                    names.dst.insert(id.clone(), dir);
                    names.order.push(("write", id));
                }
                Action::Move { src, dst, role, .. } => {
                    let (kind, id) = match role {
                        Role::Item => {
                            items += 1;
                            ("move", format!("i{items}"))
                        }
                        Role::Original => {
                            kept += 1;
                            ("preserve", format!("k{kept}"))
                        }
                    };
                    let (from, to) = (folder(src)?, folder(dst)?);
                    names.exact.insert(PathBuf::from(src), format!("Src({})", string(&id)));
                    let (at, temp) = match role {
                        Role::Item => ("Dst", "Tmp"),
                        Role::Original => ("PBak", "BakTmp"),
                    };
                    names.exact.insert(PathBuf::from(dst), format!("{at}({})", string(&id)));
                    names.temps.push((PathBuf::from(&to), temp_of(dst), format!("{temp}({})", string(&id)), id.clone()));
                    names.folders.insert(from.clone());
                    names.folders.insert(to.clone());
                    names.src.insert(id.clone(), from);
                    match role {
                        Role::Item => names.dst.insert(id.clone(), to),
                        Role::Original => names.bak.insert(id.clone(), to),
                    };
                    names.order.push((kind, id));
                }
                Action::Dir { .. } | Action::MoveCacheShard { .. } | Action::Leave { .. } => {}
            }
        }
        let data = text(&layout.data)?;
        names.exact.insert(layout.marker_file(), "Marker".into());
        names.exact.insert(layout.busy_file(), "Busy".into());
        names.temps.push((layout.data.clone(), ".layout.json.tmp-".into(), "MarkerTmp".into(), String::new()));
        names.folders.insert(data);
        Ok(names)
    }

    /// The model's name for `path`, if it has one.
    fn of_path(&self, path: &Path) -> Option<&str> {
        if let Some(expr) = self.exact.get(path) {
            return Some(expr);
        }
        let (dir, file) = (path.parent()?, path.file_name()?);
        let name = file.to_str()?;
        let temporary = gaze_fs::temp_owner(file).is_some();
        self.temps
            .iter()
            .find(|(folder, prefix, _, _)| temporary && folder == dir && name.starts_with(prefix.as_str()))
            .map(|(_, _, expr, _)| expr.as_str())
    }

    /// The id whose temporary file `path` is.
    fn temp_id(&self, path: &Path) -> Option<&str> {
        let (dir, name) = (path.parent()?, path.file_name()?.to_str()?);
        self.temps
            .iter()
            .find(|(folder, prefix, _, _)| folder == dir && name.starts_with(prefix.as_str()))
            .map(|(_, _, _, id)| id.as_str())
    }

    fn folder(&self, path: &Path) -> Option<String> {
        path.to_str().filter(|p| self.folders.contains(*p)).map(str::to_string)
    }
}

/// One event of the model's log.
fn record(op: &str, p: &str, q: &str, c: &str, d: &str, dirs: &str, keep: &str) -> String {
    format!("[op |-> {}, p |-> {p}, q |-> {q}, c |-> {c}, d |-> {d}, dirs |-> {dirs}, keep |-> {keep}]", string(op))
}

/// A set of TLA+ values.
fn set<I: IntoIterator<Item = String>>(items: I) -> String {
    let items: Vec<String> = items.into_iter().collect();
    format!("{{{}}}", items.join(", "))
}

/// A function from ids to strings, as `a :> x @@ b :> y`, or the empty
/// function.
fn function(map: &BTreeMap<String, String>) -> String {
    match map.is_empty() {
        true => "[x \\in {} |-> \"\"]".into(),
        false => map
            .iter()
            .map(|(k, v)| format!("({} :> {})", string(k), string(v)))
            .collect::<Vec<_>>()
            .join(" @@ "),
    }
}

/// The model's log of `case`'s events.
fn log(case: &Case<'_>, names: &Names) -> Result<(Vec<String>, usize, bool), String> {
    let mut out = Vec::with_capacity(case.events.len());
    let mut crashes = 0usize;
    let mut power = false;
    let mut barrier: Option<BTreeSet<String>> = None;
    let none = "None";
    let no_path = "NoPath";
    let empty = "{}";
    for (at, event) in case.events.iter().enumerate() {
        let t = match event {
            Event::ProcessCrash => {
                crashes += 1;
                out.push(record("crash-process", no_path, no_path, none, "\"\"", empty, empty));
                continue;
            }
            Event::PowerCut { pending, keep } => {
                crashes += 1;
                power = true;
                // The model's queue holds the modelled operations only, in
                // the same order: an operation is modelled when every name
                // it changes is.
                let mut index = 0usize;
                let mut kept = Vec::new();
                for (changes, &k) in pending.iter().zip(keep) {
                    let modelled = changes.iter().filter(|(p, _)| names.of_path(p).is_some()).count();
                    match (modelled, changes.len()) {
                        (0, _) => {}
                        (m, n) if m == n => {
                            index += 1;
                            if k {
                                kept.push(index.to_string());
                            }
                        }
                        _ => return Err(format!("event {at}: a pending operation changes modelled and other names: {changes:?}")),
                    }
                }
                out.push(record("crash-power", no_path, no_path, none, "\"\"", empty, &set(kept)));
                continue;
            }
            Event::Op(t) => t,
        };
        if t.op == "note" {
            match t.what.as_deref() {
                Some("barrier-begin") => barrier = Some(BTreeSet::new()),
                Some("barrier-end") => {
                    let dirs = barrier.take().ok_or_else(|| format!("event {at}: a barrier ends that never began"))?;
                    out.push(record("barrier", no_path, no_path, none, "\"\"", &set(dirs.iter().map(|d| string(d))), empty));
                }
                _ => {}
            }
            continue;
        }
        // A failed operation changed nothing.
        if t.error.is_some() {
            continue;
        }
        if let Some(dirs) = &mut barrier {
            if t.op == "sync_dir" {
                dirs.insert(text(&t.path)?);
            }
            continue;
        }
        let path = names.of_path(&t.path);
        let to = t.to.as_deref().and_then(|to| names.of_path(to));
        match t.op {
            "create_new" => {
                let Some(p) = path else { continue };
                let content = match p {
                    "Busy" | "MarkerTmp" => "Note".to_string(),
                    _ => {
                        let id = names.temp_id(&t.path).ok_or_else(|| format!("event {at}: {p} is created, but is no temporary file"))?;
                        match names.texts.get(id) {
                            Some(&fnv) if t.fnv == Some(fnv) => format!("Conv({})", string(id)),
                            Some(_) => return Err(format!("event {at}: {p} holds other bytes than its converted text")),
                            None => return Err(format!("event {at}: {p} is written, but {id} is not a write")),
                        }
                    }
                };
                out.push(record("create", p, no_path, &content, "\"\"", empty, empty));
            }
            "copy_new" => {
                let Some(target) = to else { continue };
                let temp = t.to.as_deref().expect("a copy has a target");
                let id = names.temp_id(temp).ok_or_else(|| format!("event {at}: a copy into {target}, which is no temporary file"))?;
                // The copy's content shows in the comparison that verifies
                // it, in the same process: whole, or not. A process that
                // died before comparing leaves what was copied, which is
                // whole unless copies may come out wrong.
                let mut verified = None;
                for next in &case.events[at + 1..] {
                    match next {
                        Event::ProcessCrash | Event::PowerCut { .. } => break,
                        Event::Op(next) if next.error.is_none() && next.op == "same_contents" && next.path == temp => {
                            verified = next.equal;
                            break;
                        }
                        Event::Op(_) => {}
                    }
                }
                let content = match (verified, case.bad_copies) {
                    (Some(true), _) | (None, false) => format!("Orig({})", string(id)),
                    (Some(false), _) => "Garbage".to_string(),
                    (None, true) => {
                        return Err(format!("event {at}: the copy into {target} was never compared, and copies may come out wrong"));
                    }
                };
                out.push(record("create", target, no_path, &format!("Partial({})", string(id)), "\"\"", empty, empty));
                out.push(record("write", target, no_path, &content, "\"\"", empty, empty));
            }
            "sync_file" => {
                if let Some(p) = path {
                    out.push(record("fsync-file", p, no_path, none, "\"\"", empty, empty));
                }
            }
            "sync_dir" => {
                if let Some(d) = names.folder(&t.path) {
                    out.push(record("fsync-dir", no_path, no_path, none, &string(&d), empty, empty));
                }
            }
            "hard_link" | "rename" => match (path, to) {
                (Some(a), Some(b)) => {
                    let op = if t.op == "hard_link" { "link" } else { "rename" };
                    out.push(record(op, a, b, none, "\"\"", empty, empty));
                }
                (None, None) => {}
                _ => {
                    return Err(format!(
                        "event {at}: {} {} → {} joins a modelled name with another",
                        t.op,
                        t.path.display(),
                        t.to.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
                    ));
                }
            },
            "remove_file" => {
                if let Some(p) = path {
                    out.push(record("unlink", p, no_path, none, "\"\"", empty, empty));
                }
            }
            // Reads, looks, listings, modes, folders made or removed: no
            // modelled name changes.
            _ => {}
        }
    }
    if barrier.is_some() {
        return Err("a barrier began and never ended".into());
    }
    Ok((out, crashes, power))
}

/// The TLA+ module and TLC configuration that check `case`:
/// `Trace_<name>.tla` and `Trace_<name>.cfg`.
pub fn tla(case: &Case<'_>) -> Result<(String, String), String> {
    if case.name.is_empty() || !case.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(format!("{:?} is not a TLA+ identifier", case.name));
    }
    let names = Names::of(case.layout, case.plan)?;
    let (events, crashes, power) = log(case, &names)?;
    let mut taken = Vec::with_capacity(case.taken.len());
    for path in case.taken {
        let expr = names.exact.get(path).ok_or_else(|| format!("{} is not a planned destination", path.display()))?;
        let id = expr
            .split_once('(')
            .and_then(|(_, rest)| rest.strip_suffix(')'))
            .ok_or_else(|| format!("{} is not a destination", path.display()))?;
        taken.push(id.to_string());
    }
    let far: Vec<String> = case
        .far
        .iter()
        .filter_map(|dir| names.folder(dir))
        .map(|d| string(&d))
        .collect();
    let module = format!("Trace_{}", case.name);
    let mut tla = String::with_capacity(4096 + 160 * events.len());
    let _ = writeln!(tla, "---- MODULE {module} ----");
    let _ = writeln!(tla, "\\* Generated by crates/gaze-shell/src/profile/migrate/trace.rs (ledger S8).");
    let _ = writeln!(tla, "EXTENDS ProfileStartupTrace");
    let order: Vec<String> = names
        .order
        .iter()
        .map(|(kind, id)| format!("<<{}, {}>>", string(kind), string(id)))
        .collect();
    let _ = writeln!(tla, "TR_Order == <<{}>>", order.join(", "));
    let _ = writeln!(tla, "TR_SrcDir == {}", function(&names.src));
    let _ = writeln!(tla, "TR_DstDir == {}", function(&names.dst));
    let _ = writeln!(tla, "TR_BakDir == {}", function(&names.bak));
    let _ = writeln!(tla, "TR_Far == {}", set(far));
    let _ = writeln!(tla, "TR_Checked == << >>");
    let _ = writeln!(tla, "TR_Taken == {}", set(taken));
    let _ = writeln!(tla, "TR_Start == [x \\in {{}} |-> \"missing\"]");
    let _ = writeln!(tla, "TR_Log == <<");
    for (i, event) in events.iter().enumerate() {
        let comma = if i + 1 < events.len() { "," } else { "" };
        let _ = writeln!(tla, "  {event}{comma}");
    }
    let _ = writeln!(tla, ">>");
    let _ = writeln!(tla, "====");
    let mut cfg = String::with_capacity(1024);
    let _ = writeln!(cfg, "\\* Generated by crates/gaze-shell/src/profile/migrate/trace.rs (ledger S8).");
    let _ = writeln!(cfg, "CONSTANTS");
    for (name, value) in [
        ("Order", "<- TR_Order".to_string()),
        ("SrcDir", "<- TR_SrcDir".into()),
        ("DstDir", "<- TR_DstDir".into()),
        ("BakDir", "<- TR_BakDir".into()),
        ("MarkerDir", format!("= {}", string(&text(&case.layout.data)?))),
        ("FarDirs", "<- TR_Far".into()),
        ("Checked", "<- TR_Checked".into()),
        ("MaxCrashes", format!("= {crashes}")),
        ("PowerLoss", format!("= {}", if power { "TRUE" } else { "FALSE" })),
        ("Ntfs", format!("= {}", if case.ntfs { "TRUE" } else { "FALSE" })),
        ("BadCopies", format!("= {}", if case.bad_copies { "TRUE" } else { "FALSE" })),
        ("TraceMode", "= TRUE".into()),
        ("Fault", "= \"none\"".into()),
        ("Log", "<- TR_Log".into()),
        ("TraceTaken", "<- TR_Taken".into()),
        ("TraceStart", "<- TR_Start".into()),
    ] {
        let _ = writeln!(cfg, "    {name} {value}");
    }
    let _ = writeln!(cfg, "INIT TraceInit");
    let _ = writeln!(cfg, "NEXT TraceNext");
    let _ = writeln!(cfg, "INVARIANT TraceEnd");
    let _ = writeln!(cfg, "CHECK_DEADLOCK FALSE");
    Ok((tla, cfg))
}

#[cfg(test)]
mod tests;
