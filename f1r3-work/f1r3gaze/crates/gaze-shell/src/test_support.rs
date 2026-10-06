//! Helpers shared by this crate's unit tests: throw-away profiles, local
//! pages, and the layout invariant that catches stale text colours.

use crate::engine::Engine;
use crate::profile::layout::Layout;
use crate::profile::report::Report;
use crate::profile::{KeystoreKind, Locking, Profile, StartEnv, new_user_id};
use blitz_dom::BaseDocument;
use gaze_fs::{Fs, StdFs};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;
// What only the chrome's tests use (they need the window).
#[cfg(feature = "window")]
use {
    crate::profile::{StateFile, settings},
    crate::theme::ThemeChoice,
    gaze_fs::{Perm, TraceFs},
    std::path::Path,
};

/// Taken by every test that starts a process, while the process starts, and
/// by every test that releases an instance lock and takes it again.
///
/// A process being started holds a copy of each of this process's file
/// descriptors until it executes its program: close-on-exec takes effect
/// only then. An `flock` lock lasts while any copy of its descriptor is
/// open. So a lock released while another test starts a process can still
/// be held a moment later. Without this guard, 6 of 60 runs of the lock
/// tests beside the tests that start `dbus-send` failed (ledger S5, part 2).
static PROCESS_STARTS: Mutex<()> = Mutex::new(());

/// Takes [`PROCESS_STARTS`]. A test that failed while holding it leaves
/// nothing to repair, so the guard is taken even then.
pub fn process_starts() -> MutexGuard<'static, ()> {
    PROCESS_STARTS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A temporary profile under the scratch directory (`gaze_fs::scratch_dir`:
/// `$TMPDIR` or the target directory, never `/tmp`), removed when dropped
/// (also on panic). It is a portable root, `DIR/{config,data,state,cache,
/// runtime}`, opened as the browser opens one (`Profile::open`), without
/// the instance lock (only `main.rs` takes it).
///
/// Its `config/settings.toml` empties the shard observer list, so the
/// engine starts no event thread dialling a node, and its freshness records
/// stay in memory. It is never a `settings.conf`: that is an old profile's
/// file, which start-up would move.
pub struct ScratchProfile {
    root: PathBuf,
    layout: Layout,
}

impl ScratchProfile {
    pub fn new(name: &str) -> ScratchProfile {
        let root = gaze_fs::scratch_dir(&format!("gaze-shell-{name}"));
        let layout = Layout::portable(&root);
        std::fs::create_dir_all(&layout.config).expect("make config/");
        std::fs::write(layout.settings_file(), "[shard]\nobservers = []\n").expect("write settings.toml");
        ScratchProfile { root, layout }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The profile, opened on `fs`.
    pub fn open_on(&self, fs: Arc<dyn Fs + Send + Sync>) -> Profile {
        self.open_with(fs, Locking::Unlocked)
    }

    /// The profile, opened on `fs` as `locking` says: `Locking::ReadOnly`
    /// for a session that only reads.
    pub fn open_with(&self, fs: Arc<dyn Fs + Send + Sync>, locking: Locking) -> Profile {
        let env = StartEnv {
            fs,
            report: Report::silent(),
            now: SystemTime::now(),
            new_user_id,
            keystore: KeystoreKind::File,
        };
        Profile::open(self.layout.clone(), locking, env).expect("a scratch profile opens")
    }

    /// The profile's engine.
    pub fn engine(&self) -> Rc<Engine> {
        Engine::open(self.open_on(Arc::new(StdFs)))
    }
}

// Only the chrome's tests, which need the window, use these.
#[cfg(feature = "window")]
impl ScratchProfile {
    /// The portable root. Test pages are written here too: an HTML file is
    /// nothing start-up looks at.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The profile's engine, with the profile's own file operations traced
    /// (the engine's stores, wallets and grants write through `StdFs`).
    pub fn traced_engine(&self) -> (Rc<Engine>, Arc<TraceFs>) {
        let trace = Arc::new(TraceFs::new(Arc::new(StdFs)));
        (Engine::open(self.open_on(trace.clone())), trace)
    }

    /// Writes a state file as a window that closed would have left it.
    pub fn seed<T: StateFile>(&self, state: &T) {
        let path = T::MANAGED.path(&self.layout);
        std::fs::create_dir_all(path.parent().expect("a state file has a folder")).expect("make state/");
        gaze_fs::write_atomic(&StdFs, &path, &state.to_bytes(), Perm::Private).expect("write the state file");
    }

    /// Chooses the theme in `settings.toml`, as the Appearance panel saves
    /// it. Call it before the engine is opened, which reads the settings.
    pub fn set_theme(&self, choice: &str) {
        let choice = ThemeChoice::parse(choice).expect("a theme choice");
        settings::set_theme(&StdFs, &self.layout.settings_file(), &choice).expect("save the theme");
    }

    /// A profile that also finds themes installed for everyone, in
    /// `<root>/installed-themes` (a portable root has none of its own).
    pub fn with_installed_themes(name: &str) -> ScratchProfile {
        let mut profile = ScratchProfile::new(name);
        profile.layout.system_themes = vec![profile.root.join("installed-themes")];
        profile
    }

    /// Writes a theme file installed for everyone (`with_installed_themes`);
    /// returns its path.
    pub fn installed_theme_file(&self, name: &str, css: &str) -> PathBuf {
        let dir = self.layout.system_themes.first().expect("made by with_installed_themes").clone();
        std::fs::create_dir_all(&dir).expect("make installed-themes/");
        let path = dir.join(format!("{name}.css"));
        std::fs::write(&path, css).expect("write the installed theme");
        path
    }

    /// Writes the theme file `themes/<name>.css`; returns its path.
    pub fn theme_file(&self, name: &str, css: &str) -> PathBuf {
        let dir = self.layout.themes_dir();
        std::fs::create_dir_all(&dir).expect("make themes/");
        let path = dir.join(format!("{name}.css"));
        std::fs::write(&path, css).expect("write the theme file");
        path
    }

    /// Ask windows on this profile to reopen the sidebar as the session left
    /// it (`[appearance] restore_sidebar`), for tests that start on a panel.
    /// Call it before the profile's engine is opened, which reads the
    /// settings.
    pub fn restore_sidebar(&self) {
        settings::write_setting(
            &StdFs,
            &self.layout.settings_file(),
            "appearance",
            "restore_sidebar",
            toml_edit::Value::from(true),
        )
        .expect("save restore_sidebar");
    }

    /// Write a static page (no f1r3lang) and return its `file://` URL.
    pub fn page(&self, file: &str, body: &str) -> String {
        self.document(
            file,
            &format!("<html><head><title>{file}</title></head><body>{body}</body></html>"),
        )
    }

    /// Write a complete HTML document and return its `file://` URL.
    pub fn document(&self, file: &str, html: &str) -> String {
        let path = self.root.join(file);
        std::fs::write(&path, html).expect("write the page");
        url::Url::from_file_path(&path)
            .expect("absolute page path")
            .to_string()
    }
}

impl Drop for ScratchProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// Disabled at the switch-over (ledger S14): the single-folder profile it
// made. Its `settings.conf` is an old profile's file, which start-up now
// moves; the profile is a portable root above.
// /// A temporary profile directory under the scratch directory
// /// (`gaze_fs::scratch_dir`: `$TMPDIR` or the target directory, never
// /// `/tmp`), removed when dropped (also on panic).
// ///
// /// `settings.conf` empties the shard observer list, so `Engine::new` starts
// /// no event thread dialling a local node.
// // Only the chrome's tests, which need the window, use it.
// #[cfg(feature = "window")]
// pub struct ScratchProfile(PathBuf);
//
// #[cfg(feature = "window")]
// impl ScratchProfile {
//     pub fn new(name: &str) -> ScratchProfile {
//         let dir = gaze_fs::scratch_dir(&format!("gaze-shell-{name}"));
//         std::fs::write(dir.join("settings.conf"), "observers =\n").expect("write settings.conf");
//         ScratchProfile(dir)
//     }
//
//     pub fn path(&self) -> &Path {
//         &self.0
//     }
//
//     /// Ask windows on this profile to reopen the sidebar as the workspace
//     /// left it (`restore_sidebar`), for tests that start on a panel. Call it
//     /// before the profile's `Engine` is created, which reads the settings.
//     pub fn restore_sidebar(&self) {
//         let path = self.0.join("settings.conf");
//         let mut settings = std::fs::read_to_string(&path).expect("read settings.conf");
//         settings.push_str("restore_sidebar = true\n");
//         std::fs::write(&path, settings).expect("write settings.conf");
//     }
//
//     /// Write a static page (no f1r3lang) and return its `file://` URL.
//     pub fn page(&self, file: &str, body: &str) -> String {
//         self.document(
//             file,
//             &format!("<html><head><title>{file}</title></head><body>{body}</body></html>"),
//         )
//     }
//
//     /// Write a complete HTML document and return its `file://` URL.
//     pub fn document(&self, file: &str, html: &str) -> String {
//         let path = self.0.join(file);
//         std::fs::write(&path, html).expect("write the page");
//         url::Url::from_file_path(&path)
//             .expect("absolute page path")
//             .to_string()
//     }
// }
//
// #[cfg(feature = "window")]
// impl Drop for ScratchProfile {
//     fn drop(&mut self) {
//         let _ = std::fs::remove_dir_all(&self.0);
//     }
// }

/// Labels whose glyphs would be painted in a colour other than their
/// element's current `color`.
///
/// Blitz wraps bare text inside a flex or grid box (every `<button>`, whose
/// UA style is `inline-flex`) in an anonymous block. That block's style is
/// computed once, when the box is built, and a colour-only restyle does not
/// rebuild boxes, so its glyphs keep the old colour. Every anonymous block
/// that carries visible text must therefore match its DOM parent's colour.
pub fn stale_text_colours(doc: &BaseDocument) -> Vec<String> {
    let mut stale = Vec::with_capacity(16);
    for (id, node) in doc.tree().iter() {
        if !node.is_anonymous() {
            continue;
        }
        let Some(parent) = node.parent.and_then(|p| doc.get_node(p)) else {
            continue;
        };
        // Generated content (::before/::after) is restyled with its element.
        if parent.before() == Some(id) || parent.after() == Some(id) {
            continue;
        }
        let carries_text = node
            .children
            .iter()
            .filter_map(|child| doc.get_node(*child))
            .any(|child| child.text_data().is_some_and(|t| !t.content.trim().is_empty()));
        if !carries_text {
            continue;
        }
        let (Some(own), Some(parents)) = (node.primary_styles(), parent.primary_styles()) else {
            continue;
        };
        if own.clone_color() != parents.clone_color() {
            stale.push(parent.text_content().trim().to_string());
        }
    }
    stale
}
