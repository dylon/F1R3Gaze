//! Helpers shared by this crate's unit tests: throw-away profiles, local
//! pages, and the layout invariant that catches stale text colours.

use blitz_dom::BaseDocument;
use std::path::{Path, PathBuf};

/// A temporary profile directory, removed when dropped (also on panic).
///
/// `settings.conf` empties the shard observer list, so `Engine::new` starts
/// no event thread dialling a local node.
pub struct ScratchProfile(PathBuf);

impl ScratchProfile {
    pub fn new(name: &str) -> ScratchProfile {
        let dir = std::env::temp_dir().join(format!("gaze-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch profile");
        std::fs::write(dir.join("settings.conf"), "observers =\n").expect("write settings.conf");
        ScratchProfile(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Ask windows on this profile to reopen the sidebar as the workspace
    /// left it (`restore_sidebar`), for tests that start on a panel. Call it
    /// before the profile's `Engine` is created, which reads the settings.
    pub fn restore_sidebar(&self) {
        let path = self.0.join("settings.conf");
        let mut settings = std::fs::read_to_string(&path).expect("read settings.conf");
        settings.push_str("restore_sidebar = true\n");
        std::fs::write(&path, settings).expect("write settings.conf");
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
        let path = self.0.join(file);
        std::fs::write(&path, html).expect("write the page");
        url::Url::from_file_path(&path)
            .expect("absolute page path")
            .to_string()
    }
}

impl Drop for ScratchProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
