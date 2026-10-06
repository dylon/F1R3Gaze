//! The profile: the five roots F1R3Gaze keeps its files in (config, data,
//! state, cache and runtime; docs/storage/README.md §3), and [`Profile`],
//! which opens them the way start-up does (§9): the instance lock, the busy
//! flag, the migration of an old profile, the folders, the repair of every
//! managed file, and the settings.

pub mod backup;
pub mod layout;
pub mod lock;
pub mod migrate;
pub mod reconcile;
pub mod report;
pub mod settings;

use std::path::{Path, PathBuf};

// Disabled at the switch-over (ledger S14): the single profile folder is
// gone. Its rule lives on in the layout's legacy candidates
// (`layout::platform_layout`, `Layout::legacy`), which find an old profile
// to move, and in the oracle `old_default_dir` of layout/tests.rs.
// pub fn default_dir() -> PathBuf {
//     if let Ok(p) = std::env::var("F1R3GAZE_PROFILE") {
//         return PathBuf::from(p);
//     }
//     let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
//     if cfg!(target_os = "macos") {
//         home.join("Library/Application Support/F1R3Gaze")
//     } else if cfg!(windows) {
//         std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home).join("F1R3Gaze")
//     } else {
//         std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("f1r3gaze")
//     }
// }

// Was, until the switch-over, the old settings under their old names:
//     pub use migrate::convert::{LEGACY_TEMPLATE as TEMPLATE, LegacySettings as Settings};
// The old settings (`settings.conf`) live on in `migrate/convert.rs`, where
// the conversion into `settings.toml` uses them.
pub use settings::Settings;

// Disabled at the switch-over (ledger S14): it wrote the template over
// `settings.conf` after any error reading it, an unreadable or non-UTF-8
// file included (ledger S1, H2). `Profile::open` loads `settings.toml`, which
// start-up has checked, backed up and repaired.
// impl Settings {
//     pub fn load(dir: &Path) -> Settings {
//         let p = dir.join("settings.conf");
//         match std::fs::read_to_string(&p) {
//             Ok(t) => Settings::parse(&t),
//             Err(_) => {
//                 let _ = gaze_fs::create_dir_durably(&StdFs, dir).and_then(|_| {
//                     gaze_fs::write_atomic(&StdFs, &p, TEMPLATE.as_bytes(), Perm::Private)
//                 });
//                 Settings::default()
//             }
//         }
//     }
// }

// Disabled at the switch-over (ledger S14): it made and wrote a new id after
// any error reading `user-id`, so an unreadable file lost the id (ledger S1,
// H3 and H3u). Start-up's reconcile owns the id now: `Profile::user_id`.
// /// An installation-local random user id, so site keys are per profile.
// pub fn user_id(dir: &Path) -> String {
//     let p = dir.join("user-id");
//     if let Ok(s) = std::fs::read_to_string(&p) {
//         return s.trim().to_string();
//     }
//     let mut b = [0u8; 16];
//     let _ = getrandom::getrandom(&mut b);
//     let id = gaze_net::hex(&b);
//     let _ = gaze_fs::create_dir_durably(&StdFs, dir)
//         .and_then(|_| gaze_fs::write_atomic(&StdFs, &p, id.as_bytes(), Perm::Private));
//     id
// }

/// Whether this session may write the profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Write,
    /// Only reading: another F1R3Gaze holds the profile, the file system is
    /// read-only, a migration could not finish, or a newer F1R3Gaze set the
    /// profile up. The reason is shown to the user.
    ReadOnly { why: String },
}

/// Where wallet keys are kept: files in the data folder, or the operating
/// system's credential store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeystoreKind {
    File,
    Os,
}

/// The keystore this build uses: the credential store on macOS and Windows
/// when built with `os-keyring`, files everywhere else (the choice
/// `Engine::open` makes).
pub fn keystore_kind() -> KeystoreKind {
    match cfg!(feature = "os-keyring") && cfg!(any(target_os = "macos", windows)) {
        true => KeystoreKind::Os,
        false => KeystoreKind::File,
    }
}

/// A new random user id: 16 bytes from the operating system, in hex.
pub fn new_user_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("the operating system provides entropy");
    gaze_net::hex(&bytes)
}

pub fn seed() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("OS entropy");
    b
}

// The test of the old settings parser (settings_parse) moved with it to
// migrate/convert/tests.rs (old_settings_parse).

// ── Opening a profile ───────────────────────────────────────────────────

use backup::{Backups, StoreSalvage};
use layout::Layout;
use lock::{InstanceLock, LockError, Mode};
use migrate::{Detected, Migration, Plan};
use reconcile::{Blocked, Managed, Reconciled, Start};
use report::{Event, EventKind, Report, Severity};
use crate::theme::{ThemeChoice, ThemeDirs};
use crate::ui_state::{History, SessionState, StateError};
use crate::window_state::WindowState;
use gaze_fs::Fs;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::SystemTime;

/// A state file the session reads when it starts and writes as it changes:
/// `session.json`, `history.json` and `window.json`, in the state root.
pub trait StateFile: Default + Sized {
    /// Which managed file it is: where it lives, and what start-up did to it.
    const MANAGED: Managed;
    fn read(bytes: &[u8]) -> Result<Self, StateError>;
    fn to_bytes(&self) -> Vec<u8>;
}

impl StateFile for SessionState {
    const MANAGED: Managed = Managed::Session;
    fn read(bytes: &[u8]) -> Result<Self, StateError> {
        SessionState::read(bytes)
    }
    fn to_bytes(&self) -> Vec<u8> {
        SessionState::to_bytes(self)
    }
}

impl StateFile for History {
    const MANAGED: Managed = Managed::History;
    fn read(bytes: &[u8]) -> Result<Self, StateError> {
        History::read(bytes)
    }
    fn to_bytes(&self) -> Vec<u8> {
        History::to_bytes(self)
    }
}

impl StateFile for WindowState {
    const MANAGED: Managed = Managed::Window;
    fn read(bytes: &[u8]) -> Result<Self, StateError> {
        WindowState::read(bytes)
    }
    fn to_bytes(&self) -> Vec<u8> {
        WindowState::to_bytes(self)
    }
}

/// How a command takes the instance lock (README §8; `main.rs` chooses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locking {
    /// The window, `--headless`, and commands that change the profile: when
    /// another F1R3Gaze holds it, the profile is not opened.
    Exclusive(Mode),
    /// Commands that only read, such as `wallet list`: when another F1R3Gaze
    /// holds the profile, they go on read-only.
    IfFree(Mode),
    /// No lock and nothing written: `f1r3gaze profile check`. The reason is
    /// the session's.
    ReadOnly(&'static str),
    /// No lock, and writing: tests, each on a profile of its own (only
    /// `main.rs` takes the lock).
    #[cfg(test)]
    Unlocked,
}

/// What opening a profile takes from the machine: tests give their own.
pub struct StartEnv {
    pub fs: Arc<dyn Fs + Send + Sync>,
    /// Where start-up reports what it found and did.
    pub report: Report,
    pub now: SystemTime,
    pub new_user_id: fn() -> String,
    pub keystore: KeystoreKind,
}

impl StartEnv {
    /// The real machine's: the report echoes to stderr, the clock and the
    /// keystore are this build's.
    pub fn real(fs: Arc<dyn Fs + Send + Sync>) -> StartEnv {
        StartEnv {
            fs,
            report: Report::new(),
            now: SystemTime::now(),
            new_user_id,
            keystore: keystore_kind(),
        }
    }
}

/// What start-up's steps 3 to 7 work with.
pub struct StartUp<'a> {
    pub layout: &'a Layout,
    pub fs: Arc<dyn Fs + Send + Sync>,
    pub report: &'a Report,
    /// Whether the session may write, as the lock left it.
    pub access: Access,
    pub keystore: KeystoreKind,
    pub now: SystemTime,
    pub new_user_id: &'a dyn Fn() -> String,
}

/// What start-up's steps 3 to 7 leave for the session.
#[derive(Debug)]
pub struct Started {
    /// Whether the session may write now: a profile set up by a newer
    /// F1R3Gaze, a migration that stopped, or a profile whose folders
    /// cannot be made leaves it read-only.
    pub access: Access,
    pub migration: Migration,
    pub reconciled: Reconciled,
    /// The start's backup folders, which a damaged site store opened later
    /// in the session backs up into too.
    pub backups: Backups,
}

/// Start-up's steps 3 to 7 (README §9.1), the one copy of their order:
/// the busy flag (and the barrier after a start that did not finish), the
/// migration, the folders and the sweep, the repair, and the flag removed.
/// The lock (step 2) is the caller's, and the settings (step 8) are read
/// after.
pub fn start_up(s: StartUp<'_>) -> Started {
    let (l, fs, report) = (s.layout, s.fs.as_ref(), s.report);
    let backups = Backups::new(Arc::new(l.clone()), s.fs.clone(), s.now);
    let mut access = s.access;
    let read_only = |what: &Path, why: String, report: &Report| {
        report.push(Event::new(
            EventKind::ReadOnly { why: why.clone() },
            Severity::Warning,
            what,
            format!("{why}; F1R3Gaze opened read only"),
        ));
        Access::ReadOnly { why }
    };
    // A profile a newer F1R3Gaze set up gets no busy flag: nothing is
    // written to it (§9.7). Step 4 reports it.
    if access == Access::Write
        && let Some(version) = migrate::newer_layout(l, fs)
    {
        access = Access::ReadOnly {
            why: format!(
                "a newer F1R3Gaze set this profile up (layout {version}; this one knows {})",
                migrate::LAYOUT_VERSION
            ),
        };
    }
    // Step 3: the busy flag. Only a start that set it removes it (step 7).
    let mut flagged = false;
    if access == Access::Write {
        match migrate::begin(l, fs) {
            Ok(_) => flagged = true,
            Err(e) => access = read_only(&l.busy_file(), format!("{} cannot be written ({e})", l.busy_file().display()), report),
        }
    }
    // Step 4: the migration.
    let migration = {
        let start = Start {
            layout: l,
            fs,
            backups: &backups,
            report,
            access: &access,
            keystore: s.keystore,
            new_user_id: s.new_user_id,
        };
        migrate::run(&start, s.now)
    };
    // A move that stopped is resumed by the next start: this one only reads,
    // and leaves its flag, so the next start syncs what this one left.
    // `run` reported why.
    if access == Access::Write {
        let stopped = match &migration {
            Migration::Pending { legacy } => Some(format!("the old profile at {} waits to be moved", legacy.display())),
            Migration::Newer { layout } => Some(format!("a newer F1R3Gaze set this profile up (layout {layout})")),
            Migration::Failed { error } => Some(format!("the move of the old profile stopped: {error}")),
            Migration::Current | Migration::Fresh | Migration::Done(_) => None,
        };
        if let Some(why) = stopped {
            access = Access::ReadOnly { why };
            flagged = false;
        }
    }
    // Step 5: the folders and the sweep.
    let prepared = reconcile::prepare(&Start {
        layout: l,
        fs,
        backups: &backups,
        report,
        access: &access,
        keystore: s.keystore,
        new_user_id: s.new_user_id,
    });
    if let Err(e) = prepared {
        access = read_only(&l.data, format!("the profile's folders cannot be made ({e})"), report);
        flagged = false;
    }
    let start = Start {
        layout: l,
        fs,
        backups: &backups,
        report,
        access: &access,
        keystore: s.keystore,
        new_user_id: s.new_user_id,
    };
    // Step 6: every managed file.
    let reconciled = reconcile::reconcile(&start);
    // Step 7: the flag goes; the next start needs no barrier.
    if flagged && let Err(e) = migrate::finish(l, fs) {
        report.push(Event::new(
            EventKind::Other,
            Severity::Warning,
            l.busy_file(),
            format!("{} could not be removed ({e}); the next start checks everything again", l.busy_file().display()),
        ));
    }
    Started {
        access,
        migration,
        reconciled,
        backups,
    }
}

/// Why a profile was not opened.
#[derive(Debug)]
pub enum OpenError {
    /// Another F1R3Gaze holds the profile, and this command writes it.
    Held(LockError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Held(e) => write!(f, "{e}; close it, or use --profile DIR for another profile"),
        }
    }
}

impl std::error::Error for OpenError {}

/// An open profile: its roots, its settings, what start-up found, and what
/// this session may write.
pub struct Profile {
    pub layout: Layout,
    /// As loaded at start; the theme lives on in [`Profile::theme`].
    pub settings: Settings,
    /// What start-up found and did; the window shows what it has not shown.
    pub report: Report,
    pub access: Access,
    /// The files this session must not write.
    pub blocked: Blocked,
    fs: Arc<dyn Fs + Send + Sync>,
    user_id: String,
    migration: Migration,
    backups: Backups,
    theme: RefCell<ThemeChoice>,
    keystore: KeystoreKind,
    /// Held while the profile is open; dropping the profile frees it.
    _lock: Option<InstanceLock>,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("layout", &self.layout)
            .field("access", &self.access)
            .field("migration", &self.migration)
            .finish_non_exhaustive()
    }
}

impl Profile {
    /// Opens the profile at `layout` as start-up does (README §9.1): the
    /// lock, steps 3 to 7, then the settings.
    pub fn open(layout: Layout, locking: Locking, env: StartEnv) -> Result<Profile, OpenError> {
        let report = env.report;
        let (lock, access) = match locking {
            Locking::Exclusive(mode) | Locking::IfFree(mode) => {
                match InstanceLock::acquire_in(env.fs.as_ref(), &layout, mode, env.now, &report) {
                    Ok(lock) => (Some(lock), Access::Write),
                    Err(e @ LockError::Held { .. }) if matches!(locking, Locking::Exclusive(_)) => {
                        return Err(OpenError::Held(e));
                    }
                    Err(e) => {
                        let (severity, path) = match &e {
                            LockError::Held { lock, .. } => (Severity::Notice, lock.clone()),
                            LockError::ReadOnly { lock, .. } => (Severity::Warning, lock.clone()),
                            LockError::Unavailable { .. } => (Severity::Warning, layout.anchor_lock_file()),
                        };
                        let why = e.to_string();
                        report.push(Event::new(
                            EventKind::ReadOnly { why: why.clone() },
                            severity,
                            path,
                            format!("{why}; this session only reads"),
                        ));
                        (None, Access::ReadOnly { why })
                    }
                }
            }
            Locking::ReadOnly(why) => (None, Access::ReadOnly { why: why.to_string() }),
            #[cfg(test)]
            Locking::Unlocked => (None, Access::Write),
        };
        let new_id = env.new_user_id;
        let started = start_up(StartUp {
            layout: &layout,
            fs: env.fs.clone(),
            report: &report,
            access,
            keystore: env.keystore,
            now: env.now,
            new_user_id: &new_id,
        });
        let loaded = load_settings(&layout, env.fs.as_ref());
        for diagnostic in &loaded.diagnostics {
            report.push(Event::new(
                EventKind::Setting,
                Severity::Warning,
                &diagnostic.file,
                diagnostic.to_string(),
            ));
        }
        Ok(Profile {
            theme: RefCell::new(loaded.settings.theme.clone()),
            settings: loaded.settings,
            report,
            access: started.access,
            blocked: started.reconciled.blocked,
            fs: env.fs,
            user_id: started.reconciled.user_id,
            migration: started.migration,
            backups: started.backups,
            keystore: env.keystore,
            layout,
            _lock: lock,
        })
    }

    /// The file system the profile is on.
    pub fn fs(&self) -> &dyn Fs {
        self.fs.as_ref()
    }

    /// Whether this session may write the profile.
    pub fn may_write(&self) -> bool {
        self.access == Access::Write
    }

    /// Why this session only reads, if it does.
    pub fn read_only_reason(&self) -> Option<&str> {
        match &self.access {
            Access::ReadOnly { why } => Some(why),
            Access::Write => None,
        }
    }

    /// What start-up's step 4 did.
    pub fn migration(&self) -> &Migration {
        &self.migration
    }

    /// Whether an old profile still waits to be moved: a session that only
    /// read it, or a move that stopped.
    pub fn migration_pending(&self) -> bool {
        matches!(self.migration, Migration::Pending { .. } | Migration::Failed { .. })
    }

    /// The user id site keys derive from.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn keystore(&self) -> KeystoreKind {
        self.keystore
    }

    /// The theme chosen now: the setting, or what this session chose since.
    pub fn theme(&self) -> ThemeChoice {
        self.theme.borrow().clone()
    }

    /// Chooses a theme: for this session first, then in `settings.toml`
    /// (comments kept). An error says why it was not saved; the session
    /// keeps the choice either way.
    pub fn set_theme(&self, choice: ThemeChoice) -> Result<(), String> {
        self.theme.replace(choice.clone());
        if let Some(why) = self.blocked.why(Managed::Settings) {
            return Err(why.to_string());
        }
        settings::set_theme(self.fs(), &self.layout.settings_file(), &choice).map_err(|e| e.to_string())
    }

    /// Where theme files are found: the user's folder, then the system's.
    pub fn theme_dirs(&self) -> ThemeDirs {
        ThemeDirs::of(&self.layout)
    }

    /// A state file as start-up left it: its default when it is missing,
    /// could not be read, or was written by a newer F1R3Gaze (start-up has
    /// repaired a damaged one).
    pub fn read_state<T: StateFile>(&self) -> T {
        match self.fs().read(&T::MANAGED.path(&self.layout)) {
            Ok(bytes) => T::read(&bytes).unwrap_or_default(),
            Err(_) => T::default(),
        }
    }

    /// Saves a state file atomically, owner-only. Refused when this session
    /// must not write it: read-only, or a file start-up could not read or
    /// that a newer F1R3Gaze wrote.
    pub fn write_state<T: StateFile>(&self, state: &T) -> Result<(), String> {
        let m = T::MANAGED;
        if let Some(why) = self.blocked.why(m) {
            return Err(format!("{} is not saved: {why}", m.name()));
        }
        let path = m.path(&self.layout);
        let dir = gaze_fs::parent(&path).map_err(|e| e.to_string())?;
        gaze_fs::create_dir_durably(self.fs(), dir)
            .and_then(|_| gaze_fs::write_atomic(self.fs(), &path, &state.to_bytes(), m.perm()))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// What keeps a damaged site store before it is cut back: a copy in this
    /// start's data backups, and a report.
    pub fn store_salvage(&self, site: &str) -> StoreSalvage {
        StoreSalvage::new(self.backups.clone(), self.report.clone(), site)
    }

    /// Removes every backup of the browsing history ("Clear history").
    /// Refused in a session that only reads.
    pub fn forget_history_backups(&self) -> Result<usize, String> {
        if let Some(why) = self.read_only_reason() {
            return Err(format!("the history's backups are kept: {why}"));
        }
        backup::forget_history_backups(&self.layout, self.fs()).map_err(|e| e.to_string())
    }
}

/// The settings: the system-wide files, then the user's.
fn load_settings(l: &Layout, fs: &dyn Fs) -> settings::Loaded {
    let system: Vec<PathBuf> = l.system_config.iter().map(|dir| dir.join("settings.toml")).collect();
    settings::load(fs, &system, &l.settings_file())
}

/// What `f1r3gaze profile check` found: what a start would do, done by a
/// start that only reads.
#[derive(Debug)]
pub struct Checked {
    pub detected: Result<Detected, String>,
    /// The plan of the old profile a start would move.
    pub plan: Option<Result<Plan, String>>,
    pub started: Started,
    pub settings: settings::Loaded,
    pub events: Vec<Event>,
}

impl Checked {
    /// What a start would change, one line each: empty when a start would
    /// write nothing but its busy flag and its lock files.
    pub fn changes(&self) -> Vec<String> {
        let mut changes = Vec::new();
        match &self.detected {
            Ok(Detected::Current { cleanup: true }) => {
                changes.push("the folder of a finished migration would be removed".to_string());
            }
            Ok(Detected::Fresh) => changes.push("the layout's marker would be written".to_string()),
            Ok(Detected::Legacy { .. } | Detected::Resume | Detected::Current { cleanup: false } | Detected::Newer { .. })
            | Err(_) => {}
        }
        for event in &self.events {
            match event.kind {
                EventKind::Created
                | EventKind::Refreshed
                | EventKind::Repaired { .. }
                | EventKind::Regenerated { .. }
                | EventKind::Recovered { .. }
                | EventKind::Swept { .. }
                | EventKind::MigrationPending { .. } => changes.push(event.message.clone()),
                _ => {}
            }
        }
        changes
    }
}

/// `f1r3gaze profile check`: start-up's steps 3 to 8, reading only, with
/// no lock. The report is silent; its events are returned.
pub fn check(layout: &Layout, fs: Arc<dyn Fs + Send + Sync>, now: SystemTime) -> Checked {
    let report = Report::silent();
    let detected = migrate::detect(layout, fs.as_ref(), &report);
    let plan = match &detected {
        Ok(Detected::Legacy { root }) => Some(migrate::plan(layout, fs.as_ref(), root, now)),
        _ => None,
    };
    let new_id = || "(a new id would be made)".to_string();
    let started = start_up(StartUp {
        layout,
        fs: fs.clone(),
        report: &report,
        access: Access::ReadOnly {
            why: "profile check only looks".into(),
        },
        keystore: keystore_kind(),
        now,
        new_user_id: &new_id,
    });
    let settings = load_settings(layout, fs.as_ref());
    Checked {
        detected,
        plan,
        started,
        settings,
        events: report.events(),
    }
}

#[cfg(test)]
mod tests;
