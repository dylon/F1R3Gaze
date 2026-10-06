//! Moving an older F1R3Gaze's single-folder profile into the five roots, and
//! the marker that says a profile is in this layout (`data/layout.json`).
//! The design is in docs/storage/README.md and the model in
//! docs/storage/tla/ProfileStartup.tla (ledger S7).

pub mod convert;
#[cfg(any(test, feature = "storage-trace"))]
pub mod trace;

use super::layout::{Class, Layout};
use super::lock::utc_time;
use super::reconcile::Managed;
use super::report::{Event, EventKind, Report, Severity};
use crate::ui_state::StateError;
use gaze_fs::{Fs, Kind, PRIVATE_FILE_MODE, Perm};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The version of the layout this build keeps files in.
pub const LAYOUT_VERSION: u32 = 1;

/// The version of `plan.json` this build writes and replays.
pub const PLAN_FORMAT: u32 = 1;

/// `data/.migration/plan.json`: a migration decided once and then replayed
/// until it is done, so a crash at any step resumes it. Paths are UTF-8.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// [`PLAN_FORMAT`]; a larger one is never replayed.
    pub format: u32,
    /// The single-folder profile being moved.
    pub legacy: String,
    /// `config`, `data`, `state` and `cache`, as the plan was made: a start
    /// that sees other roots does not replay it.
    pub roots: BTreeMap<String, String>,
    /// The backups folder the originals go into, the same in every class.
    pub session: String,
    /// When the plan was made, in UTC (RFC 3339).
    pub planned: String,
    pub theme: ThemeNote,
    /// The settings converted, by name; never their values.
    pub settings: Vec<SettingNote>,
    /// Fixed phrases about sources that could not be converted.
    pub notes: Vec<String>,
    pub actions: Vec<Action>,
}

/// What became of the old theme.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeNote {
    /// The theme `workspace.json` named, if it named one.
    pub legacy: Option<String>,
    /// `system`, `default-light` or `custom`; nothing if no theme was moved.
    pub became: Option<String>,
    pub why: String,
}

/// A converted setting: its name, and whether its value can be used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingNote {
    pub key: String,
    pub usable: bool,
}

/// One step of a plan, in the order the plan lists them: every `Dir`, then
/// every `Write`, the item moves, the original moves, the cache shards,
/// and what is left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Action {
    /// A folder the moves need.
    Dir { path: String },
    /// A converted file: `text`, from the old files `from`.
    Write {
        item: String,
        from: Vec<String>,
        dst: String,
        text: String,
    },
    /// A file moved: an item to its new place, or an original into the
    /// backups.
    Move { item: String, role: Role, src: String, dst: String },
    /// A folder of the content cache, renamed whole.
    MoveCacheShard { src: String, dst: String },
    /// Something not moved, and why.
    Leave { path: String, why: String },
}

/// Why a file moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Into the new layout.
    Item,
    /// An original that was converted, kept in the backups.
    Original,
}

impl Plan {
    /// The folders whose entries the plan's actions change: those it
    /// creates, and those its files move out of and into.
    pub fn folders(&self) -> BTreeSet<PathBuf> {
        let mut folders = BTreeSet::new();
        let mut parent_of = |path: &str| {
            if let Some(dir) = Path::new(path).parent() {
                folders.insert(dir.to_path_buf());
            }
        };
        for action in &self.actions {
            match action {
                Action::Dir { path } => parent_of(path),
                Action::Write { dst, .. } => parent_of(dst),
                Action::Move { src, dst, .. } | Action::MoveCacheShard { src, dst } => {
                    parent_of(src);
                    parent_of(dst);
                }
                Action::Leave { .. } => {}
            }
        }
        for action in &self.actions {
            if let Action::Dir { path } = action {
                folders.insert(PathBuf::from(path));
            }
        }
        folders
    }
}

// ── M0: detection ────────────────────────────────────────────────────────

/// The files whose presence makes a folder an old F1R3Gaze profile. Folder
/// names alone never count.
pub const SIGNATURE: [&str; 7] = [
    "settings.conf",
    "workspace.json",
    "user-id",
    "wallets.tsv",
    "grants.tsv",
    "wallet-active",
    "palette.css",
];

/// The note left in an old profile once its files moved.
pub const MIGRATED_FILE: &str = "MIGRATED.txt";

/// What step 4 finds (design B.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detected {
    /// The profile is in this layout. `cleanup`: a finished migration's
    /// folder is still there.
    Current { cleanup: bool },
    /// A newer F1R3Gaze set the profile up.
    Newer { layout: u32 },
    /// A migration was planned and has not finished.
    Resume,
    /// An old profile to move.
    Legacy { root: PathBuf },
    /// Nothing to move.
    Fresh,
}

/// Whether `path` is a folder, following a link at `path` itself.
fn is_folder(fs: &dyn Fs, path: &Path) -> io::Result<bool> {
    match fs.kind(path) {
        Ok(Kind::Dir) => Ok(true),
        Ok(Kind::Symlink) => Ok(fs.list(path).is_ok()),
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Whether an old profile's file is there, as a file or a link.
fn has_file(fs: &dyn Fs, path: &Path) -> bool {
    matches!(fs.kind(path), Ok(Kind::File | Kind::Symlink))
}

/// The layout a newer F1R3Gaze wrote into the marker, if a newer one set
/// the profile up. Reads only: start-up asks before it writes anything, so
/// that such a profile gets no busy flag (README §9.7). A marker that is
/// missing, damaged or unreadable is not newer; reconcile deals with it.
pub fn newer_layout(layout: &Layout, fs: &dyn Fs) -> Option<u32> {
    match fs.read(&layout.marker_file()).map(|bytes| Marker::read(&bytes)) {
        Ok(Err(StateError::Newer { version, .. })) => Some(version),
        _ => None,
    }
}

/// Step 4's question: is there anything to migrate? Reads only.
pub fn detect(layout: &Layout, fs: &dyn Fs, report: &Report) -> Result<Detected, String> {
    let marker = layout.marker_file();
    match fs.kind(&marker) {
        Ok(_) => {
            // A marker that is damaged or cannot be read is reconcile's.
            if let Ok(bytes) = fs.read(&marker)
                && let Err(StateError::Newer { version, .. }) = Marker::read(&bytes)
            {
                return Ok(Detected::Newer { layout: version });
            }
            let cleanup = present(fs, &layout.migration_dir()).map_err(|e| format!("{}: {e}", layout.migration_dir().display()))?;
            return Ok(Detected::Current { cleanup });
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", marker.display())),
    }
    let plan_file = layout.migration_dir().join(PLAN_FILE);
    if present(fs, &plan_file).map_err(|e| format!("{}: {e}", plan_file.display()))? {
        return Ok(Detected::Resume);
    }
    let roots: Vec<&Path> = Class::ALL.iter().map(|&class| layout.root(class)).collect();
    let mut found: Option<PathBuf> = None;
    for root in &layout.legacy {
        if roots.iter().any(|r| root.starts_with(r)) {
            continue;
        }
        if !is_folder(fs, root).map_err(|e| format!("{}: {e}", root.display()))? || has_file(fs, &root.join(MIGRATED_FILE)) {
            continue;
        }
        let signed = SIGNATURE.iter().any(|name| has_file(fs, &root.join(name)));
        match (signed, &found) {
            (true, None) => found = Some(root.clone()),
            (true, Some(first)) => report.push(Event::new(
                EventKind::Other,
                Severity::Notice,
                root,
                format!(
                    "another old F1R3Gaze profile at {} was not moved: only the one at {} was",
                    root.display(),
                    first.display()
                ),
            )),
            (false, _) => {
                let keys = fs
                    .list(&root.join("keys"))
                    .map(|names| names.iter().any(|n| n.to_string_lossy().ends_with(".key")))
                    .unwrap_or(false);
                if keys {
                    report.push(Event::new(
                        EventKind::Other,
                        Severity::Notice,
                        root,
                        format!(
                            "{} holds wallet keys but nothing else F1R3Gaze recognises, so it was not moved",
                            root.display()
                        ),
                    ));
                }
            }
        }
    }
    Ok(match found {
        Some(root) => Detected::Legacy { root },
        None => Detected::Fresh,
    })
}

// ── M1: the plan ─────────────────────────────────────────────────────────

/// `path` as UTF-8, if it is.
fn utf8(path: &Path) -> Option<String> {
    path.to_str().map(str::to_string)
}

/// `path` as UTF-8, or an error naming it.
fn text_of(path: &Path) -> Result<String, String> {
    utf8(path).ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))
}

/// The roots a plan records, by class name.
fn roots_of(layout: &Layout) -> Result<BTreeMap<String, String>, String> {
    [Class::Config, Class::Data, Class::State, Class::Cache]
        .into_iter()
        .map(|class| Ok((class.name().to_string(), text_of(layout.root(class))?)))
        .collect()
}

/// A backup session name free in the config, data and state backups:
/// `stamp`, else `stamp.1`, `stamp.2`, …
fn free_session(layout: &Layout, fs: &dyn Fs, stamp: &str) -> Result<String, String> {
    for n in 0u32.. {
        let name = match n {
            0 => stamp.to_string(),
            n => format!("{stamp}.{n}"),
        };
        let mut free = true;
        for class in [Class::Config, Class::Data, Class::State] {
            let candidate = layout.backups(class).join(&name);
            if present(fs, &candidate).map_err(|e| format!("{}: {e}", candidate.display()))? {
                free = false;
            }
        }
        if free {
            return Ok(name);
        }
    }
    unreachable!("u32 session numbers run out only after 4 billion migrations in one second")
}

/// Whether `name` is a wallet key's file in an old `keys/` folder.
fn key_file(name: &str) -> bool {
    let stem = name.strip_suffix(".key").or_else(|| name.strip_suffix(".tmp"));
    stem.is_some_and(|stem| stem.len() == 64 && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Whether `name` is a site's store or the store index in an old `store/`.
fn store_file(name: &str) -> bool {
    name == "origins.json"
        || name
            .strip_suffix(".gzs")
            .is_some_and(|stem| stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Whether `name` is a replay log in an old `logs/`.
fn log_file(name: &str) -> bool {
    name.strip_suffix(".gzlog").is_some_and(|stem| !stem.is_empty())
}

/// Whether `name` is a shard of the old content cache.
fn cache_shard(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Builds a plan's actions while the old profile is read.
struct Planner<'a> {
    layout: &'a Layout,
    fs: &'a dyn Fs,
    session: String,
    writes: Vec<Action>,
    items: Vec<Action>,
    originals: Vec<Action>,
    shards: Vec<Action>,
    left: Vec<Action>,
    notes: Vec<String>,
}

impl Planner<'_> {
    fn leave(&mut self, path: &Path, why: &str) -> Result<(), String> {
        self.left.push(Action::Leave {
            path: text_of(path)?,
            why: why.to_string(),
        });
        Ok(())
    }

    /// The file `src` moves to `dst` (design B.4): a file, or a link to an
    /// absolute target, which moves as a link.
    fn file_item(&mut self, item: &str, src: &Path, dst: PathBuf) -> Result<(), String> {
        match self.fs.kind(src).map_err(|e| format!("{}: {e}", src.display()))? {
            Kind::File => {}
            Kind::Symlink => match self.fs.read_link(src) {
                Ok(target) if target.is_absolute() => {}
                Ok(_) => return self.leave(src, "a relative link, which would point elsewhere once moved"),
                Err(e) => return Err(format!("{}: {e}", src.display())),
            },
            Kind::Dir | Kind::Other => return self.leave(src, "not a file"),
        }
        self.items.push(Action::Move {
            item: item.to_string(),
            role: Role::Item,
            src: text_of(src)?,
            dst: text_of(&dst)?,
        });
        Ok(())
    }

    /// The files of an old folder that `accepts`, each into `to`.
    fn folder(&mut self, name: &str, dir: &Path, to: &Path, accepts: fn(&str) -> bool) -> Result<(), String> {
        match self.fs.kind(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            Kind::Dir => {}
            Kind::Symlink => return self.leave(dir, "a linked folder: left as it is"),
            Kind::File | Kind::Other => return self.leave(dir, "not a folder"),
        }
        for entry in sorted(self.fs, dir)? {
            let Some(child) = entry.to_str() else { continue };
            let path = dir.join(child);
            match accepts(child) {
                true => self.file_item(&format!("{name}/{child}"), &path, to.join(child))?,
                false => self.leave(&path, "not part of a F1R3Gaze profile")?,
            }
        }
        Ok(())
    }

    /// The old content cache's shards, each renamed whole into the new one.
    fn cache(&mut self, dir: &Path, portable: bool) -> Result<(), String> {
        match self.fs.kind(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            Kind::Dir => {}
            Kind::Symlink => return self.leave(dir, "a linked folder: left as it is"),
            Kind::File | Kind::Other => return self.leave(dir, "not a folder"),
        }
        let content = self.layout.content_dir();
        for entry in sorted(self.fs, dir)? {
            let Some(child) = entry.to_str() else { continue };
            let path = dir.join(child);
            if portable && path == content {
                continue;
            }
            let is_dir = matches!(self.fs.kind(&path), Ok(Kind::Dir));
            match is_dir && cache_shard(child) {
                true => self.shards.push(Action::MoveCacheShard {
                    src: text_of(&path)?,
                    dst: text_of(&content.join(child))?,
                }),
                false => self.leave(&path, "not part of F1R3Gaze's cache")?,
            }
        }
        Ok(())
    }

    /// The folder that keeps the originals converted from the old profile.
    fn kept_in(&self, class: Class) -> PathBuf {
        self.layout.backups(class).join(&self.session).join("legacy-profile")
    }

    /// Where an original converted from the old profile is kept.
    fn kept_at(&self, class: Class, name: &str) -> PathBuf {
        self.kept_in(class).join(name)
    }
}

/// The names in `dir`, sorted.
fn sorted(fs: &dyn Fs, dir: &Path) -> Result<Vec<std::ffi::OsString>, String> {
    let mut names = fs.list(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    names.sort();
    Ok(names)
}

/// What an old file holds, read through a link: its text, or why there is
/// none. `Ok(None)`: it is not there.
fn read_text(fs: &dyn Fs, path: &Path) -> Result<Option<String>, String> {
    match fs.read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "it is not UTF-8 text".to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !has_file(fs, path) => Ok(None),
        Err(e) => Err(format!("it cannot be read ({e})")),
    }
}

/// M1: the plan for moving the old profile at `legacy`. Reads only.
pub fn plan(layout: &Layout, fs: &dyn Fs, legacy: &Path, now: SystemTime) -> Result<Plan, String> {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let session = free_session(layout, fs, &super::backup::backup_stamp(seconds))?;
    let portable = matches!(&layout.kind, super::layout::Kind::Portable(root) if root == legacy);
    let mut p = Planner {
        layout,
        fs,
        session,
        writes: Vec::with_capacity(3),
        items: Vec::with_capacity(16),
        originals: Vec::with_capacity(2),
        shards: Vec::with_capacity(256),
        left: Vec::with_capacity(8),
        notes: Vec::with_capacity(4),
    };
    for entry in sorted(fs, legacy)? {
        // A name that is not UTF-8 gets no action; MIGRATED.txt lists it.
        let Some(name) = entry.to_str() else { continue };
        let path = legacy.join(name);
        match name {
            "settings.conf" | "workspace.json" | MIGRATED_FILE => {}
            "palette.css" => p.file_item(name, &path, layout.themes_dir().join("custom.css"))?,
            "user-id" => p.file_item(name, &path, layout.user_id_file())?,
            "wallets.tsv" | "wallet-active" => p.file_item(name, &path, layout.wallet_dir().join(name))?,
            "grants.tsv" => p.file_item(name, &path, layout.grants_file())?,
            "keys" => p.folder(name, &path, &layout.keys_dir(), key_file)?,
            "exports" => p.folder(name, &path, &layout.exports_dir(), |_| true)?,
            "store" => p.folder(name, &path, &layout.site_data_dir(), store_file)?,
            "logs" => p.folder(name, &path, &layout.replay_logs_dir(), log_file)?,
            "cache" => p.cache(&path, portable)?,
            "config" | "data" | "state" | "runtime" if portable => {}
            _ => p.leave(&path, "not part of a F1R3Gaze profile")?,
        }
    }

    // workspace.json: the open tabs and the history, and the old theme.
    let workspace = legacy.join("workspace.json");
    let mut from_workspace = false;
    let mut theme_legacy: Option<String> = None;
    let fold = match read_text(fs, &workspace) {
        Ok(None) => convert::ThemeFold::Unchanged,
        Ok(Some(text)) => match serde_json::from_str::<crate::ui_state::UiState>(&text) {
            Ok(state) => {
                let (session_state, history, theme) = state.split();
                for (dst, bytes) in [(layout.session_file(), session_state.to_bytes()), (layout.history_file(), history.to_bytes())] {
                    p.writes.push(Action::Write {
                        item: "workspace.json".into(),
                        from: vec![text_of(&workspace)?],
                        dst: text_of(&dst)?,
                        text: String::from_utf8(bytes).expect("state files are JSON, which is UTF-8"),
                    });
                }
                from_workspace = true;
                theme_legacy = Some(theme.clone());
                match theme.as_str() {
                    "light" => convert::ThemeFold::Light,
                    "custom" => {
                        let palette = legacy.join("palette.css");
                        let custom = layout.themes_dir().join("custom.css");
                        let movable = matches!(fs.kind(&palette), Ok(Kind::File))
                            && match fs.kind(&custom) {
                                Err(e) if e.kind() == io::ErrorKind::NotFound => true,
                                Ok(Kind::File) => fs.same_contents(&palette, &custom).unwrap_or(false),
                                _ => false,
                            };
                        match movable {
                            true => convert::ThemeFold::Custom,
                            false => {
                                p.notes.push(
                                    "Your theme was \"custom\", but its palette.css could not be moved to themes/custom.css, so F1R3Gaze follows your system's light or dark setting.".into(),
                                );
                                convert::ThemeFold::System { was: Some(theme) }
                            }
                        }
                    }
                    _ => convert::ThemeFold::System { was: Some(theme) },
                }
            }
            Err(_) => {
                p.notes.push("workspace.json could not be read, so the open tabs and the history start empty.".into());
                convert::ThemeFold::System { was: None }
            }
        },
        Err(why) => {
            p.notes.push(format!("workspace.json could not be used ({why}), so the open tabs and the history start empty."));
            convert::ThemeFold::System { was: None }
        }
    };
    match fs.kind(&workspace) {
        Ok(Kind::File) => p.originals.push(Action::Move {
            item: "workspace.json".into(),
            role: Role::Original,
            src: text_of(&workspace)?,
            dst: text_of(&p.kept_at(Class::State, "workspace.json"))?,
        }),
        Ok(_) => p.leave(&workspace, match from_workspace {
            true => "converted; the link was left",
            false => "not a file that could be read",
        })?,
        Err(_) => {}
    }

    // settings.conf, with the theme folded in.
    let conf_path = legacy.join("settings.conf");
    let conf = match read_text(fs, &conf_path) {
        Ok(conf) => conf,
        Err(why) => {
            p.notes.push(format!("settings.conf could not be converted ({why}), so settings.toml starts from the template."));
            None
        }
    };
    let themed = matches!(fold, convert::ThemeFold::Light | convert::ThemeFold::Custom);
    let mut settings_notes = Vec::new();
    if conf.is_some() || themed {
        let converted = convert::convert(conf.as_deref(), &fold)?;
        let mut from = Vec::with_capacity(2);
        if conf.is_some() {
            from.push(text_of(&conf_path)?);
        }
        if themed {
            from.push(text_of(&workspace)?);
        }
        settings_notes = converted
            .written
            .iter()
            .map(|(key, usable)| SettingNote { key: key.clone(), usable: *usable })
            .collect();
        p.writes.insert(
            0,
            Action::Write {
                item: "settings.conf".into(),
                from,
                dst: text_of(&layout.settings_file())?,
                text: converted.text,
            },
        );
    }
    match fs.kind(&conf_path) {
        Ok(Kind::File) => p.originals.insert(
            0,
            Action::Move {
                item: "settings.conf".into(),
                role: Role::Original,
                src: text_of(&conf_path)?,
                dst: text_of(&p.kept_at(Class::Config, "settings.conf"))?,
            },
        ),
        Ok(_) => p.leave(&conf_path, match conf.is_some() {
            true => "converted; the link was left",
            false => "not a file that could be read",
        })?,
        Err(_) => {}
    }

    let theme = ThemeNote {
        legacy: theme_legacy.clone(),
        became: match &fold {
            convert::ThemeFold::Unchanged => None,
            convert::ThemeFold::System { .. } => Some("system".into()),
            convert::ThemeFold::Light => Some("default-light".into()),
            convert::ThemeFold::Custom => Some("custom".into()),
        },
        why: match (&fold, theme_legacy.as_deref()) {
            (convert::ThemeFold::Unchanged, _) => "there was no workspace.json".into(),
            (convert::ThemeFold::System { .. }, Some("dark")) => "dark was the old default, so it may never have been chosen".into(),
            (convert::ThemeFold::System { .. }, Some("custom")) => "the custom palette could not be moved".into(),
            (convert::ThemeFold::System { .. }, Some(_)) => "the old theme is not one F1R3Gaze knows".into(),
            (convert::ThemeFold::System { .. }, None) => "workspace.json could not be read".into(),
            (convert::ThemeFold::Light, _) => "it was chosen".into(),
            (convert::ThemeFold::Custom, _) => "it was chosen, and its palette moved to themes/custom.css".into(),
        },
    };

    // M2's folders: every destination's parent, the new cache's folder, and
    // the session in the config and state backups.
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for action in p.writes.iter().chain(&p.items).chain(&p.originals).chain(&p.shards) {
        if let Action::Write { dst, .. } | Action::Move { dst, .. } | Action::MoveCacheShard { dst, .. } = action
            && let Some(parent) = Path::new(dst).parent()
        {
            dirs.insert(parent.to_path_buf());
        }
    }
    dirs.insert(layout.content_dir());
    dirs.insert(p.kept_in(Class::Config));
    dirs.insert(p.kept_in(Class::State));
    let mut actions = Vec::with_capacity(dirs.len() + p.writes.len() + p.items.len() + p.originals.len() + p.shards.len() + p.left.len());
    for dir in dirs {
        actions.push(Action::Dir { path: text_of(&dir)? });
    }
    actions.append(&mut p.writes);
    actions.append(&mut p.items);
    actions.append(&mut p.originals);
    actions.append(&mut p.shards);
    actions.append(&mut p.left);
    Ok(Plan {
        format: PLAN_FORMAT,
        legacy: text_of(legacy)?,
        roots: roots_of(layout)?,
        session: p.session,
        planned: utc_time(seconds),
        theme,
        settings: settings_notes,
        notes: p.notes,
        actions,
    })
}

/// Reads `plan.json`, refusing one this build must not replay.
fn read_plan(layout: &Layout, fs: &dyn Fs) -> Result<Plan, String> {
    let path = layout.migration_dir().join(PLAN_FILE);
    let bytes = fs.read(&path).map_err(|e| format!("{} cannot be read ({e})", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("{} is damaged ({e})", path.display()))?;
    match value.get("format").and_then(serde_json::Value::as_u64) {
        Some(format) if format > u64::from(PLAN_FORMAT) => {
            return Err(format!(
                "a newer F1R3Gaze started the move (plan format {format}); start that F1R3Gaze to finish it"
            ));
        }
        _ => {}
    }
    let plan: Plan = serde_json::from_value(value).map_err(|e| format!("{} is damaged ({e})", path.display()))?;
    let roots = roots_of(layout)?;
    if plan.roots != roots {
        return Err(format!(
            "the move was planned for other folders ({}); start F1R3Gaze with the environment it had then to finish it",
            plan.roots.values().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(plan)
}

/// M1's write: the plan, durable before anything moves.
fn write_plan(layout: &Layout, fs: &dyn Fs, plan: &Plan) -> io::Result<()> {
    let dir = layout.migration_dir();
    gaze_fs::create_dir_durably(fs, &dir)?;
    gaze_fs::sweep_temps_of(fs, &dir, std::ffi::OsStr::new(PLAN_FILE))?;
    let mut bytes = serde_json::to_vec_pretty(plan).expect("a plan always serializes");
    bytes.push(b'\n');
    gaze_fs::write_new(fs, &dir.join(PLAN_FILE), &bytes, Perm::Private)
}

// ── M2–M9: the replay ────────────────────────────────────────────────────

/// What became of one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Done now.
    Done,
    /// Done by an earlier start that did not finish.
    AlreadyDone,
    /// Something else is at the destination: both are kept.
    Conflict,
    /// Neither the source nor the destination is there.
    Missing,
    /// Not moved, and why.
    Left(String),
}

/// The destination's own stale temporary files go first (the model's
/// `MigrateBegin` sweeps `TmpOf(a)`).
fn sweep_for(fs: &dyn Fs, dst: &Path) -> io::Result<()> {
    let (Ok(dir), Some(name)) = (gaze_fs::parent(dst), dst.file_name()) else {
        return Ok(());
    };
    gaze_fs::sweep_temps_of(fs, dir, name).map(drop)
}

/// ⟨write a converted file⟩ (design B.5).
fn write_converted(fs: &dyn Fs, dst: &Path, text: &str) -> io::Result<Outcome> {
    for _ in 0..2 {
        match fs.read(dst) {
            Ok(bytes) if bytes == text.as_bytes() => return Ok(Outcome::AlreadyDone),
            Ok(_) => return Ok(Outcome::Conflict),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !has_file(fs, dst) => {
                match gaze_fs::write_new(fs, dst, text.as_bytes(), Perm::Private) {
                    Ok(()) => return Ok(Outcome::Done),
                    // Something appeared meanwhile: look again.
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
            // A link to nothing, or a folder: something is there.
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::IsADirectory => {
                return Ok(Outcome::Conflict);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Outcome::Conflict)
}

/// The kind of `path`, `None` when nothing is there; links not followed.
fn kind_at(fs: &dyn Fs, path: &Path) -> io::Result<Option<Kind>> {
    match fs.kind(path) {
        Ok(kind) => Ok(Some(kind)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Removes `src`, which `dst` already holds, and syncs its folder.
fn retire(fs: &dyn Fs, src: &Path) -> io::Result<()> {
    fs.remove_file(src)?;
    fs.sync_dir(gaze_fs::parent(src)?)
}

/// ⟨move a file⟩ (design B.5): never replacing anything at `dst`.
fn move_file(fs: &dyn Fs, src: &Path, dst: &Path) -> io::Result<Outcome> {
    // A second look happens only when something appeared at `dst` between
    // the first look and the move: then it is a conflict, or the same file.
    let mut outcome = Outcome::Conflict;
    for _ in 0..2 {
        match move_once(fs, src, dst) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            result => {
                outcome = result?;
                break;
            }
        }
    }
    // A moved file is private: its mode only tightens. Links are never
    // changed (it would change their target).
    if matches!(outcome, Outcome::Done | Outcome::AlreadyDone)
        && let Ok(Kind::File) = fs.kind(dst)
        && let Ok(Some(mode)) = fs.mode(dst)
        && mode & 0o7777 != mode & 0o600
    {
        fs.set_mode(dst, mode & 0o600)?;
    }
    Ok(outcome)
}

/// One look at `src` and `dst`, and the move it calls for. `AlreadyExists`
/// when `dst` appeared after the look: nothing was moved.
fn move_once(fs: &dyn Fs, src: &Path, dst: &Path) -> io::Result<Outcome> {
    Ok(match (kind_at(fs, src)?, kind_at(fs, dst)?) {
        (None, Some(_)) => Outcome::AlreadyDone,
        (None, None) => Outcome::Missing,
        (Some(Kind::Symlink), None) => {
            // A link moves as a link: renamed after checking the name is
            // free (hard links of links differ between systems). The
            // instance lock keeps every other F1R3Gaze out of the gap
            // between the check and the rename, as in publish_no_replace
            // on file systems without hard links.
            match fs.rename(src, dst) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                    return Ok(Outcome::Left("a link cannot be moved to another file system".into()));
                }
                Err(e) => return Err(e),
            }
            fs.sync_dir(gaze_fs::parent(dst)?)?;
            fs.sync_dir(gaze_fs::parent(src)?)?;
            Outcome::Done
        }
        (Some(_), None) => {
            gaze_fs::move_no_replace(fs, src, dst)?;
            Outcome::Done
        }
        (Some(Kind::Symlink), Some(Kind::Symlink)) => match fs.read_link(src)? == fs.read_link(dst)? {
            true => {
                retire(fs, src)?;
                Outcome::AlreadyDone
            }
            false => Outcome::Conflict,
        },
        (Some(Kind::File), Some(Kind::File)) => match fs.same_contents(src, dst)? {
            // An earlier start published the copy but died before it
            // removed the source; the barrier made the copy durable.
            true => {
                retire(fs, src)?;
                Outcome::AlreadyDone
            }
            false => Outcome::Conflict,
        },
        (Some(_), Some(_)) => Outcome::Conflict,
    })
}

/// ⟨move a cache shard⟩ (design B.5). The cache can be fetched again, so it
/// never fails a migration: what cannot be moved is left.
fn move_shard(fs: &dyn Fs, src: &Path, dst: &Path) -> Outcome {
    match (kind_at(fs, src), kind_at(fs, dst)) {
        (Ok(None), Ok(Some(_))) => Outcome::AlreadyDone,
        (Ok(None), Ok(None)) => Outcome::Missing,
        (Ok(Some(_)), Ok(Some(_))) => Outcome::Left("already in the new cache".into()),
        (Ok(Some(_)), Ok(None)) => {
            let moved = fs
                .rename(src, dst)
                .and_then(|()| fs.sync_dir(gaze_fs::parent(dst)?))
                .and_then(|()| fs.sync_dir(gaze_fs::parent(src)?));
            match moved {
                Ok(()) => Outcome::Done,
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => Outcome::Left(FAR_CACHE.into()),
                Err(e) => Outcome::Left(format!("it could not be moved ({e}); F1R3Gaze fetches it again, so you may delete it")),
            }
        }
        (Err(e), _) | (_, Err(e)) => Outcome::Left(format!("it could not be looked at ({e})")),
    }
}

/// Why the old cache stays where it is on another file system.
const FAR_CACHE: &str = "on another file system; F1R3Gaze fetches it again, so you may delete it";

/// What one replay did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub legacy: PathBuf,
    /// Files moved into the new layout (now or by an earlier start).
    pub moved: usize,
    /// Files written from converted ones.
    pub converted: usize,
    /// The old files not moved because something was already there.
    pub conflicts: Vec<String>,
    /// What stays in the old folder, as written in `MIGRATED.txt`.
    pub left: Vec<String>,
    /// Whether an earlier start began it.
    pub resumed: bool,
}

/// What step 4 did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Migration {
    /// The profile was already in this layout.
    Current,
    /// There was nothing to migrate; the marker was made.
    Fresh,
    /// An old profile was moved.
    Done(Summary),
    /// An old profile waits for a session that may write.
    Pending { legacy: PathBuf },
    /// A newer F1R3Gaze set the profile up: read only.
    Newer { layout: u32 },
    /// The move stopped: read only; the next start resumes it.
    Failed { error: String },
}

/// Step 4 (design B.8): detect, then plan and replay, or resume. Every
/// step can be repeated after a crash; `begin`'s barrier runs first, since
/// a profile without a marker has one.
pub fn run(start: &super::reconcile::Start<'_>, now: SystemTime) -> Migration {
    let (l, fs, report) = (start.layout, start.fs, start.report);
    let writable = matches!(start.access, super::Access::Write);
    let failed = |error: String| {
        report.push(Event::new(
            EventKind::MigrationPending { error: error.clone() },
            Severity::Alert,
            l.migration_dir(),
            format!(
                "the move of your old profile stopped: {error}. F1R3Gaze opened read only, and finishes the move at the next start"
            ),
        ));
        Migration::Failed { error }
    };
    let detected = match detect(l, fs, report) {
        Ok(detected) => detected,
        Err(e) => return failed(e),
    };
    match detected {
        Detected::Newer { layout } => {
            report.push(Event::new(
                EventKind::Newer { version: layout, known: LAYOUT_VERSION },
                Severity::Alert,
                l.marker_file(),
                format!("this profile was set up by a newer F1R3Gaze (layout {layout}; this one knows {LAYOUT_VERSION}), so nothing is written to it"),
            ));
            Migration::Newer { layout }
        }
        Detected::Current { cleanup } => match (cleanup, writable) {
            (true, true) => match clean_up(l, fs, report) {
                Ok(()) => Migration::Current,
                Err(e) => failed(format!("{}: {e}", l.migration_dir().display())),
            },
            _ => Migration::Current,
        },
        Detected::Fresh => match writable {
            true => match mark_fresh(l, fs, now, report) {
                Ok(_) => Migration::Fresh,
                Err(e) => failed(format!("{}: {e}", l.marker_file().display())),
            },
            false => Migration::Fresh,
        },
        Detected::Legacy { root } => {
            let planned = match plan(l, fs, &root, now) {
                Ok(planned) => planned,
                Err(e) => return failed(e),
            };
            if !writable {
                return pending(report, &root, &planned);
            }
            match write_plan(l, fs, &planned) {
                Ok(()) => execute(start, &planned, false),
                // A plan appeared: replay that one.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match read_plan(l, fs) {
                    Ok(written) => execute(start, &written, true),
                    Err(e) => failed(e),
                },
                Err(e) => failed(format!("the plan could not be written ({e})")),
            }
        }
        Detected::Resume => match read_plan(l, fs) {
            Ok(planned) if writable => execute(start, &planned, true),
            Ok(planned) => pending(report, Path::new(&planned.legacy), &planned),
            Err(e) => failed(e),
        },
    }
}

/// A migration this session may not do, reported.
fn pending(report: &Report, legacy: &Path, plan: &Plan) -> Migration {
    let moves = plan.actions.iter().filter(|a| matches!(a, Action::Move { .. } | Action::Write { .. })).count();
    report.push(Event::new(
        EventKind::MigrationPending {
            error: "this session only reads".into(),
        },
        Severity::Alert,
        legacy,
        format!(
            "the old profile at {} would be moved into the new folders ({moves} files), but this session only reads",
            legacy.display()
        ),
    ));
    Migration::Pending {
        legacy: legacy.to_path_buf(),
    }
}

/// M2–M9: the plan's actions, then `MIGRATED.txt`, the marker, and the
/// clean-up.
fn execute(start: &super::reconcile::Start<'_>, plan: &Plan, resumed: bool) -> Migration {
    let (l, fs, report) = (start.layout, start.fs, start.report);
    // The plan this start replays, for a trace of a real run to be checked
    // against the model with (`trace::plan_of`; ledger S8, part 2). A real
    // file system ignores notes.
    fs.note(&format!("plan {}", serde_json::to_string(plan).expect("a plan always serializes")));
    let mut outcomes = Vec::with_capacity(plan.actions.len());
    let mut far_cache = false;
    for action in &plan.actions {
        let step = match action {
            Action::Dir { path } => gaze_fs::create_dir_durably(fs, Path::new(path)).map(|_| Outcome::Done),
            Action::Write { dst, text, .. } => {
                sweep_for(fs, Path::new(dst)).and_then(|()| write_converted(fs, Path::new(dst), text))
            }
            Action::Move { src, dst, .. } => {
                sweep_for(fs, Path::new(dst)).and_then(|()| move_file(fs, Path::new(src), Path::new(dst)))
            }
            Action::MoveCacheShard { src, dst } => Ok(match far_cache {
                true => Outcome::Left(FAR_CACHE.into()),
                false => {
                    let outcome = move_shard(fs, Path::new(src), Path::new(dst));
                    far_cache = outcome == Outcome::Left(FAR_CACHE.into());
                    outcome
                }
            }),
            Action::Leave { why, .. } => Ok(Outcome::Left(why.clone())),
        };
        match step {
            Ok(outcome) => outcomes.push(outcome),
            Err(e) => {
                let what = match action {
                    Action::Dir { path } => path.clone(),
                    Action::Write { dst, .. } => dst.clone(),
                    Action::Move { src, .. } | Action::MoveCacheShard { src, .. } => src.clone(),
                    Action::Leave { path, .. } => path.clone(),
                };
                return failed_at(report, l, &what, &e);
            }
        }
    }
    let legacy = PathBuf::from(&plan.legacy);
    // M6b: the old profile's folders the moves emptied (`keys/`, `exports/`,
    // `store/`, `logs/`, `cache/`), so the folder holds only the note and
    // what was not moved, as the note says. Only an empty folder can be
    // removed: one that still holds something not moved stays, and the note
    // lists what is in it. One that is gone already (a resumed start) or
    // cannot be removed stays as it is. (Until ledger S14 they were left.)
    for dir in emptied_folders(plan) {
        let _ = fs.remove_dir(&dir);
    }
    // The removals are not synced here. M7 next writes the note into the
    // same folder atomically, which syncs the folder, and with it these
    // removals (on Windows its rename's flush commits them: W2). If the
    // start stops first, the next start's barrier syncs the plan's folders,
    // and this one is among them: every plan moves a file out of it.
    // Was, until the mutation check M14ad showed that it changed nothing:
    // if removed && let Err(e) = fs.sync_dir(&legacy) {
    //     return failed_at(report, l, &legacy.display().to_string(), &e);
    // }
    let note = migrated_text(l, fs, plan, &outcomes);
    // M7: the note, before the marker, so a crash never leaves an emptied
    // folder without one.
    let written = gaze_fs::sweep_temps_of(fs, &legacy, std::ffi::OsStr::new(MIGRATED_FILE))
        .and_then(|_| gaze_fs::write_atomic(fs, &legacy.join(MIGRATED_FILE), note.as_bytes(), Perm::Private));
    if let Err(e) = written {
        return failed_at(report, l, &legacy.join(MIGRATED_FILE).display().to_string(), &e);
    }
    // M8: the marker, after a dead start's temporary marker is swept (the
    // model's MarkerStep: Unlink(MarkerTmp), then FsyncDir(MarkerDir)).
    let marker_file = l.marker_file();
    let swept = match marker_file.file_name() {
        Some(name) => gaze_fs::sweep_temps_of(fs, &l.data, name).map(drop),
        None => Ok(()),
    };
    if let Err(e) = swept {
        return failed_at(report, l, &l.data.display().to_string(), &e);
    }
    let marker = Marker {
        layout: LAYOUT_VERSION,
        created: Some(plan.planned.clone()),
        migrated_from: Some(plan.legacy.clone()),
        repaired: false,
    };
    match gaze_fs::write_new(fs, &l.marker_file(), &marker.to_bytes(), Perm::Private) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return failed_at(report, l, &l.marker_file().display().to_string(), &e),
    }
    // M9: the plan goes.
    if let Err(e) = clean_up(l, fs, report) {
        report.push(Event::new(
            EventKind::Other,
            Severity::Notice,
            l.migration_dir(),
            format!("{} could not be removed ({e}); the next start tries again", l.migration_dir().display()),
        ));
    }
    let summary = summarize(plan, &outcomes, resumed);
    for (action, outcome) in plan.actions.iter().zip(&outcomes) {
        if let (Action::Move { item, dst, .. } | Action::Write { item, dst, .. }, Outcome::Conflict) = (action, outcome) {
            report.push(Event::new(
                EventKind::Conflict { other: PathBuf::from(dst) },
                Severity::Warning,
                PathBuf::from(dst),
                format!("{item} was not moved from the old profile: {dst} already exists. Both are kept"),
            ));
        }
    }
    if let Some(convert_note) = theme_notice(&plan.theme) {
        report.push(Event::new(EventKind::Other, Severity::Notice, l.settings_file(), convert_note));
    }
    report.push(Event::new(
        EventKind::Migrated {
            from: legacy.clone(),
            moved: summary.moved + summary.converted,
            conflicts: summary.conflicts.len(),
        },
        Severity::Notice,
        &legacy,
        format!(
            "your old profile at {} was moved into the new folders; {} says where each file went",
            legacy.display(),
            legacy.join(MIGRATED_FILE).display()
        ),
    ));
    Migration::Done(summary)
}

/// The old profile's folders the plan moves items out of, deepest first:
/// the ones a finished move leaves empty.
fn emptied_folders(plan: &Plan) -> Vec<PathBuf> {
    let legacy = Path::new(&plan.legacy);
    let mut folders = BTreeSet::new();
    for action in &plan.actions {
        let src = match action {
            Action::Move { src, .. } | Action::MoveCacheShard { src, .. } => Path::new(src),
            Action::Dir { .. } | Action::Write { .. } | Action::Leave { .. } => continue,
        };
        // Every folder between the item and the old profile's own.
        let mut dir = src.parent();
        while let Some(d) = dir
            && d != legacy
            && d.starts_with(legacy)
        {
            folders.insert(d.to_path_buf());
            dir = d.parent();
        }
    }
    let mut folders: Vec<PathBuf> = folders.into_iter().collect();
    folders.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    folders
}

/// A failed step, reported: read only, and resumed at the next start.
fn failed_at(report: &Report, l: &Layout, what: &str, e: &io::Error) -> Migration {
    let error = format!("{what}: {e}");
    report.push(Event::new(
        EventKind::MigrationPending { error: error.clone() },
        Severity::Alert,
        l.migration_dir(),
        format!("the move of your old profile stopped at {what}: {e}. F1R3Gaze opened read only, and finishes the move at the next start"),
    ));
    Migration::Failed { error }
}

/// M9: removes the plan and the migration's folder. Anything else found
/// there is left, and reported.
fn clean_up(l: &Layout, fs: &dyn Fs, report: &Report) -> io::Result<()> {
    let dir = l.migration_dir();
    let plan_file = dir.join(PLAN_FILE);
    if present(fs, &plan_file)? {
        fs.remove_file(&plan_file)?;
        fs.sync_dir(&dir)?;
    }
    let mut others = Vec::new();
    let mut removed = 0usize;
    for name in fs.list(&dir)? {
        let text = name.to_string_lossy();
        match text.starts_with(&format!(".{PLAN_FILE}.tmp-")) && gaze_fs::temp_owner(&name).is_some() {
            true => {
                fs.remove_file(&dir.join(&name))?;
                removed += 1;
            }
            false => others.push(text.into_owned()),
        }
    }
    // Each removal is synced in its folder before the folder goes, so
    // nothing is left pending (ReadyIsDurable).
    if removed > 0 {
        fs.sync_dir(&dir)?;
    }
    if !others.is_empty() {
        report.push(Event::new(
            EventKind::Other,
            Severity::Notice,
            &dir,
            format!("{} holds files F1R3Gaze did not put there ({}); it is left", dir.display(), others.join(", ")),
        ));
        return Ok(());
    }
    fs.remove_dir(&dir)?;
    fs.sync_dir(&l.data)
}

/// The counts and lists of a replay.
fn summarize(plan: &Plan, outcomes: &[Outcome], resumed: bool) -> Summary {
    let mut summary = Summary {
        legacy: PathBuf::from(&plan.legacy),
        moved: 0,
        converted: 0,
        conflicts: Vec::new(),
        left: Vec::new(),
        resumed,
    };
    for (action, outcome) in plan.actions.iter().zip(outcomes) {
        match (action, outcome) {
            (Action::Move { role: Role::Item, .. } | Action::MoveCacheShard { .. }, Outcome::Done | Outcome::AlreadyDone) => {
                summary.moved += 1;
            }
            (Action::Write { .. }, Outcome::Done | Outcome::AlreadyDone) => summary.converted += 1,
            (Action::Move { item, .. } | Action::Write { item, .. }, Outcome::Conflict) => summary.conflicts.push(item.clone()),
            (Action::Leave { path, .. }, _)
            | (Action::Move { src: path, .. } | Action::MoveCacheShard { src: path, .. }, Outcome::Left(_)) => {
                summary.left.push(path.clone());
            }
            _ => {}
        }
    }
    summary
}

/// The Notice about the theme, when the old one was not kept as such.
fn theme_notice(theme: &ThemeNote) -> Option<String> {
    match (theme.became.as_deref(), theme.legacy.as_deref()) {
        (Some("system"), Some("dark")) => Some(
            "your theme was dark, the old default, so F1R3Gaze now follows your system's light or dark setting. To keep it dark, choose Dark in Appearance".into(),
        ),
        (Some("system"), Some(other)) => Some(format!(
            "your theme \"{other}\" could not be kept, so F1R3Gaze follows your system's light or dark setting"
        )),
        _ => None,
    }
}

// ── MIGRATED.txt ─────────────────────────────────────────────────────────

/// `path` as `<class>/<below>` when it is under a root, else in full.
fn shown(l: &Layout, path: &Path) -> String {
    match l.class_of(path) {
        Some((class, below)) => format!("{}/{}", class.name(), below.display()),
        None => path.display().to_string(),
    }
}

/// `text` padded to `width` columns.
fn padded(text: &str, width: usize) -> String {
    let count = text.chars().count();
    match count < width {
        true => format!("{text}{}", " ".repeat(width - count)),
        false => format!("{text} "),
    }
}

/// `MIGRATED.txt` (design B.9). It names files and folders only: never an
/// address, a setting's value, a tab's title, or the history.
fn migrated_text(l: &Layout, fs: &dyn Fs, plan: &Plan, outcomes: &[Outcome]) -> String {
    const COLUMN: usize = 24;
    let legacy = Path::new(&plan.legacy);
    let (date, time) = plan.planned.split_once('T').unwrap_or((&plan.planned, ""));
    let mut out = String::with_capacity(2048);
    out.push_str(&format!(
        "F1R3Gaze moved this profile on {date} at {} UTC.\n\n",
        time.trim_end_matches('Z')
    ));
    out.push_str("F1R3Gaze now keeps its files in four places:\n");
    for class in [Class::Config, Class::Data, Class::State, Class::Cache] {
        out.push_str(&format!("  {}{}\n", padded(class.name(), 8), l.root(class).display()));
    }
    let mut moved = String::new();
    let mut shards = 0usize;
    let mut not_moved = String::new();
    let mut reasons: BTreeMap<String, String> = BTreeMap::new();
    for (action, outcome) in plan.actions.iter().zip(outcomes) {
        match (action, outcome) {
            (Action::Move { item, role: Role::Item, dst, .. }, Outcome::Done | Outcome::AlreadyDone) => {
                moved.push_str(&format!("  {}{}\n", padded(item, COLUMN), shown(l, Path::new(dst))));
            }
            (Action::MoveCacheShard { .. }, Outcome::Done | Outcome::AlreadyDone) => shards += 1,
            (Action::Move { item, dst, .. } | Action::Write { item, dst, .. }, Outcome::Conflict) => {
                not_moved.push_str(&format!("  {}{} already exists\n", padded(item, COLUMN), shown(l, Path::new(dst))));
                if let Action::Move { src, .. } = action {
                    reasons.insert(
                        src.clone(),
                        format!("not moved: {} already exists; both are kept", shown(l, Path::new(dst))),
                    );
                }
            }
            (Action::Move { src, .. }, Outcome::Left(why)) | (Action::MoveCacheShard { src, .. }, Outcome::Left(why)) => {
                reasons.insert(src.clone(), why.clone());
            }
            (Action::Leave { path, why }, _) => {
                reasons.insert(path.clone(), why.clone());
            }
            _ => {}
        }
    }
    if shards > 0 {
        let folders = match shards {
            1 => "1 folder".to_string(),
            n => format!("{n} folders"),
        };
        moved.push_str(&format!("  {}{}/\n", padded(&format!("cache/ ({folders})"), COLUMN), shown(l, &l.content_dir())));
    }
    if !moved.is_empty() {
        out.push_str("\nMoved\n");
        out.push_str(&moved);
    }
    let converted = converted_text(l, plan, outcomes);
    if !converted.is_empty() {
        out.push_str("Converted\n");
        out.push_str(&converted);
    }
    if !not_moved.is_empty() {
        out.push_str("Not moved (something was already there; both are kept)\n");
        out.push_str(&not_moved);
    }
    // A portable profile's own folders are not the old profile's.
    let own: Vec<PathBuf> = match &l.kind {
        super::layout::Kind::Portable(root) if root == legacy => {
            vec![l.config.clone(), l.data.clone(), l.state.clone(), l.runtime.clone(), l.content_dir()]
        }
        _ => Vec::new(),
    };
    let left = left_here(fs, legacy, &reasons, &own);
    if !left.is_empty() {
        out.push_str("Left here\n");
        out.push_str(&left);
    }
    out.push_str(
        "\nAn older F1R3Gaze started now finds this folder empty: its settings, wallets and keys are in the folders above.\n",
    );
    out
}

/// The "Converted" part of `MIGRATED.txt`.
fn converted_text(l: &Layout, plan: &Plan, outcomes: &[Outcome]) -> String {
    const COLUMN: usize = 24;
    let indent = " ".repeat(COLUMN + 2);
    let mut out = String::new();
    let written = |dst: &Path| {
        plan.actions.iter().zip(outcomes).any(|(a, o)| {
            matches!((a, o), (Action::Write { dst: d, .. }, Outcome::Done | Outcome::AlreadyDone) if Path::new(d) == dst)
        })
    };
    let original = |item: &str| {
        plan.actions.iter().zip(outcomes).find_map(|(a, o)| match (a, o) {
            (Action::Move { item: i, role: Role::Original, dst, .. }, Outcome::Done | Outcome::AlreadyDone) if i == item => {
                Some(shown(l, Path::new(dst)))
            }
            _ => None,
        })
    };
    let text_of_write = |dst: &Path| {
        plan.actions.iter().find_map(|a| match a {
            Action::Write { dst: d, text, .. } if Path::new(d) == dst => Some(text.clone()),
            _ => None,
        })
    };
    let settings_file = l.settings_file();
    let set: Vec<&SettingNote> = plan.settings.iter().filter(|s| s.key != "appearance.theme").collect();
    if written(&settings_file) || original("settings.conf").is_some() {
        let what = match (written(&settings_file), set.len()) {
            (false, _) => "not converted: settings.toml starts from the template".to_string(),
            (true, 0) => format!("{} (no settings were set: it is the new template)", shown(l, &settings_file)),
            (true, 1) => format!("{} (1 setting)", shown(l, &settings_file)),
            (true, n) => format!("{} ({n} settings)", shown(l, &settings_file)),
        };
        out.push_str(&format!("  {}{what}\n", padded("settings.conf", COLUMN)));
        if let Some(kept) = original("settings.conf") {
            out.push_str(&format!("{indent}the original is kept in {kept}\n"));
        }
    }
    let (session_file, history_file) = (l.session_file(), l.history_file());
    if written(&session_file) || original("workspace.json").is_some() {
        let tabs = text_of_write(&session_file)
            .and_then(|t| crate::ui_state::SessionState::read(t.as_bytes()).ok())
            .map_or(0, |s| s.tabs.len());
        let visits = text_of_write(&history_file)
            .and_then(|t| crate::ui_state::History::read(t.as_bytes()).ok())
            .map_or(0, |h| h.visits.len());
        let count = |n: usize, one: &str, several: &str| match n {
            1 => format!("1 {one}"),
            n => format!("{n} {several}"),
        };
        let planned = |dst: &Path| {
            plan.actions.iter().any(|a| matches!(a, Action::Write { dst: d, .. } if Path::new(d) == dst))
        };
        let tabs_part = format!("{} ({})", shown(l, &session_file), count(tabs, "tab", "tabs"));
        let visits_part = format!("{} ({})", shown(l, &history_file), count(visits, "visit", "visits"));
        let what = match (planned(&session_file), written(&session_file), written(&history_file)) {
            (false, _, _) => "not converted: the open tabs and the history start empty".to_string(),
            (true, true, true) => format!("{tabs_part} and {visits_part}"),
            (true, true, false) => format!("{tabs_part}; {} was already there", shown(l, &history_file)),
            (true, false, true) => format!("{visits_part}; {} was already there", shown(l, &session_file)),
            (true, false, false) => format!(
                "not converted: {} and {} were already there",
                shown(l, &session_file),
                shown(l, &history_file)
            ),
        };
        out.push_str(&format!("  {}{what}\n", padded("workspace.json", COLUMN)));
        if let Some(kept) = original("workspace.json") {
            out.push_str(&format!("{indent}the original is kept in {kept}\n"));
        }
    }
    match (plan.theme.became.as_deref(), plan.theme.legacy.as_deref()) {
        (Some("system"), Some("dark")) => out.push_str(
            "  Theme: your theme was \"dark\", the old default, so F1R3Gaze now follows your system's light or\n  dark setting. To keep it dark, choose Dark in Appearance, or set theme = \"default-dark\"\n  under [appearance] in settings.toml.\n",
        ),
        (Some("system"), Some(other)) => out.push_str(&format!(
            "  Theme: your theme \"{other}\" could not be kept, so F1R3Gaze follows your system's light or dark\n  setting. Choose a theme in Appearance.\n"
        )),
        (Some("default-light"), _) => out.push_str("  Theme: light, as before (theme = \"default-light\").\n"),
        (Some("custom"), _) => out.push_str("  Theme: your palette.css is now themes/custom.css (theme = \"custom\").\n"),
        _ => {}
    }
    let unusable: Vec<&str> = set.iter().filter(|s| !s.usable).map(|s| s.key.as_str()).collect();
    if !unusable.is_empty() {
        out.push_str("  Settings F1R3Gaze cannot use as written (it uses their defaults until you change them):\n");
        for key in unusable {
            out.push_str(&format!("  {key}\n"));
        }
    }
    for note in &plan.notes {
        out.push_str(&format!("  {note}\n"));
    }
    out
}

/// The "Left here" part: a fresh listing of the old folder and its item
/// folders, so names the plan could not hold (not UTF-8) or that appeared
/// later are listed too.
fn left_here(fs: &dyn Fs, legacy: &Path, reasons: &BTreeMap<String, String>, own: &[PathBuf]) -> String {
    const COLUMN: usize = 24;
    let mut out = String::new();
    let mut line = |path: &Path, shown_name: String| {
        let why = utf8(path)
            .and_then(|p| reasons.get(&p).cloned())
            .unwrap_or_else(|| "not part of a F1R3Gaze profile".into());
        out.push_str(&format!("  {}{why}\n", padded(&shown_name, COLUMN)));
    };
    let Ok(names) = sorted(fs, legacy) else {
        return out;
    };
    for name in names {
        let path = legacy.join(&name);
        let text = name.to_string_lossy().into_owned();
        if text == MIGRATED_FILE || text.starts_with(&format!(".{MIGRATED_FILE}.tmp-")) || own.contains(&path) {
            continue;
        }
        match (text.as_str(), fs.kind(&path)) {
            ("keys" | "exports" | "store" | "logs" | "cache", Ok(Kind::Dir)) => {
                if let Ok(children) = sorted(fs, &path) {
                    for child in children {
                        let child_path = path.join(&child);
                        if !own.contains(&child_path) {
                            line(&child_path, format!("{text}/{}", child.to_string_lossy()));
                        }
                    }
                }
            }
            (_, Ok(Kind::Dir)) => line(&path, format!("{text}/")),
            _ => line(&path, text),
        }
    }
    out
}

/// The folders the barrier syncs: every folder start-up writes into, so
/// that, like the model's `Barrier`, it leaves nothing pending.
/// - The skeleton, and each root's ancestors up to the file system's root:
///   a start creates the data root (and its parents) before it can set its
///   busy flag, and a later start finds those folders without knowing
///   whether their names were synced.
/// - Every folder of the backups: a repair that did not finish may have
///   left the name of its copy pending (the next start makes a new copy).
/// - The folder of each managed file's link target, where writes through
///   the link make their temporary files and renames.
/// - The migration's folder and, while its plan can be read, every folder
///   the plan changes.
pub fn barrier_folders(layout: &Layout, fs: &dyn Fs) -> BTreeSet<PathBuf> {
    let mut folders: BTreeSet<PathBuf> = layout.skeleton().into_iter().collect();
    for class in Class::ALL {
        folders.extend(layout.root(class).ancestors().skip(1).map(Path::to_path_buf));
        backup_folders(fs, &layout.backups(class), &mut folders);
    }
    for m in Managed::ALL {
        let path = m.path(layout);
        if let Ok(Kind::Symlink) = fs.kind(&path)
            && let Ok(target) = gaze_fs::resolve_link(fs, &path)
            && let Ok(dir) = gaze_fs::parent(&target)
        {
            folders.insert(dir.to_path_buf());
        }
    }
    folders.insert(layout.migration_dir());
    if let Ok(bytes) = fs.read(&layout.migration_dir().join(PLAN_FILE))
        && let Ok(plan) = serde_json::from_slice::<Plan>(&bytes)
    {
        folders.extend(plan.folders());
    }
    folders
}

/// `dir` and every folder below it, if it is a folder.
fn backup_folders(fs: &dyn Fs, dir: &Path, folders: &mut BTreeSet<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        match fs.kind(&dir) {
            Ok(Kind::Dir) => {}
            _ => continue,
        }
        if let Ok(names) = fs.list(&dir) {
            stack.extend(names.into_iter().map(|name| dir.join(name)));
        }
        folders.insert(dir);
    }
}

/// The plan's file name, in `data/.migration`.
pub const PLAN_FILE: &str = "plan.json";

/// Start-up's step 3, the model's `Barrier`: syncs every folder of
/// [`barrier_folders`] that exists, so what a start that did not finish
/// left pending is durable before this one decides anything from what it
/// sees. A folder above the roots that cannot be opened is skipped: no
/// start could have created anything in it. Returns how many folders it
/// synced.
pub fn barrier(layout: &Layout, fs: &dyn Fs) -> io::Result<usize> {
    fs.note("barrier-begin");
    let roots: Vec<&Path> = Class::ALL.iter().map(|&class| layout.root(class)).collect();
    let mut synced = 0;
    for dir in barrier_folders(layout, fs) {
        match fs.kind(&dir) {
            Ok(Kind::Dir | Kind::Symlink) => match fs.sync_dir(&dir) {
                Ok(()) => synced += 1,
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied && !roots.iter().any(|root| dir.starts_with(root)) => {}
                Err(e) => return Err(e),
            },
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    fs.note("barrier-end");
    Ok(synced)
}

/// What [`begin`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begun {
    /// The last start finished: no barrier.
    Clean,
    /// The last start did not finish, or the profile has no marker yet: the
    /// barrier synced this many folders.
    AfterUnfinished { synced: usize },
}

/// Start-up's step 3, the model's `StartStep`, with the instance lock held
/// and before anything is written. The barrier runs if the last start did
/// not finish: its busy flag is there, or the profile has no marker yet (a
/// start that died while it made the data root, before it could set the
/// flag, leaves no flag, and nothing else about a profile without a marker
/// is known to be durable). Then the flag is set, not synced: a power cut
/// leaves nothing pending, so the start after one needs no barrier whether
/// or not the flag survived. The data root is created first if it is
/// missing, durably.
pub fn begin(layout: &Layout, fs: &dyn Fs) -> io::Result<Begun> {
    gaze_fs::create_dir_durably(fs, &layout.data)?;
    let busy = layout.busy_file();
    let flagged = present(fs, &busy)?;
    let unfinished = flagged || !present(fs, &layout.marker_file())?;
    let begun = match unfinished {
        true => Begun::AfterUnfinished {
            synced: barrier(layout, fs)?,
        },
        false => Begun::Clean,
    };
    if !flagged {
        fs.create_new(&busy, b"", PRIVATE_FILE_MODE)?;
    }
    Ok(begun)
}

/// Whether something is at `path` (a dangling link counts).
fn present(fs: &dyn Fs, path: &Path) -> io::Result<bool> {
    match fs.kind(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Step 4 for a profile with nothing to migrate: the marker, made without
/// replacing anything (`write_new`), so that later starts know the profile
/// is in this layout. A marker already there is left to reconcile. Returns
/// whether it made one.
pub fn mark_fresh(layout: &Layout, fs: &dyn Fs, now: SystemTime, report: &Report) -> io::Result<bool> {
    let path = layout.marker_file();
    if present(fs, &path)? {
        return Ok(false);
    }
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let marker = Marker {
        layout: LAYOUT_VERSION,
        created: Some(utc_time(seconds)),
        migrated_from: None,
        repaired: false,
    };
    match gaze_fs::write_new(fs, &path, &marker.to_bytes(), Perm::Private) {
        Ok(()) => {
            report.push(Event::new(
                EventKind::Created,
                Severity::Quiet,
                &path,
                "layout.json was created: a new profile",
            ));
            Ok(true)
        }
        // Another writer made one meanwhile: reconcile checks it.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Start-up's last step, the model's `FinishStep`: the busy flag goes, and
/// its removal is synced, so nothing is pending once start-up is done
/// (`ReadyIsDurable`).
pub fn finish(layout: &Layout, fs: &dyn Fs) -> io::Result<()> {
    match fs.remove_file(&layout.busy_file()) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs.sync_dir(&layout.data)
}

/// `data/layout.json`: the profile is in the five-root layout. Written once,
/// as the migration's last step (or at a fresh profile's first start).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub layout: u32,
    /// When the profile was set up, in UTC (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// The single-folder profile it was moved from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
    /// Rewritten by start-up after it was found damaged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repaired: bool,
}

impl Marker {
    /// Reads a marker. Its `layout` is read on its own first, so a newer
    /// layout is recognised whatever the rest of the file holds.
    pub fn read(bytes: &[u8]) -> Result<Marker, StateError> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| StateError::Corrupt(e.to_string()))?;
        match value.get("layout").and_then(serde_json::Value::as_u64) {
            None => Err(StateError::Corrupt("no layout number".into())),
            Some(0) => Err(StateError::Corrupt("layout 0".into())),
            Some(layout) if layout > u64::from(LAYOUT_VERSION) => Err(StateError::Newer {
                version: u32::try_from(layout).unwrap_or(u32::MAX),
                known: LAYOUT_VERSION,
            }),
            Some(_) => serde_json::from_value(value).map_err(|e| StateError::Corrupt(e.to_string())),
        }
    }

    /// The marker start-up writes when it finds one damaged.
    pub fn repaired() -> Marker {
        Marker {
            layout: LAYOUT_VERSION,
            created: None,
            migrated_from: None,
            repaired: true,
        }
    }

    /// Its bytes: compact JSON and a line break.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(self).expect("a marker always serializes");
        bytes.push(b'\n');
        bytes
    }
}

#[cfg(test)]
mod tests;
