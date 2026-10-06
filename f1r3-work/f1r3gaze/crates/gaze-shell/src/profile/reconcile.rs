//! Start-up's repair of the profile (ledger S6; the model's reconcile phase,
//! docs/storage/tla/ProfileStartup.tla).
//!
//! [`prepare`] makes the folders, then sweeps the temporary files of writers
//! that died. [`reconcile`] then checks each managed file:
//! - **valid:** it is never written;
//! - **written by a newer F1R3Gaze:** left as it is, and not saved to;
//! - **damaged:** a durable copy goes into the backups first, then the
//!   repair (its usable part, or its default) is renamed over the file, so
//!   the file is never absent and its content is never only in a backup;
//! - **missing:** its default is created without replacing anything;
//! - **unreadable:** left as it is, and not saved to.
//!
//! In a read-only session both only say what they would do. A start after
//! one that finished writes nothing: each repair's result passes its own
//! check.

use super::backup::Backups;
use super::layout::{Class, Layout};
use super::migrate::Marker;
use super::report::{Event, EventKind, Report, Severity};
use super::settings;
use super::{Access, KeystoreKind};
use crate::grants::check_grants;
use crate::site_index::{index_bytes, read_index, salvage_index};
use crate::theme::{Scheme, example_css};
use crate::ui_state::{History, SessionState, StateError};
use crate::window_state::WindowState;
use gaze_fs::{Fs, Kind, LineCheck, Perm};
use gaze_shard::fresh::check_records;
use gaze_wallet::{Address, KeyFile, KeyScan, check_active, check_list, entry_line, parse_list, recover_entries, scan_keys, wallet_of_key_file};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// What start-up works with.
pub struct Start<'a> {
    pub layout: &'a Layout,
    /// The file system `backups` writes to.
    pub fs: &'a dyn Fs,
    pub backups: &'a Backups,
    pub report: &'a Report,
    pub access: &'a Access,
    pub keystore: KeystoreKind,
    /// Makes a new user id (random in use, fixed in tests).
    pub new_user_id: &'a dyn Fn() -> String,
}

/// The files start-up checks and repairs, in the order it visits them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Managed {
    Settings,
    SettingsExample,
    DarkExample,
    LightExample,
    Marker,
    UserId,
    WalletList,
    ActiveWallet,
    Grants,
    SiteIndex,
    Freshness,
    Window,
    Session,
    History,
}

/// One event a repair reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub kind: EventKind,
    pub severity: Severity,
    pub message: String,
}

/// How a damaged file is repaired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Its new content, written over it.
    Replace { bytes: Vec<u8>, notes: Vec<Note> },
    /// Removed: its repaired state is "absent" (`wallet-active`: no wallet
    /// pays until one is chosen).
    Remove { notes: Vec<Note> },
}

/// What a file's content says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Valid,
    /// Written by a newer F1R3Gaze, in a format this one does not know.
    Newer { version: u32, known: u32 },
    Damaged { why: String, fix: Fix },
}

/// What to do about a missing file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// Nothing: its owner creates it when there is something to keep.
    Leave,
    /// Nothing: the migration writes it (the marker).
    Migration,
    /// Created with this content.
    Create { bytes: Vec<u8>, notes: Vec<Note> },
}

/// What the checks may look at besides the file itself.
pub struct CheckInput<'a> {
    /// The keystore folder, read (empty with the OS keystore).
    pub keys: &'a KeyScan,
    pub new_user_id: &'a dyn Fn() -> String,
}

fn note(kind: EventKind, severity: Severity, message: impl Into<String>) -> Note {
    Note {
        kind,
        severity,
        message: message.into(),
    }
}

/// `n` and the word for one or several of it.
fn count(n: usize, one: &str, several: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {several}"),
    }
}

/// Whether a user id can be used: UTF-8, 1 to 256 bytes once trimmed, and
/// no white space or control character inside.
pub fn check_user_id(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
    let id = text.trim();
    match id.len() {
        0 => Err("empty".into()),
        n if n > 256 => Err(format!("{n} bytes long")),
        _ if id.chars().any(|c| c.is_whitespace() || c.is_control()) => Err("white space inside".into()),
        _ => Ok(()),
    }
}

/// A state file's check: valid, newer, or damaged and replaced by `default`.
fn state_check(read: Result<(), StateError>, name: &str, default: Vec<u8>) -> Check {
    match read {
        Ok(()) => Check::Valid,
        Err(StateError::Newer { version, known }) => Check::Newer { version, known },
        Err(StateError::Corrupt(why)) => Check::Damaged {
            fix: Fix::Replace {
                bytes: default,
                notes: vec![note(
                    EventKind::Regenerated { why: why.clone() },
                    Severity::Warning,
                    format!("{name} was damaged ({why}); it starts again from its default"),
                )],
            },
            why,
        },
    }
}

/// How many of a line file's kept lines hold something.
fn kept_lines(kept: &[u8]) -> usize {
    kept.split(|b| *b == b'\n').filter(|line| !line.trim_ascii().is_empty()).count()
}

/// "1 line of N could not be read and was left out", or "were" for several.
fn left_out(dropped: usize, name: &str) -> String {
    format!(
        "{} of {name} could not be read and {} left out",
        count(dropped, "line", "lines"),
        match dropped {
            1 => "was",
            _ => "were",
        }
    )
}

/// A line file's check: valid, or damaged and cut back to its usable lines.
/// `also` follows the message, for what the loss means.
fn lines_check(lines: LineCheck, name: &str, severity: Severity, also: &str) -> Check {
    match lines.is_clean() {
        true => Check::Valid,
        false => {
            let kept = kept_lines(&lines.kept);
            let dropped = lines.dropped.len();
            let why = format!("{} could not be read", count(dropped, "line", "lines"));
            Check::Damaged {
                fix: Fix::Replace {
                    bytes: lines.kept,
                    notes: vec![note(
                        EventKind::Repaired { kept, dropped },
                        severity,
                        format!("{}{also}", left_out(dropped, name)),
                    )],
                },
                why,
            }
        }
    }
}

/// The wallet list's lines with the recovered wallets appended.
fn list_with(kept: &[u8], recovered: &[gaze_wallet::Entry]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(kept.len() + 96 * recovered.len());
    bytes.extend_from_slice(kept);
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    for entry in recovered {
        bytes.extend_from_slice(entry_line(entry).as_bytes());
    }
    bytes
}

fn recovered_notes(recovered: &[gaze_wallet::Entry]) -> Vec<Note> {
    recovered
        .iter()
        .map(|e| {
            note(
                EventKind::Recovered { address: e.address.to_string() },
                Severity::Warning,
                format!(
                    "the wallet {} was listed again from its key file, as \"{}\"",
                    e.address,
                    gaze_wallet::RECOVERED_LABEL
                ),
            )
        })
        .collect()
}

impl Managed {
    pub const ALL: [Managed; 14] = [
        Managed::Settings,
        Managed::SettingsExample,
        Managed::DarkExample,
        Managed::LightExample,
        Managed::Marker,
        Managed::UserId,
        Managed::WalletList,
        Managed::ActiveWallet,
        Managed::Grants,
        Managed::SiteIndex,
        Managed::Freshness,
        Managed::Window,
        Managed::Session,
        Managed::History,
    ];

    pub fn path(self, l: &Layout) -> PathBuf {
        match self {
            Managed::Settings => l.settings_file(),
            Managed::SettingsExample => l.settings_example(),
            Managed::DarkExample => l.themes_dir().join("default-dark.css.example"),
            Managed::LightExample => l.themes_dir().join("default-light.css.example"),
            Managed::Marker => l.marker_file(),
            Managed::UserId => l.user_id_file(),
            Managed::WalletList => l.wallet_dir().join(gaze_wallet::LIST_FILE),
            Managed::ActiveWallet => l.wallet_dir().join(gaze_wallet::ACTIVE_FILE),
            Managed::Grants => l.grants_file(),
            Managed::SiteIndex => l.origins_file(),
            Managed::Freshness => l.trust_file(),
            Managed::Window => l.window_file(),
            Managed::Session => l.session_file(),
            Managed::History => l.history_file(),
        }
    }

    pub fn class(self) -> Class {
        match self {
            Managed::Settings | Managed::SettingsExample | Managed::DarkExample | Managed::LightExample => Class::Config,
            Managed::Marker
            | Managed::UserId
            | Managed::WalletList
            | Managed::ActiveWallet
            | Managed::Grants
            | Managed::SiteIndex
            | Managed::Freshness => Class::Data,
            Managed::Window | Managed::Session | Managed::History => Class::State,
        }
    }

    /// Its path below its root, for messages.
    pub fn name(self) -> &'static str {
        match self {
            Managed::Settings => "settings.toml",
            Managed::SettingsExample => "settings.toml.example",
            Managed::DarkExample => "themes/default-dark.css.example",
            Managed::LightExample => "themes/default-light.css.example",
            Managed::Marker => "layout.json",
            Managed::UserId => "user-id",
            Managed::WalletList => "wallet/wallets.tsv",
            Managed::ActiveWallet => "wallet/wallet-active",
            Managed::Grants => "permissions/grants.tsv",
            Managed::SiteIndex => "site-data/origins.json",
            Managed::Freshness => "trust/freshness.tsv",
            Managed::Window => "window.json",
            Managed::Session => "session.json",
            Managed::History => "history.json",
        }
    }

    /// The mode of a rewrite: a file the user may have opened up keeps its
    /// mode; everything else is the owner's alone.
    pub fn perm(self) -> Perm {
        match self {
            Managed::Settings | Managed::SettingsExample | Managed::DarkExample | Managed::LightExample => Perm::Preserve,
            _ => Perm::Private,
        }
    }

    /// How much a file that cannot be read matters.
    pub fn unreadable(self) -> Severity {
        match self {
            // Without them, a stale answer could be accepted.
            Managed::Freshness => Severity::Alert,
            _ => Severity::Warning,
        }
    }

    /// The bytes F1R3Gaze itself writes for this file, if it has fixed text.
    fn builtin(self) -> Option<Vec<u8>> {
        match self {
            Managed::SettingsExample => Some(settings::TEMPLATE.as_bytes().to_vec()),
            Managed::DarkExample => Some(example_css(Scheme::Dark).into_bytes()),
            Managed::LightExample => Some(example_css(Scheme::Light).into_bytes()),
            _ => None,
        }
    }

    /// What the content `bytes` of the file at `path` says.
    pub fn check(self, path: &Path, bytes: &[u8], input: &CheckInput<'_>) -> Check {
        match self {
            Managed::Settings => match settings::check(path, bytes) {
                Ok(()) => Check::Valid,
                Err(diagnostic) => {
                    let why = diagnostic.problem.clone();
                    Check::Damaged {
                        fix: Fix::Replace {
                            bytes: settings::TEMPLATE.as_bytes().to_vec(),
                            notes: vec![note(
                                EventKind::Regenerated { why: why.clone() },
                                Severity::Warning,
                                format!("{diagnostic}; settings.toml was replaced by the template"),
                            )],
                        },
                        why,
                    }
                }
            },
            Managed::SettingsExample | Managed::DarkExample | Managed::LightExample => {
                let builtin = self.builtin().expect("examples have fixed text");
                match bytes == builtin.as_slice() {
                    true => Check::Valid,
                    false => Check::Damaged {
                        why: "it differs from F1R3Gaze's own text".into(),
                        fix: Fix::Replace {
                            bytes: builtin,
                            notes: vec![note(
                                EventKind::Refreshed,
                                Severity::Quiet,
                                format!("{} differed from F1R3Gaze's own text and was rewritten", self.name()),
                            )],
                        },
                    },
                }
            }
            Managed::Marker => match Marker::read(bytes) {
                Ok(_) => Check::Valid,
                Err(StateError::Newer { version, known }) => Check::Newer { version, known },
                Err(StateError::Corrupt(why)) => Check::Damaged {
                    fix: Fix::Replace {
                        bytes: Marker::repaired().to_bytes(),
                        notes: vec![note(
                            EventKind::Regenerated { why: why.clone() },
                            Severity::Warning,
                            format!("layout.json was damaged ({why}) and was rewritten"),
                        )],
                    },
                    why,
                },
            },
            Managed::UserId => match check_user_id(bytes) {
                Ok(()) => Check::Valid,
                Err(why) => {
                    let mut id = (input.new_user_id)();
                    id.push('\n');
                    Check::Damaged {
                        fix: Fix::Replace {
                            bytes: id.into_bytes(),
                            notes: vec![note(
                                EventKind::Regenerated { why: why.clone() },
                                Severity::Alert,
                                format!("user-id was damaged ({why}); a new id was made, so sites see this profile as a new user"),
                            )],
                        },
                        why,
                    }
                }
            },
            Managed::WalletList => {
                let lines = check_list(bytes);
                let listed = parse_list(&lines.kept);
                let recovered = recover_entries(input.keys, &listed);
                match (lines.is_clean(), recovered.is_empty()) {
                    (true, true) => Check::Valid,
                    (clean, _) => {
                        let mut notes = Vec::with_capacity(1 + recovered.len());
                        let dropped = lines.dropped.len();
                        if !clean {
                            notes.push(note(
                                EventKind::Repaired { kept: listed.len(), dropped },
                                Severity::Warning,
                                left_out(dropped, self.name()),
                            ));
                        }
                        notes.extend(recovered_notes(&recovered));
                        let why = match clean {
                            true => format!(
                                "{} with a key file {} not listed",
                                count(recovered.len(), "wallet", "wallets"),
                                match recovered.len() {
                                    1 => "is",
                                    _ => "are",
                                }
                            ),
                            false => format!("{} could not be read", count(dropped, "line", "lines")),
                        };
                        Check::Damaged {
                            why,
                            fix: Fix::Replace {
                                bytes: list_with(&lines.kept, &recovered),
                                notes,
                            },
                        }
                    }
                }
            }
            Managed::ActiveWallet => match check_active(bytes) {
                Ok(()) => Check::Valid,
                Err(why) => Check::Damaged {
                    fix: Fix::Remove {
                        notes: vec![note(
                            EventKind::Regenerated { why: why.clone() },
                            Severity::Warning,
                            format!("wallet/wallet-active was damaged ({why}); no wallet pays until you choose one"),
                        )],
                    },
                    why,
                },
            },
            Managed::Grants => lines_check(
                check_grants(bytes),
                self.name(),
                Severity::Warning,
                "; those sites ask again",
            ),
            Managed::SiteIndex => match read_index(bytes) {
                Ok(_) => Check::Valid,
                Err(why) => {
                    let (kept, dropped) = salvage_index(bytes);
                    Check::Damaged {
                        fix: Fix::Replace {
                            bytes: index_bytes(&kept),
                            notes: vec![note(
                                EventKind::Repaired { kept: kept.len(), dropped },
                                Severity::Warning,
                                format!("site-data/origins.json was damaged ({why}); {} kept", count(kept.len(), "site was", "sites were")),
                            )],
                        },
                        why,
                    }
                }
            },
            Managed::Freshness => lines_check(
                check_records(bytes),
                self.name(),
                Severity::Alert,
                ", so a stale answer for the names they recorded could be accepted once",
            ),
            Managed::Window => state_check(WindowState::read(bytes).map(drop), "window.json", WindowState::default().to_bytes()),
            Managed::Session => state_check(SessionState::read(bytes).map(drop), "session.json", SessionState::default().to_bytes()),
            Managed::History => state_check(History::read(bytes).map(drop), "history.json", History::default().to_bytes()),
        }
    }

    /// What to do when the file is missing.
    pub fn missing(self, input: &CheckInput<'_>) -> Missing {
        let created = |bytes: Vec<u8>| Missing::Create {
            bytes,
            notes: vec![note(EventKind::Created, Severity::Quiet, format!("{} was created", self.name()))],
        };
        match self {
            Managed::Settings => created(settings::TEMPLATE.as_bytes().to_vec()),
            Managed::SettingsExample | Managed::DarkExample | Managed::LightExample => {
                created(self.builtin().expect("examples have fixed text"))
            }
            Managed::Marker => Missing::Migration,
            Managed::UserId => {
                let mut id = (input.new_user_id)();
                id.push('\n');
                created(id.into_bytes())
            }
            Managed::WalletList => {
                let recovered = recover_entries(input.keys, &[]);
                match recovered.is_empty() {
                    true => Missing::Leave,
                    false => Missing::Create {
                        bytes: list_with(&[], &recovered),
                        notes: recovered_notes(&recovered),
                    },
                }
            }
            Managed::ActiveWallet | Managed::Grants | Managed::SiteIndex | Managed::Freshness => Missing::Leave,
            Managed::Window => created(WindowState::default().to_bytes()),
            Managed::Session => created(SessionState::default().to_bytes()),
            Managed::History => created(History::default().to_bytes()),
        }
    }
}

/// What this session must not write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocked {
    /// Why nothing may be written, in a read-only session.
    pub all: Option<String>,
    /// Why a file may not be written: it cannot be read, or a newer
    /// F1R3Gaze wrote it.
    pub files: BTreeMap<Managed, String>,
}

impl Blocked {
    pub fn why(&self, m: Managed) -> Option<&str> {
        self.all.as_deref().or_else(|| self.files.get(&m).map(String::as_str))
    }

    /// Why the wallets may not be changed.
    pub fn wallets(&self) -> Option<&str> {
        self.why(Managed::WalletList).or_else(|| self.why(Managed::ActiveWallet))
    }
}

/// What start-up's repair leaves for the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciled {
    pub blocked: Blocked,
    /// The user id in use: the file's, a new one written, or (when it
    /// cannot be written) a new one kept in memory.
    pub user_id: String,
    /// The wallets listed again from their keys.
    pub recovered: Vec<Address>,
}

/// The folders swept of dead writers' temporary files. Never `wallet/keys`
/// (a key's only copy may be under a temporary name: `FileKeystore` syncs
/// its temporary file before renaming it), the cache (gaze-blob's own
/// names, and regenerable), backups, `.migration`, or the old profile.
fn swept_folders(l: &Layout) -> [PathBuf; 11] {
    [
        l.config.clone(),
        l.themes_dir(),
        l.data.clone(),
        l.wallet_dir(),
        l.exports_dir(),
        l.permissions_dir(),
        l.site_data_dir(),
        l.trust_dir(),
        l.replay_logs_dir(),
        l.state.clone(),
        l.runtime.clone(),
    ]
}

// Folders are created by gaze_fs::create_dir_durably, which this module's
// `create_missing` became so the instance lock could use it too.

// `sweep_named` moved to gaze-fs as `sweep_temps_of`, which the migration
// uses too.

/// Start-up's step 5: the folders, then the sweep. An error means the
/// profile cannot be written: the session goes read-only.
pub fn prepare(start: &Start<'_>) -> Result<(), String> {
    let (fs, l, report) = (start.fs, start.layout, start.report);
    let read_only = matches!(start.access, Access::ReadOnly { .. });
    for dir in l.skeleton() {
        match read_only {
            // Was: only a missing folder was reported, so a session that
            // only reads (`f1r3gaze profile check`) did not say that
            // something else stood where a folder must be, which stops a
            // start from writing.
            true => match fs.kind(&dir) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => report.push(Event::new(
                    EventKind::Created,
                    Severity::Quiet,
                    &dir,
                    format!("{} would be created, but this session only reads", dir.display()),
                )),
                Ok(Kind::Dir) => {}
                Ok(Kind::Symlink) if fs.list(&dir).is_ok() => {}
                Ok(_) => report.push(Event::new(
                    EventKind::Unrepaired {
                        error: "in the way of a folder".into(),
                    },
                    Severity::Warning,
                    &dir,
                    format!("{} is in the way of a folder: a start would only read", dir.display()),
                )),
                Err(e) => report.push(Event::new(
                    EventKind::Unreadable { error: e.to_string() },
                    Severity::Warning,
                    &dir,
                    format!("{} cannot be looked at ({e})", dir.display()),
                )),
            },
            false => {
                let created = gaze_fs::create_dir_durably(fs, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                for folder in created {
                    report.push(Event::new(EventKind::Created, Severity::Quiet, &folder, format!("{} was created", folder.display())));
                }
            }
        }
    }
    // Was: `if read_only { return Ok(()); }` here, so a session that only
    // reads (and `f1r3gaze profile check`) never said which temporary files
    // a start would remove. It lists them now, removing nothing.
    let swept = |dir: &Path, removed: io::Result<usize>, place: String| match removed {
        Ok(0) => {}
        Ok(removed) => {
            let files = count(removed, "temporary file", "temporary files");
            report.push(Event::new(
                EventKind::Swept { removed },
                Severity::Quiet,
                dir,
                match read_only {
                    true => format!("{files} of writers that died would be removed {place}, but this session only reads"),
                    false => format!("{files} of writers that died removed {place}"),
                },
            ));
        }
        // A folder a read-only session would have created holds nothing, and
        // something in the way of one was reported above.
        Err(e) if read_only && matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {}
        Err(e) => report.push(Event::new(
            EventKind::Unreadable { error: e.to_string() },
            Severity::Warning,
            dir,
            format!("{} cannot be listed ({e}), so it was not swept", dir.display()),
        )),
    };
    for dir in swept_folders(l) {
        let removed = match read_only {
            true => gaze_fs::stale_temps(fs, &dir).map(|stale| stale.len()),
            false => gaze_fs::sweep_temps(fs, &dir),
        };
        swept(&dir, removed, format!("from {}", dir.display()));
    }
    // write_atomic makes its temporary file next to a linked settings.toml's
    // target.
    let settings = l.settings_file();
    if let Ok(Kind::Symlink) = fs.kind(&settings)
        && let Ok(target) = gaze_fs::resolve_link(fs, &settings)
        && let (Ok(dir), Some(name)) = (gaze_fs::parent(&target), target.file_name())
        && dir != l.config.as_path()
    {
        let removed = match read_only {
            true => gaze_fs::stale_temps_of(fs, dir, name).map(|stale| stale.len()),
            false => gaze_fs::sweep_temps_of(fs, dir, name),
        };
        swept(dir, removed, "next to settings.toml's target".into());
    }
    Ok(())
}

/// What happened to one managed file.
enum Outcome {
    /// It holds these bytes now.
    Holds(Vec<u8>),
    /// It is absent.
    Absent,
    /// It cannot be used this session.
    Blocked(String),
}

/// Start-up's step 6: every managed file, then the wallet keys.
pub fn reconcile(start: &Start<'_>) -> Reconciled {
    let (fs, l, report) = (start.fs, start.layout, start.report);
    let mut blocked = Blocked {
        all: match start.access {
            Access::ReadOnly { why } => Some(why.clone()),
            Access::Write => None,
        },
        files: BTreeMap::new(),
    };
    let keys_dir = l.keys_dir();
    let scan = match start.keystore {
        KeystoreKind::Os => KeyScan::default(),
        KeystoreKind::File => match scan_keys(fs, &keys_dir) {
            Ok(scan) => scan,
            Err(e) => {
                report.push(Event::new(
                    EventKind::Unreadable { error: e.to_string() },
                    Severity::Warning,
                    &keys_dir,
                    format!("wallet/keys cannot be read ({e}); no wallet is listed again from its key"),
                ));
                KeyScan::default()
            }
        },
    };
    let input = CheckInput {
        keys: &scan,
        new_user_id: start.new_user_id,
    };
    let mut outcomes: BTreeMap<Managed, Outcome> = BTreeMap::new();
    let mut recovered = Vec::new();
    for m in Managed::ALL {
        let outcome = reconcile_file(start, &input, m, &mut recovered);
        if let Outcome::Blocked(why) = &outcome {
            blocked.files.insert(m, why.clone());
        }
        outcomes.insert(m, outcome);
    }
    let user_id = match outcomes.get(&Managed::UserId) {
        Some(Outcome::Holds(bytes)) => String::from_utf8_lossy(bytes).trim().to_string(),
        _ => (start.new_user_id)(),
    };
    if start.keystore == KeystoreKind::File {
        let listed = match outcomes.get(&Managed::WalletList) {
            Some(Outcome::Holds(bytes)) => parse_list(bytes),
            _ => Vec::new(),
        };
        report_keys(start, &scan, &listed);
    }
    Reconciled {
        blocked,
        user_id,
        recovered,
    }
}

/// What the key files say, reported: they are never written.
fn report_keys(start: &Start<'_>, scan: &KeyScan, listed: &[gaze_wallet::Entry]) {
    let report = start.report;
    let owner = |path: &Path| {
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        wallet_of_key_file(&file, listed)
    };
    let whose = |entry: Option<&gaze_wallet::Entry>| match entry {
        Some(e) if e.label.is_empty() => format!("it holds the key of the wallet {}", e.address),
        Some(e) => format!("it holds the key of the wallet \"{}\" ({})", e.label, e.address),
        None => "no listed wallet uses it".to_string(),
    };
    for (path, what) in &scan.keys {
        match what {
            KeyFile::Corrupt(why) => {
                let entry = owner(path);
                report.push(Event::new(
                    EventKind::CorruptKey {
                        wallet: entry.map(|e| e.address.to_string()),
                    },
                    Severity::Alert,
                    path,
                    format!("{} is damaged ({why}); {}. It is left as it is", path.display(), whose(entry)),
                ));
            }
            KeyFile::Unreadable(error) => report.push(Event::new(
                EventKind::Unreadable { error: error.clone() },
                Severity::Alert,
                path,
                format!("{} cannot be read ({error}); {}. It is left as it is", path.display(), whose(owner(path))),
            )),
            KeyFile::Wallet(_) | KeyFile::Other => {}
        }
    }
    for entry in listed {
        let file = gaze_shard::keys::key_file_name(&gaze_wallet::wallet_key_name(&entry.address));
        if !scan.keys.iter().any(|(p, _)| p.file_name().is_some_and(|n| n.to_string_lossy() == file)) {
            report.push(Event::new(
                EventKind::MissingKey { wallet: entry.address.to_string() },
                Severity::Warning,
                start.layout.keys_dir().join(&file),
                format!("the wallet {} has no key file, so it cannot pay", entry.address),
            ));
        }
    }
    for (path, address) in &scan.stranded {
        report.push(Event::new(
            EventKind::StrayKey { wallet: address.to_string() },
            Severity::Warning,
            path,
            format!(
                "{} holds the only copy of the key of the wallet {address}: a save that did not finish. It is left as it is; to use it, run f1r3gaze wallet import {}",
                path.display(),
                path.display()
            ),
        ));
    }
}

/// ⟨reconcile a file⟩ (module documentation; the model's ReconcileBegin and
/// ReconcileStep).
fn reconcile_file(start: &Start<'_>, input: &CheckInput<'_>, m: Managed, recovered: &mut Vec<Address>) -> Outcome {
    let (fs, l, report) = (start.fs, start.layout, start.report);
    let path = m.path(l);
    let read_only = matches!(start.access, Access::ReadOnly { .. });
    let blocked = |severity: Severity, kind: EventKind, message: String| {
        report.push(Event::new(kind, severity, &path, message.clone()));
        Outcome::Blocked(message)
    };
    // A second look happens only after something appeared at a missing name.
    for _ in 0..2 {
        let bytes = match fs.read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Ok(Kind::Symlink) = fs.kind(&path) {
                    let target = fs.read_link(&path).map(|t| t.display().to_string()).unwrap_or_else(|_| "nothing".into());
                    return blocked(
                        m.unreadable(),
                        EventKind::Unreadable { error: e.to_string() },
                        format!("{} is a link to {target}, which does not exist; it is left as it is", m.name()),
                    );
                }
                match m.missing(input) {
                    Missing::Leave | Missing::Migration => return Outcome::Absent,
                    Missing::Create { bytes, notes } => {
                        if read_only {
                            for n in notes {
                                report.push(Event::new(n.kind, Severity::Quiet, &path, format!("{} would be created, but this session only reads", m.name())));
                            }
                            return Outcome::Absent;
                        }
                        match gaze_fs::write_new(fs, &path, &bytes, m.perm()) {
                            Ok(()) => {
                                for n in notes {
                                    if let EventKind::Recovered { address } = &n.kind
                                        && let Ok(a) = Address::parse(address)
                                    {
                                        recovered.push(a);
                                    }
                                    report.push(Event::new(n.kind, n.severity, &path, n.message));
                                }
                                return Outcome::Holds(bytes);
                            }
                            // Something appeared since the read: look again.
                            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                            Err(e) => {
                                return blocked(
                                    Severity::Warning,
                                    EventKind::Unrepaired { error: e.to_string() },
                                    format!("{} is missing and cannot be created ({e})", m.name()),
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                return blocked(
                    m.unreadable(),
                    EventKind::Unreadable { error: e.to_string() },
                    format!("{} cannot be read ({e}); it is left as it is", m.name()),
                );
            }
        };
        return match m.check(&path, &bytes, input) {
            Check::Valid => Outcome::Holds(bytes),
            Check::Newer { version, known } => blocked(
                Severity::Notice,
                EventKind::Newer { version, known },
                format!(
                    "{} was written by a newer F1R3Gaze (format {version}; this one knows {known}); it is left as it is, and nothing is saved to it this session",
                    m.name()
                ),
            ),
            Check::Damaged { why, fix } => repair(start, m, &path, &bytes, &why, fix, recovered, read_only),
        };
    }
    blocked(
        Severity::Warning,
        EventKind::Unrepaired {
            error: "it kept changing".into(),
        },
        format!("{} kept changing while start-up looked at it; it is left as it is", m.name()),
    )
}

/// ⟨repair a damaged file⟩: a durable copy first, then the fix.
#[allow(clippy::too_many_arguments)]
fn repair(
    start: &Start<'_>,
    m: Managed,
    path: &Path,
    bytes: &[u8],
    why: &str,
    fix: Fix,
    recovered: &mut Vec<Address>,
    read_only: bool,
) -> Outcome {
    let (fs, report) = (start.fs, start.report);
    let notes = match &fix {
        Fix::Replace { notes, .. } | Fix::Remove { notes } => notes.clone(),
    };
    if read_only {
        for n in notes {
            report.push(Event::new(
                n.kind,
                n.severity,
                path,
                format!("{} is damaged ({why}); it would be backed up and repaired, but this session only reads", m.name()),
            ));
        }
        return Outcome::Blocked(format!("{} is damaged and this session only reads", m.name()));
    }
    let backup = match start.backups.copy_into(path, bytes) {
        Ok(backup) => backup,
        Err(e) => {
            let message = format!("{} is damaged ({why}) but cannot be backed up ({e}); it is left as it is", m.name());
            report.push(Event::new(EventKind::Unrepaired { error: e.to_string() }, Severity::Warning, path, message.clone()));
            return Outcome::Blocked(message);
        }
    };
    let fixed = match fix {
        Fix::Replace { bytes: new, .. } => gaze_fs::write_atomic(fs, path, &new, m.perm()).map(|()| Outcome::Holds(new)),
        Fix::Remove { .. } => fs
            .remove_file(path)
            .and_then(|()| fs.sync_dir(gaze_fs::parent(path)?))
            .map(|()| Outcome::Absent),
    };
    match fixed {
        Ok(outcome) => {
            for (i, n) in notes.into_iter().enumerate() {
                if let EventKind::Recovered { address } = &n.kind
                    && let Ok(a) = Address::parse(address)
                {
                    recovered.push(a);
                }
                let message = match i {
                    0 => format!("{}; the original is kept in {}", n.message, backup.display()),
                    _ => n.message,
                };
                report.push(Event::new(n.kind, n.severity, path, message).with_backup(&backup));
            }
            outcome
        }
        Err(e) => {
            let message = format!(
                "{} is damaged ({why}); it was kept in {}, but could not be repaired ({e})",
                m.name(),
                backup.display()
            );
            report.push(Event::new(EventKind::Unrepaired { error: e.to_string() }, Severity::Warning, path, message.clone()).with_backup(&backup));
            Outcome::Blocked(message)
        }
    }
}

#[cfg(test)]
mod tests;
