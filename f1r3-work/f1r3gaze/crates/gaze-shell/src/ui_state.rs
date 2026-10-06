//! Host-owned workspace and visit history. Only URLs, titles and layout choices
//! are persisted; capabilities, keys, documents and running sessions are not.

use gaze_fs::{Perm, StdFs};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const HISTORY_LIMIT: usize = 5_000;
const TAB_LIMIT: usize = 100;

fn exact_score(query: &str, title: &str, url: &str) -> Option<usize> {
    if query.is_empty() || title.to_lowercase().contains(query) {
        Some(0)
    } else if url.to_lowercase().contains(query) {
        Some(1)
    } else {
        None
    }
}

/// Match the visible name or address, including a bounded typo in a single
/// word. The bounded native Rust distance avoids scanning long URLs with an
/// unbounded edit-distance matrix on every keypress.
pub fn search_score(query: &str, title: &str, url: &str) -> Option<usize> {
    let query = query.trim().to_lowercase();
    if let Some(score) = exact_score(&query, title, url) {
        return Some(score);
    }
    let (len, max_distance) = fuzzy_bound(&query)?;
    [(title, 3), (url, 6)].into_iter().find_map(|(text, base)| {
        fuzzy_word(&query, len, max_distance, text).map(|(_, distance)| base + distance)
    })
}

/// Where `query` matches `text` by the rules of [`search_score`]: its first
/// case-insensitive occurrence, else the first word within the typo bound.
/// A byte range of `text` on character boundaries; `None` for an empty
/// query. The chrome highlights it so a row shows why it matched.
pub fn match_span(query: &str, text: &str) -> Option<Range<usize>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    find_ignoring_case(text, &query).or_else(|| {
        let (len, max_distance) = fuzzy_bound(&query)?;
        fuzzy_word(&query, len, max_distance, text).map(|(range, _)| range)
    })
}

/// The first occurrence of `needle` (already lowercase) in `text` ignoring
/// case: the byte range of `text` covering every character the match
/// touches. Agrees exactly with `text.to_lowercase().contains(needle)`,
/// which `exact_score` uses.
pub fn find_ignoring_case(text: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let (lower, origins) = lowercase_with_origins(text);
    let at = lower.find(needle)?;
    Some(origins[at].start..origins[at + needle.len() - 1].end)
}

/// `text.to_lowercase()`, and for each of its bytes the byte range of the
/// character of `text` it came from.
///
/// `str::to_lowercase` maps each character with `char::to_lowercase`,
/// except a final capital sigma, which becomes `ς` instead of `σ`: still
/// exactly one character. The lowercase string therefore aligns with the
/// original character by character, each original character contributing
/// `c.to_lowercase().count()` characters (`İ` contributes two).
fn lowercase_with_origins(text: &str) -> (String, Vec<Range<usize>>) {
    let lower = text.to_lowercase();
    let mut origins = Vec::with_capacity(lower.len());
    let mut lowered = lower.chars();
    for (start, c) in text.char_indices() {
        let origin = start..start + c.len_utf8();
        for _ in 0..c.to_lowercase().count() {
            let l = lowered
                .next()
                .expect("str::to_lowercase yields as many characters as char::to_lowercase");
            origins.extend(std::iter::repeat_n(origin.clone(), l.len_utf8()));
        }
    }
    (lower, origins)
}

/// For a query that may match with a typo: its length in characters and
/// the edit distance allowed. Short, spaced, or all-digit queries only
/// match exactly.
fn fuzzy_bound(query: &str) -> Option<(usize, usize)> {
    let len = query.chars().count();
    match (3..=48).contains(&len)
        && !query.chars().any(char::is_whitespace)
        && !query.chars().all(|c| c.is_ascii_digit())
    {
        true => Some((len, if len < 6 { 1 } else { 2 })),
        false => None,
    }
}

/// The first word of `text` (a maximal run of alphanumeric characters)
/// within `max_distance` edits of `query`: its byte range and the distance.
fn fuzzy_word(
    query: &str,
    len: usize,
    max_distance: usize,
    text: &str,
) -> Option<(Range<usize>, usize)> {
    words(text).find_map(|range| {
        let word = text[range.clone()].to_lowercase();
        match word.chars().count().abs_diff(len) <= max_distance {
            true => liblevenshtein::distance::standard_distance_bounded(query, &word, max_distance)
                .map(|distance| (range, distance)),
            false => None,
        }
    })
}

/// Byte ranges of the maximal runs of alphanumeric characters in `text`.
fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        let (start, _) = chars.find(|(_, c)| c.is_alphanumeric())?;
        let mut end = text.len();
        while let Some(&(at, c)) = chars.peek() {
            if !c.is_alphanumeric() {
                end = at;
                break;
            }
            chars.next();
        }
        Some(start..end)
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub theme: String,
    pub sidebar_open: bool,
    pub panel: String,
    pub tree_tabs: bool,
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub visits: Vec<Visit>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            sidebar_open: false,
            panel: "tabs".into(),
            tree_tabs: false,
            tabs: Vec::new(),
            active: 0,
            visits: Vec::new(),
        }
    }
}

impl UiState {
    pub fn load(dir: &Path) -> Self {
        let Ok(bytes) = std::fs::read(dir.join("workspace.json")) else {
            return Self::default();
        };
        let Ok(mut state) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        state.tabs.truncate(TAB_LIMIT);
        state.visits.truncate(HISTORY_LIMIT);
        if state.active >= state.tabs.len() {
            state.active = 0;
        }
        if !matches!(state.theme.as_str(), "dark" | "light" | "custom") {
            state.theme = "dark".into();
        }
        if !matches!(
            state.panel.as_str(),
            "tabs" | "history" | "sites" | "grants" | "wallet" | "console" | "appearance"
        ) {
            state.panel = "tabs".into();
        }
        state
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        gaze_fs::create_dir_durably(&StdFs, dir).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        gaze_fs::write_atomic(&StdFs, &dir.join("workspace.json"), &bytes, Perm::Private)
            .map_err(|e| e.to_string())
    }

    pub fn visit(&mut self, url: &str, title: &str) {
        record_visit(&mut self.visits, url, title, now_seconds());
    }

    /// Remove every visit of `url` from history; returns how many there were.
    pub fn forget(&mut self, url: &str) -> usize {
        forget_visits(&mut self.visits, url)
    }

    pub fn suggestions(&self, query: &str, open: &[SavedTab]) -> Vec<SavedTab> {
        suggest(&self.visits, query, open)
    }

    /// The legacy `workspace.json` as this version keeps it: the session,
    /// the history, and the theme name, which belongs to `settings.toml`
    /// now (profile/migrate.rs folds it in).
    pub fn split(self) -> (SessionState, History, String) {
        let session = SessionState {
            version: SESSION_VERSION,
            sidebar_open: self.sidebar_open,
            panel: self.panel,
            tree_tabs: self.tree_tabs,
            tabs: self.tabs,
            active: self.active,
        };
        let history = History {
            version: HISTORY_VERSION,
            visits: self.visits,
        };
        (session.sanitized(), history.sanitized(), self.theme)
    }
}

fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Records a visit at `at`, newest first, keeping at most [`HISTORY_LIMIT`].
/// Built-in pages are not recorded; a page visited again within 10 seconds
/// (a redirect, a reload) updates its last visit instead of adding one.
fn record_visit(visits: &mut Vec<Visit>, url: &str, title: &str, at: u64) {
    if url.starts_with("gaze://") || url.is_empty() {
        return;
    }
    if let Some(last) = visits.first_mut()
        && last.url == url
        && at.saturating_sub(last.at) < 10
    {
        last.title = title.to_string();
        last.at = at;
        return;
    }
    visits.insert(
        0,
        Visit {
            url: url.into(),
            title: title.into(),
            at,
        },
    );
    visits.truncate(HISTORY_LIMIT);
}

/// Removes every visit of `url`; returns how many there were.
fn forget_visits(visits: &mut Vec<Visit>, url: &str) -> usize {
    let before = visits.len();
    visits.retain(|visit| visit.url != url);
    before - visits.len()
}

/// At most eight open tabs and visits matching `query`: exact matches
/// first, then matches with a typo among the open tabs and the 200 most
/// recent visits.
fn suggest(visits: &[Visit], query: &str, open: &[SavedTab]) -> Vec<SavedTab> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for t in open
        .iter()
        .cloned()
        .chain(visits.iter().map(|v| SavedTab {
            url: v.url.clone(),
            title: v.title.clone(),
            parent: None,
        }))
    {
        if exact_score(&q, &t.title, &t.url).is_some()
            && !out.iter().any(|x: &SavedTab| x.url == t.url)
        {
            out.push(t.clone());
        }
        if out.len() >= 8 {
            break;
        }
    }
    if out.len() < 8 {
        // Fuzzy suggestions consider open tabs and recent visits. Older
        // visits remain reachable through exact history search.
        for t in open
            .iter()
            .cloned()
            .chain(visits.iter().take(200).map(|v| SavedTab {
                url: v.url.clone(),
                title: v.title.clone(),
                parent: None,
            }))
        {
            if search_score(&q, &t.title, &t.url).is_some()
                && !out.iter().any(|x| x.url == t.url)
            {
                out.push(t);
            }
            if out.len() >= 8 {
                break;
            }
        }
    }
    out
}

/// The version of `session.json` and of `history.json` this build writes.
pub const SESSION_VERSION: u32 = 1;
pub const HISTORY_VERSION: u32 = 1;

/// The panels the sidebar can show.
pub const PANELS: [&str; 7] = ["tabs", "history", "sites", "grants", "wallet", "console", "appearance"];

/// Why a state file cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// It is not JSON of the file's shape.
    Corrupt(String),
    /// A newer F1R3Gaze wrote it, in a format this one does not know. It is
    /// left as it is.
    Newer { version: u32, known: u32 },
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Corrupt(why) => write!(f, "not a state file of this kind: {why}"),
            StateError::Newer { version, known } => {
                write!(f, "written by a newer F1R3Gaze (format {version}; this one knows {known})")
            }
        }
    }
}

/// The `version` of a state file, read on its own so that a newer format
/// is recognised even when its other fields have changed shape.
#[derive(Deserialize)]
struct Versioned {
    #[serde(default)]
    version: u32,
}

/// Reads a state file of type `T`, written at most at version `known`.
pub(crate) fn read_state<T: serde::de::DeserializeOwned>(bytes: &[u8], known: u32) -> Result<T, StateError> {
    let versioned: Versioned = serde_json::from_slice(bytes).map_err(|e| StateError::Corrupt(e.to_string()))?;
    match versioned.version > known {
        true => Err(StateError::Newer {
            version: versioned.version,
            known,
        }),
        false => serde_json::from_slice(bytes).map_err(|e| StateError::Corrupt(e.to_string())),
    }
}

/// A state file's bytes: indented JSON ending in a line break.
pub(crate) fn state_bytes<T: Serialize>(state: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(state).expect("state types serialize: plain fields and strings");
    bytes.push(b'\n');
    bytes
}

/// `state/session.json`: the open tabs and the sidebar's layout.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SessionState {
    pub version: u32,
    pub sidebar_open: bool,
    pub panel: String,
    pub tree_tabs: bool,
    pub tabs: Vec<SavedTab>,
    pub active: usize,
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState {
            version: SESSION_VERSION,
            sidebar_open: false,
            panel: "tabs".into(),
            tree_tabs: false,
            tabs: Vec::new(),
            active: 0,
        }
    }
}

impl SessionState {
    /// Reads `session.json`, keeping what can be shown.
    pub fn read(bytes: &[u8]) -> Result<SessionState, StateError> {
        read_state::<SessionState>(bytes, SESSION_VERSION).map(SessionState::sanitized)
    }

    /// At most 100 tabs, an active tab that exists, a panel that exists,
    /// and this build's version.
    pub fn sanitized(mut self) -> SessionState {
        self.version = SESSION_VERSION;
        self.tabs.truncate(TAB_LIMIT);
        if self.active >= self.tabs.len() {
            self.active = 0;
        }
        if !PANELS.contains(&self.panel.as_str()) {
            self.panel = "tabs".into();
        }
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        state_bytes(self)
    }
}

/// `state/history.json`: the pages visited, newest first.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct History {
    pub version: u32,
    pub visits: Vec<Visit>,
}

impl Default for History {
    fn default() -> Self {
        History {
            version: HISTORY_VERSION,
            visits: Vec::new(),
        }
    }
}

impl History {
    /// Reads `history.json`, keeping the newest [`HISTORY_LIMIT`] visits.
    pub fn read(bytes: &[u8]) -> Result<History, StateError> {
        read_state::<History>(bytes, HISTORY_VERSION).map(History::sanitized)
    }

    pub fn sanitized(mut self) -> History {
        self.version = HISTORY_VERSION;
        self.visits.truncate(HISTORY_LIMIT);
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        state_bytes(self)
    }

    pub fn visit(&mut self, url: &str, title: &str) {
        record_visit(&mut self.visits, url, title, now_seconds());
    }

    /// Remove every visit of `url`; returns how many there were.
    pub fn forget(&mut self, url: &str) -> usize {
        forget_visits(&mut self.visits, url)
    }

    pub fn suggestions(&self, query: &str, open: &[SavedTab]) -> Vec<SavedTab> {
        suggest(&self.visits, query, open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str, parent: Option<usize>) -> SavedTab {
        SavedTab {
            url: url.into(),
            title: url.into(),
            parent,
        }
    }

    #[test]
    fn a_session_round_trips_and_is_kept_in_bounds() {
        let session = SessionState {
            sidebar_open: true,
            panel: "wallet".into(),
            tree_tabs: true,
            tabs: vec![tab("https://a.example/", None), tab("https://b.example/", Some(0))],
            active: 1,
            ..SessionState::default()
        };
        assert_eq!(SessionState::read(&session.to_bytes()), Ok(session.clone()));
        assert!(session.to_bytes().ends_with(b"}\n"));
        let wild = br#"{"panel": "nowhere", "active": 7, "tabs": [{"url": "u", "title": "t"}], "later": 1}"#;
        let read = SessionState::read(wild).expect("unknown fields are ignored");
        assert_eq!((read.panel.as_str(), read.active, read.tabs.len()), ("tabs", 0, 1));
        let many: Vec<SavedTab> = (0..150).map(|i| tab(&format!("https://{i}.example/"), None)).collect();
        let crowded = SessionState { tabs: many, ..SessionState::default() };
        assert_eq!(SessionState::read(&crowded.to_bytes()).expect("read").tabs.len(), TAB_LIMIT);
        assert_eq!(SessionState::read(b"{}"), Ok(SessionState::default()), "missing fields take their defaults");
    }

    #[test]
    fn a_newer_state_file_is_recognised_whatever_its_shape() {
        assert_eq!(
            SessionState::read(br#"{"version": 2, "tabs": "a format to come"}"#),
            Err(StateError::Newer { version: 2, known: SESSION_VERSION })
        );
        assert_eq!(
            History::read(br#"{"version": 9}"#),
            Err(StateError::Newer { version: 9, known: HISTORY_VERSION })
        );
        for corrupt in [&b"[1, 2]"[..], b"{\"tabs\": 3}", b"{\"visits\": [{\"url\": 1}]}", b"not json", b"", b"{\"version\": -1}"] {
            assert!(
                matches!(SessionState::read(corrupt), Err(StateError::Corrupt(_)))
                    || matches!(History::read(corrupt), Err(StateError::Corrupt(_))),
                "{}",
                String::from_utf8_lossy(corrupt)
            );
        }
        assert!(matches!(History::read(b"{\"visits\": 5}"), Err(StateError::Corrupt(_))));
    }

    #[test]
    fn history_round_trips_newest_first_and_bounded() {
        let mut history = History::default();
        for i in 0..(HISTORY_LIMIT + 2) {
            history.visits.insert(
                0,
                Visit {
                    url: format!("https://example.org/{i}"),
                    title: "Example".into(),
                    at: i as u64,
                },
            );
        }
        let read = History::read(&history.to_bytes()).expect("read");
        assert_eq!(read.visits.len(), HISTORY_LIMIT);
        assert_eq!(read.visits[0].url, format!("https://example.org/{}", HISTORY_LIMIT + 1));
        let mut h = History::default();
        h.visit("https://a.example/", "A");
        h.visit("https://a.example/", "A again");
        h.visit("gaze://newtab", "never kept");
        assert_eq!(h.visits.len(), 1, "a reload within ten seconds updates the visit");
        assert_eq!(h.visits[0].title, "A again");
        assert_eq!(h.suggestions("a.exa", &[]).len(), 1);
        assert_eq!(h.forget("https://a.example/"), 1);
    }

    #[test]
    fn a_legacy_workspace_splits_into_session_history_and_theme() {
        let legacy = UiState {
            theme: "light".into(),
            sidebar_open: true,
            panel: "history".into(),
            tree_tabs: true,
            tabs: vec![tab("https://a.example/", None)],
            active: 0,
            visits: vec![Visit {
                url: "https://a.example/".into(),
                title: "A".into(),
                at: 5,
            }],
        };
        let (session, history, theme) = legacy.clone().split();
        assert_eq!(theme, "light");
        assert_eq!(
            session,
            SessionState {
                version: SESSION_VERSION,
                sidebar_open: true,
                panel: "history".into(),
                tree_tabs: true,
                tabs: legacy.tabs.clone(),
                active: 0,
            }
        );
        assert_eq!(history, History { version: HISTORY_VERSION, visits: legacy.visits.clone() });
    }

    #[test]
    fn history_is_bounded_and_suggests_unique_urls() {
        let mut s = UiState::default();
        for i in 0..(HISTORY_LIMIT + 2) {
            s.visit(&format!("https://example.org/{i}"), "Example");
        }
        assert_eq!(s.visits.len(), HISTORY_LIMIT);
        let open = vec![SavedTab {
            url: "https://example.org/5001".into(),
            title: "Example".into(),
            parent: None,
        }];
        let found = s.suggestions("5001", &open);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn forgetting_a_page_removes_every_visit_of_it() {
        let mut s = UiState {
            visits: ["https://a.example/", "https://b.example/", "https://a.example/"]
                .iter()
                .enumerate()
                .map(|(i, url)| Visit {
                    url: (*url).into(),
                    title: format!("visit {i}"),
                    at: i as u64,
                })
                .collect(),
            ..UiState::default()
        };
        assert_eq!(s.forget("https://a.example/"), 2);
        assert_eq!(s.visits.len(), 1);
        assert_eq!(s.visits[0].url, "https://b.example/");
        assert_eq!(s.forget("https://missing.example/"), 0);
    }

    #[test]
    fn native_search_tolerates_short_typos_but_keeps_exact_first() {
        assert_eq!(
            search_score("gaze", "F1R3Gaze", "https://example.org"),
            Some(0)
        );
        assert_eq!(
            search_score("f1r3gze", "F1R3Gaze", "https://example.org"),
            Some(4)
        );
        assert_eq!(
            search_score("zzzz", "F1R3Gaze", "https://example.org"),
            None
        );
        let mut state = UiState::default();
        state.visit("https://example.org", "F1R3Gaze");
        assert_eq!(state.suggestions("f1r3gze", &[])[0].title, "F1R3Gaze");
    }

    #[test]
    fn match_spans_point_at_the_original_characters() {
        let span = |query: &str, text: &'static str| match_span(query, text).map(|r| &text[r]);
        // Exact, ignoring case, anywhere in the text.
        assert_eq!(span("gaze", "file:///tmp/f1r3gaze-ui/notes.html"), Some("gaze"));
        assert_eq!(span("WIKI", "https://en.Wikipedia.org/"), Some("Wiki"));
        assert_eq!(span("  wiki ", "https://en.wikipedia.org/"), Some("wiki"));
        // Multibyte characters keep their boundaries.
        assert_eq!(span("ünï", "ÀBC Ünïcode"), Some("Ünï"));
        // A final capital sigma lowercases to ς, as str::to_lowercase does.
        assert_eq!(span("οδος", "ΟΔΟΣ"), Some("ΟΔΟΣ"));
        // İ lowercases to two characters (i + combining dot); a match that
        // touches either covers the whole original character.
        assert_eq!(span("i", "İstanbul"), Some("İ"));
        assert_eq!(span("stan", "İstanbul"), Some("stan"));
        // Otherwise the first word within the typo bound.
        assert_eq!(span("capabilty", "Capability-based security"), Some("Capability"));
        assert_eq!(span("f1r3gze", "Welcome to F1R3Gaze"), Some("F1R3Gaze"));
        // No match, and no span for an empty query.
        assert_eq!(span("zzzz", "F1R3Gaze"), None);
        assert_eq!(span("", "F1R3Gaze"), None);
        assert_eq!(span("ab", "a-b"), None);
    }

    #[test]
    fn a_row_is_highlighted_exactly_when_it_matches() {
        let rows = [
            ("Field notes on gaze and capability", "file:///tmp/site/notes.html"),
            ("Cross-site fetch", "file:///tmp/f1r3gaze-ui-snapshots/site/prompt.html"),
            ("Capability-based security - Wikipedia", "https://en.wikipedia.org/wiki/Capability-based_security"),
            ("ΟΔΟΣ", "https://example.org/odos"),
            ("İstanbul", "https://example.org/ist"),
            ("", "gaze://newtab"),
        ];
        let queries = [
            "gaze", "GAZE", "capabilty", "wiki", "odos", "οδος", "i", "zzzz", "f1r3gze", "1234",
            "a b", "notes.html", "site/p",
        ];
        for (title, url) in rows {
            for query in queries {
                let matched = search_score(query, title, url).is_some();
                let shown = match_span(query, title).is_some() || match_span(query, url).is_some();
                assert_eq!(matched, shown, "{query:?} against {title:?} / {url:?}");
            }
        }
    }

    #[test]
    fn words_are_the_alphanumeric_runs() {
        let text = "--a1 bc..ü--";
        let found: Vec<&str> = words(text).map(|r| &text[r]).collect();
        assert_eq!(found, ["a1", "bc", "ü"]);
        assert_eq!(words("").count(), 0);
        assert_eq!(words("...").count(), 0);
        assert_eq!(words("word").map(|r| &"word"[r]).collect::<Vec<_>>(), ["word"]);
    }
}
