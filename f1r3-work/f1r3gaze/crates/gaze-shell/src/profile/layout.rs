//! Where F1R3Gaze keeps its files: five roots, one per kind of file, each in
//! the place the operating system sets aside for that kind
//! (docs/storage/README.md, sections 2 and 3).
//!
//! | Class | Holds | Linux (XDG Base Directory 0.8) | macOS | Windows |
//! |---|---|---|---|---|
//! | config | settings, themes | `$XDG_CONFIG_HOME/f1r3fly-io/f1r3gaze` | `~/Library/Application Support/io.f1r3fly.f1r3gaze/config` | `%APPDATA%\f1r3fly-io\f1r3gaze` |
//! | data | wallets, keys, permissions, site data, trust records, replay logs | `$XDG_DATA_HOME/…` | `…/io.f1r3fly.f1r3gaze/data` | `%LOCALAPPDATA%\f1r3fly-io\f1r3gaze\data` |
//! | state | window, session, history | `$XDG_STATE_HOME/…` | `…/io.f1r3fly.f1r3gaze/state` | `…\state` |
//! | cache | fetched content | `$XDG_CACHE_HOME/…` | `~/Library/Caches/io.f1r3fly.f1r3gaze` | `…\cache` |
//! | runtime | the instance lock | `$XDG_RUNTIME_DIR/…` | `$TMPDIR/io.f1r3fly.f1r3gaze` | `…\runtime` |
//!
//! Where one OS folder holds several classes (Application Support on macOS,
//! the local application data on Windows), each class has a subfolder named
//! after it. `--profile DIR` (or `F1R3GAZE_PROFILE`) puts all five under DIR
//! instead, and ignores the system-wide layers.
//!
//! [`platform_layout`] is a pure function of the platform and of what
//! [`Machine::detect`] read, so the tests check all three platforms on any
//! one of them. For that, what counts as an absolute path is the target
//! platform's rule, not the host's: `/…` on Linux and macOS, `X:\…` or
//! `\\server\…` on Windows.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The vendor folder on Linux and Windows.
pub const VENDOR: &str = "f1r3fly-io";
/// The application folder on Linux and Windows.
pub const APP: &str = "f1r3gaze";
/// The application folder on macOS: the bundle identifier
/// (`packaging/macos/Info.plist`).
pub const BUNDLE_ID: &str = "io.f1r3fly.f1r3gaze";
/// The environment variable naming a portable profile root.
pub const PROFILE_VAR: &str = "F1R3GAZE_PROFILE";

/// The five kinds of file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Config,
    Data,
    State,
    Cache,
    Runtime,
}

impl Class {
    pub const ALL: [Class; 5] = [Class::Config, Class::Data, Class::State, Class::Cache, Class::Runtime];

    pub fn name(self) -> &'static str {
        match self {
            Class::Config => "config",
            Class::Data => "data",
            Class::State => "state",
            Class::Cache => "cache",
            Class::Runtime => "runtime",
        }
    }
}

/// Whose conventions decide the roots. `Xdg` covers Linux and the BSDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Xdg,
    MacOs,
    Windows,
}

impl Platform {
    pub fn current() -> Platform {
        match (cfg!(target_os = "macos"), cfg!(windows)) {
            (true, _) => Platform::MacOs,
            (_, true) => Platform::Windows,
            _ => Platform::Xdg,
        }
    }
}

/// The environment variables the mapping reads.
const ENV_READ: [&str; 11] = [
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "TMPDIR",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
];

/// What the mapping reads from the machine: gathered by [`Machine::detect`],
/// written out by tests.
#[derive(Clone, Debug, Default)]
pub struct Machine {
    /// The user's home directory.
    pub home: Option<PathBuf>,
    /// The variables of `ENV_READ` that are set, empty ones included.
    pub env: BTreeMap<String, OsString>,
    /// `FOLDERID_RoamingAppData` on Windows.
    pub roaming_app_data: Option<PathBuf>,
    /// `FOLDERID_LocalAppData` on Windows.
    pub local_app_data: Option<PathBuf>,
}

impl Machine {
    /// The running machine: its home directory, the variables the mapping
    /// reads, and on Windows the Known Folders. On macOS Foundation returns
    /// the app container home when App Sandbox is enabled.
    pub fn detect() -> Machine {
        let windows = Platform::current() == Platform::Windows;
        Machine {
            home: detected_home_dir(),
            env: ENV_READ
                .iter()
                .filter_map(|name| std::env::var_os(name).map(|value| (name.to_string(), value)))
                .collect(),
            roaming_app_data: windows.then(dirs::config_dir).flatten(),
            local_app_data: windows.then(dirs::data_local_dir).flatten(),
        }
    }

    /// A variable's value as a path absolute on `platform`; `None` if it is
    /// unset, empty or relative. The XDG specification: "If an
    /// implementation encounters a relative path in any of these variables
    /// it should consider the path invalid and ignore it."
    fn absolute(&self, name: &str, platform: Platform) -> Option<PathBuf> {
        self.env
            .get(name)
            .filter(|value| is_absolute_on(platform, value))
            .map(PathBuf::from)
    }

    /// An XDG list (`$XDG_CONFIG_DIRS`, `$XDG_DATA_DIRS`): colon-separated,
    /// most important first, with empty and relative entries left out;
    /// `default` if none remains.
    fn path_list(&self, name: &str, default: &[&str]) -> Vec<PathBuf> {
        let listed: Vec<PathBuf> = self
            .env
            .get(name)
            .map(|value| {
                split_colons(value)
                    .into_iter()
                    .filter(|entry| is_absolute_on(Platform::Xdg, entry))
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        match listed.is_empty() {
            true => default.iter().map(PathBuf::from).collect(),
            false => listed,
        }
    }

    fn home(&self) -> Result<&Path, LayoutError> {
        self.home.as_deref().ok_or(LayoutError::NoHome)
    }
}

#[cfg(target_os = "macos")]
fn detected_home_dir() -> Option<PathBuf> {
    // Preserve HOME for the Developer ID build and isolated command-line
    // profiles. A sandboxed app gets its own .../Library/Containers/ID/Data
    // home from Foundation, even if HOME still names the user's normal home.
    let foundation = PathBuf::from(objc2_foundation::NSHomeDirectory().to_string());
    if is_macos_container_home(&foundation) {
        Some(foundation)
    } else {
        dirs::home_dir().or(Some(foundation))
    }
}

#[cfg(target_os = "macos")]
pub fn is_macos_container_home(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "Data")
        && path.parent().and_then(Path::parent).and_then(Path::file_name).is_some_and(|name| name == "Containers")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == "Library")
}

#[cfg(not(target_os = "macos"))]
fn detected_home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Why the roots could not be found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// No home directory, and a root needs one.
    NoHome,
    /// Windows: neither the Known Folder nor `%APPDATA%`/`%LOCALAPPDATA%`.
    NoAppData,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::NoHome => write!(
                f,
                "cannot find your home directory to keep F1R3Gaze's files in; use --profile DIR"
            ),
            LayoutError::NoAppData => write!(
                f,
                "cannot find the application data folders (%APPDATA%, %LOCALAPPDATA%); use --profile DIR"
            ),
        }
    }
}

/// How the roots were chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The platform's places.
    Platform(Platform),
    /// Everything under one directory (`--profile DIR`).
    Portable(PathBuf),
}

/// The five roots, and the read-only system-wide places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub kind: Kind,
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub runtime: PathBuf,
    /// Whether the runtime root is the stand-in under state, because the
    /// platform's (`$XDG_RUNTIME_DIR`, `$TMPDIR`) is not available.
    pub runtime_is_fallback: bool,
    /// Directories holding system-wide `settings.toml` files, most important
    /// first. Never written.
    pub system_config: Vec<PathBuf>,
    /// Directories of packaged themes, most important first. Never written.
    pub system_themes: Vec<PathBuf>,
    /// Where an old single-folder profile may be, most likely first.
    pub legacy: Vec<PathBuf>,
}

/// The roots for `platform`, as conventional there, from what `machine`
/// holds. Pure.
pub fn platform_layout(platform: Platform, machine: &Machine) -> Result<Layout, LayoutError> {
    match platform {
        Platform::Xdg => xdg_layout(machine),
        Platform::MacOs => macos_layout(machine),
        Platform::Windows => windows_layout(machine),
    }
}

fn xdg_layout(machine: &Machine) -> Result<Layout, LayoutError> {
    let base = |var: &str, under_home: &str| -> Result<PathBuf, LayoutError> {
        match machine.absolute(var, Platform::Xdg) {
            Some(path) => Ok(path),
            None => machine.home().map(|home| home.join(under_home)),
        }
    };
    let ours = |dir: PathBuf| dir.join(VENDOR).join(APP);
    let state = ours(base("XDG_STATE_HOME", ".local/state")?);
    let (runtime, runtime_is_fallback) = match machine.absolute("XDG_RUNTIME_DIR", Platform::Xdg) {
        Some(dir) => (ours(dir), false),
        None => (state.join("runtime"), true),
    };
    // The places the single-folder profile could have been: the old
    // `default_dir`, with a relative `$XDG_DATA_HOME` left out (it named a
    // folder relative to wherever F1R3Gaze was started).
    let mut legacy = Vec::with_capacity(2);
    if let Some(data_home) = machine.absolute("XDG_DATA_HOME", Platform::Xdg) {
        legacy.push(data_home.join("f1r3gaze"));
    }
    if let Ok(home) = machine.home() {
        push_unique(&mut legacy, home.join(".local/share/f1r3gaze"));
    }
    Ok(Layout {
        kind: Kind::Platform(Platform::Xdg),
        config: ours(base("XDG_CONFIG_HOME", ".config")?),
        data: ours(base("XDG_DATA_HOME", ".local/share")?),
        cache: ours(base("XDG_CACHE_HOME", ".cache")?),
        state,
        runtime,
        runtime_is_fallback,
        system_config: machine
            .path_list("XDG_CONFIG_DIRS", &["/etc/xdg"])
            .into_iter()
            .map(ours)
            .collect(),
        system_themes: machine
            .path_list("XDG_DATA_DIRS", &["/usr/local/share", "/usr/share"])
            .into_iter()
            .map(|dir| ours(dir).join("themes"))
            .collect(),
        legacy,
    })
}

fn macos_layout(machine: &Machine) -> Result<Layout, LayoutError> {
    let home = machine.home()?;
    let support = home.join("Library/Application Support").join(BUNDLE_ID);
    let state = support.join("state");
    let (runtime, runtime_is_fallback) = match machine.absolute("TMPDIR", Platform::MacOs) {
        Some(dir) => (dir.join(BUNDLE_ID), false),
        None => (state.join("runtime"), true),
    };
    let system = Path::new("/Library/Application Support").join(BUNDLE_ID);
    Ok(Layout {
        kind: Kind::Platform(Platform::MacOs),
        config: support.join("config"),
        data: support.join("data"),
        state,
        cache: home.join("Library/Caches").join(BUNDLE_ID),
        runtime,
        runtime_is_fallback,
        system_config: vec![system.join("config")],
        system_themes: vec![system.join("themes")],
        legacy: vec![home.join("Library/Application Support/F1R3Gaze")],
    })
}

fn windows_layout(machine: &Machine) -> Result<Layout, LayoutError> {
    let roaming = machine
        .roaming_app_data
        .clone()
        .or_else(|| machine.absolute("APPDATA", Platform::Windows))
        .ok_or(LayoutError::NoAppData)?;
    let local = machine
        .local_app_data
        .clone()
        .or_else(|| machine.absolute("LOCALAPPDATA", Platform::Windows))
        .ok_or(LayoutError::NoAppData)?;
    let ours = local.join(VENDOR).join(APP);
    let system = machine
        .absolute("ProgramData", Platform::Windows)
        .map(|dir| dir.join(VENDOR).join(APP));
    // The old default_dir: %APPDATA%, else the home directory, then F1R3Gaze.
    let mut legacy = Vec::with_capacity(3);
    if let Some(app_data) = machine.absolute("APPDATA", Platform::Windows) {
        legacy.push(app_data.join("F1R3Gaze"));
    }
    push_unique(&mut legacy, roaming.join("F1R3Gaze"));
    if let Some(home) = &machine.home {
        push_unique(&mut legacy, home.join("F1R3Gaze"));
    }
    Ok(Layout {
        kind: Kind::Platform(Platform::Windows),
        config: roaming.join(VENDOR).join(APP),
        data: ours.join("data"),
        state: ours.join("state"),
        cache: ours.join("cache"),
        runtime: ours.join("runtime"),
        runtime_is_fallback: false,
        system_config: system.iter().cloned().collect(),
        system_themes: system.iter().map(|dir| dir.join("themes")).collect(),
        legacy,
    })
}

/// Whether `value` is an absolute path by `platform`'s rules: a leading `/`
/// on Linux and macOS; a drive (`C:\` or `C:/`) or a UNC prefix (`\\`) on
/// Windows.
fn is_absolute_on(platform: Platform, value: &OsStr) -> bool {
    let text = value.to_string_lossy();
    match platform {
        Platform::Xdg | Platform::MacOs => text.starts_with('/'),
        Platform::Windows => {
            let bytes = text.as_bytes();
            let drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/');
            drive || text.starts_with("\\\\")
        }
    }
}

/// The entries of a colon-separated list, byte for byte.
fn split_colons(value: &OsStr) -> Vec<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value
            .as_bytes()
            .split(|&b| b == b':')
            .map(|entry| OsStr::from_bytes(entry).to_os_string())
            .collect()
    }
    #[cfg(not(unix))]
    {
        // Non-UTF-8 list entries cannot be split exactly here; such a list is
        // only ever an XDG list read on a Unix system.
        value.to_string_lossy().split(':').map(OsString::from).collect()
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

impl Layout {
    /// Everything under `root`: `root/{config,data,state,cache,runtime}`. No
    /// system-wide layers, so the profile is the same on every machine; an
    /// old single-folder profile at `root` itself is migrated in place.
    pub fn portable(root: &Path) -> Layout {
        Layout {
            kind: Kind::Portable(root.to_path_buf()),
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            cache: root.join("cache"),
            runtime: root.join("runtime"),
            runtime_is_fallback: false,
            system_config: Vec::new(),
            system_themes: Vec::new(),
            legacy: vec![root.to_path_buf()],
        }
    }

    pub fn root(&self, class: Class) -> &Path {
        match class {
            Class::Config => &self.config,
            Class::Data => &self.data,
            Class::State => &self.state,
            Class::Cache => &self.cache,
            Class::Runtime => &self.runtime,
        }
    }

    /// The class whose root holds `path`, and the path below that root. The
    /// longest root wins, so a runtime root under state is runtime.
    pub fn class_of(&self, path: &Path) -> Option<(Class, PathBuf)> {
        Class::ALL
            .iter()
            .filter_map(|&class| {
                path.strip_prefix(self.root(class))
                    .ok()
                    .map(|below| (class, below.to_path_buf()))
            })
            .max_by_key(|(class, _)| self.root(*class).components().count())
    }

    /// Where backups of a class's files go.
    pub fn backups(&self, class: Class) -> PathBuf {
        self.root(class).join("backups")
    }

    // ── config ──
    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.toml")
    }
    pub fn settings_example(&self) -> PathBuf {
        self.config.join("settings.toml.example")
    }
    pub fn themes_dir(&self) -> PathBuf {
        self.config.join("themes")
    }

    // ── data ──
    /// The record that this layout is in use (`layout.json`): written on the
    /// first start, fresh or migrated; a later start never migrates again.
    pub fn marker_file(&self) -> PathBuf {
        self.data.join("layout.json")
    }
    /// The plan of a migration in progress.
    pub fn migration_dir(&self) -> PathBuf {
        self.data.join(".migration")
    }
    /// There while a start is under way: a start that finds it follows one
    /// that did not finish, and syncs what that one may have left pending.
    pub fn busy_file(&self) -> PathBuf {
        self.data.join(".startup-busy")
    }
    pub fn user_id_file(&self) -> PathBuf {
        self.data.join("user-id")
    }
    pub fn wallet_dir(&self) -> PathBuf {
        self.data.join("wallet")
    }
    pub fn keys_dir(&self) -> PathBuf {
        self.wallet_dir().join("keys")
    }
    pub fn exports_dir(&self) -> PathBuf {
        self.wallet_dir().join("exports")
    }
    pub fn permissions_dir(&self) -> PathBuf {
        self.data.join("permissions")
    }
    pub fn grants_file(&self) -> PathBuf {
        self.permissions_dir().join("grants.tsv")
    }
    pub fn site_data_dir(&self) -> PathBuf {
        self.data.join("site-data")
    }
    pub fn origins_file(&self) -> PathBuf {
        self.site_data_dir().join("origins.json")
    }
    pub fn trust_dir(&self) -> PathBuf {
        self.data.join("trust")
    }
    pub fn trust_file(&self) -> PathBuf {
        self.trust_dir().join("freshness.tsv")
    }
    pub fn replay_logs_dir(&self) -> PathBuf {
        self.data.join("replay-logs")
    }
    /// The anchor lock: held by the one F1R3Gaze that owns this data, whatever
    /// runtime directory each process sees.
    pub fn anchor_lock_file(&self) -> PathBuf {
        self.data.join("instance.lock")
    }

    // ── state ──
    pub fn window_file(&self) -> PathBuf {
        self.state.join("window.json")
    }
    pub fn session_file(&self) -> PathBuf {
        self.state.join("session.json")
    }
    pub fn history_file(&self) -> PathBuf {
        self.state.join("history.json")
    }

    // ── cache ──
    pub fn content_dir(&self) -> PathBuf {
        self.cache.join("content")
    }

    // ── runtime ──
    pub fn lock_file(&self) -> PathBuf {
        self.runtime.join("instance.lock")
    }
    /// Who holds the lock, readable while it is held (Windows locks the lock
    /// file's bytes).
    pub fn lock_info_file(&self) -> PathBuf {
        self.runtime.join("instance.pid")
    }

    /// Every directory start-up creates, parents first. `backups/` folders
    /// appear only when something is backed up.
    pub fn skeleton(&self) -> Vec<PathBuf> {
        vec![
            self.config.clone(),
            self.themes_dir(),
            self.data.clone(),
            self.wallet_dir(),
            self.keys_dir(),
            self.exports_dir(),
            self.permissions_dir(),
            self.site_data_dir(),
            self.trust_dir(),
            self.replay_logs_dir(),
            self.state.clone(),
            self.cache.clone(),
            self.content_dir(),
            self.runtime.clone(),
        ]
    }

    /// The roots, one per line, for `f1r3gaze paths`: `class<TAB>path`, then
    /// the system-wide and legacy places.
    pub fn describe(&self) -> String {
        let mut out = String::with_capacity(512);
        for class in Class::ALL {
            out.push_str(&format!("{}\t{}\n", class.name(), self.root(class).display()));
        }
        for dir in &self.system_config {
            out.push_str(&format!("system-config\t{}\n", dir.display()));
        }
        for dir in &self.system_themes {
            out.push_str(&format!("system-themes\t{}\n", dir.display()));
        }
        for dir in &self.legacy {
            out.push_str(&format!("legacy\t{}\n", dir.display()));
        }
        out
    }
}

/// The layout this process uses: `--profile DIR` if given, else a non-empty
/// `F1R3GAZE_PROFILE`, else the platform's. A relative profile directory is
/// taken relative to the current directory.
pub fn locate(profile_arg: Option<PathBuf>, machine: &Machine) -> Result<Layout, String> {
    let chosen = profile_arg.or_else(|| {
        std::env::var_os(PROFILE_VAR)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    });
    match chosen {
        Some(root) => std::path::absolute(&root)
            .map(|root| Layout::portable(&root))
            .map_err(|e| format!("{}: {e}", root.display())),
        None => platform_layout(Platform::current(), machine).map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests;
