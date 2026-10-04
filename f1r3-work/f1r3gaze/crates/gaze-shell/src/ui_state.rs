//! Host-owned workspace and visit history. Only URLs, titles and layout choices
//! are persisted; capabilities, keys, documents and running sessions are not.

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
/// which [`exact_score`] uses.
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
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join("workspace.json");
        let temp = dir.join("workspace.json.part");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }

    pub fn visit(&mut self, url: &str, title: &str) {
        if url.starts_with("gaze://") || url.is_empty() {
            return;
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Preserve visits but collapse rapid redirects/reloads of the same page.
        if let Some(last) = self.visits.first_mut()
            && last.url == url
            && at.saturating_sub(last.at) < 10
        {
            last.title = title.to_string();
            last.at = at;
            return;
        }
        self.visits.insert(
            0,
            Visit {
                url: url.into(),
                title: title.into(),
                at,
            },
        );
        self.visits.truncate(HISTORY_LIMIT);
    }

    /// Remove every visit of `url` from history; returns how many there were.
    pub fn forget(&mut self, url: &str) -> usize {
        let before = self.visits.len();
        self.visits.retain(|visit| visit.url != url);
        before - self.visits.len()
    }

    pub fn suggestions(&self, query: &str, open: &[SavedTab]) -> Vec<SavedTab> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for t in open
            .iter()
            .cloned()
            .chain(self.visits.iter().map(|v| SavedTab {
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
                .chain(self.visits.iter().take(200).map(|v| SavedTab {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
