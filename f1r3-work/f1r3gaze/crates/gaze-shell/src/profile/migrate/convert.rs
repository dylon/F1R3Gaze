//! An old profile's `settings.conf` (`key = value` lines), and its
//! conversion into `settings.toml` (design B.6, ledger S7).
//!
//! The old parser and template moved here from `profile.rs` unchanged:
//! [`LegacySettings::parse`] is what an older F1R3Gaze made of a file, so
//! it is the reference a conversion must agree with.

use crate::profile::settings::{self, DiagnosticKind, Key};
use gaze_shard::ShardConfig;
use std::path::Path;
use toml_edit::Value;

/// The settings an older F1R3Gaze read from `settings.conf`.
#[derive(Clone, Debug)]
pub struct LegacySettings {
    pub home: String,
    pub shard: ShardConfig,
    /// Blob mirrors tried for every hash, after a site's own.
    pub mirrors: Vec<String>,
    /// Content cache size.
    pub cache_bytes: u64,
    pub store_quota: u64,
    /// Refuse plain-HTTP top-level documents.
    pub https_only: bool,
    /// The Embers service for wallet balances, history and transfers (the
    /// one F1R3Sky uses). Deploys go to the node directly either way.
    pub embers_api: Option<String>,
    /// Upper bound on a transfer contract's `phlo_price × phlo_limit`.
    pub max_fee: i64,
    /// Reopen the sidebar at start when it was open as the last window
    /// closed. By default every window starts with it collapsed; the panel
    /// last shown is remembered either way (Ctrl+B reopens it).
    pub restore_sidebar: bool,
}

impl Default for LegacySettings {
    fn default() -> Self {
        LegacySettings {
            home: "gaze://newtab".into(),
            shard: ShardConfig::default(),
            mirrors: Vec::new(),
            cache_bytes: 512 * 1024 * 1024,
            store_quota: gaze_broker::DEFAULT_STORE_QUOTA,
            https_only: false,
            embers_api: None,
            max_fee: 10_000_000,
            restore_sidebar: false,
        }
    }
}

/// The last `settings.conf` template (commit `e7ff471`). Its lines are all
/// comments: it sets nothing.
pub const LEGACY_TEMPLATE: &str = "# F1R3Gaze settings. Lists are comma-separated.
# home = gaze://newtab
# observers = https://observer-1.example, https://observer-2.example, https://observer-3.example
# validator = https://validator.example
# shard_id = root
# quorum = 2
# mirrors = https://cdn.example/blob/
# https_only = false
# Wallet balances, history and transfers (the Embers service F1R3Sky uses):
# embers_api = https://embers.example
# max_fee = 10000000
# Reopen the sidebar at start if it was open when the window closed:
# restore_sidebar = false
";

impl LegacySettings {
    pub fn parse(text: &str) -> LegacySettings {
        let mut s = LegacySettings::default();
        let list = |v: &str| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "home" => s.home = v.into(),
                "observers" => s.shard.observers = list(v),
                "validator" => s.shard.validator = v.into(),
                "shard_id" => s.shard.shard_id = v.into(),
                "quorum" => s.shard.quorum = v.parse().unwrap_or(2),
                "phlo_price" => s.shard.phlo_price = v.parse().unwrap_or(1),
                "mirrors" => s.mirrors = list(v),
                "cache_bytes" => s.cache_bytes = v.parse().unwrap_or(s.cache_bytes),
                "store_quota" => s.store_quota = v.parse().unwrap_or(s.store_quota),
                "https_only" => s.https_only = v == "true",
                "embers_api" => s.embers_api = Some(v.to_string()).filter(|v| !v.is_empty()),
                "max_fee" => s.max_fee = v.parse().unwrap_or(s.max_fee),
                "restore_sidebar" => s.restore_sidebar = v == "true",
                _ => {}
            }
        }
        s
    }
}

/// Each old key and the setting it became, as `(old, table, name)`.
pub const LEGACY_KEYS: [(&str, &str, &str); 13] = [
    ("home", "browsing", "home"),
    ("https_only", "browsing", "https_only"),
    ("observers", "shard", "observers"),
    ("validator", "shard", "validator"),
    ("shard_id", "shard", "shard_id"),
    ("quorum", "shard", "quorum"),
    ("phlo_price", "shard", "phlo_price"),
    ("mirrors", "content", "mirrors"),
    ("cache_bytes", "content", "cache_bytes"),
    ("store_quota", "site_data", "store_quota"),
    ("embers_api", "wallet", "embers_api"),
    ("max_fee", "wallet", "max_fee"),
    ("restore_sidebar", "appearance", "restore_sidebar"),
];

/// The old key a setting came from.
fn legacy_name(k: &Key) -> Option<&'static str> {
    LEGACY_KEYS
        .iter()
        .find(|(_, table, name)| *table == k.table && *name == k.name)
        .map(|(old, _, _)| *old)
}

/// The settings `text` sets: the known old keys of its lines that the old
/// parser used (not a comment, with an `=`), in the order of
/// [`settings::KEYS`], each once.
pub fn keys_set(text: &str) -> Vec<&'static Key> {
    let set: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(k, _)| k.trim())
        .collect();
    settings::KEYS
        .iter()
        .filter(|k| legacy_name(k).is_some_and(|old| set.contains(&old)))
        .collect()
}

/// An unsigned number as TOML: an integer when TOML can hold it, else its
/// digits as a string, which the new file then reports as unusable (a
/// literal too large for TOML would make the whole file invalid).
fn unsigned(n: u64) -> Value {
    match i64::try_from(n) {
        Ok(n) => Value::from(n),
        Err(_) => Value::from(n.to_string()),
    }
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(String::as_str).collect())
}

/// The value the old parser gave a setting, as TOML; `None` for a setting
/// with no old key (the theme).
fn legacy_value(k: &Key, old: &LegacySettings) -> Option<Value> {
    Some(match (k.table, k.name) {
        ("browsing", "home") => Value::from(old.home.as_str()),
        ("browsing", "https_only") => Value::from(old.https_only),
        ("shard", "observers") => strings(&old.shard.observers),
        ("shard", "validator") => Value::from(old.shard.validator.as_str()),
        ("shard", "shard_id") => Value::from(old.shard.shard_id.as_str()),
        ("shard", "quorum") => unsigned(old.shard.quorum as u64),
        ("shard", "phlo_price") => Value::from(old.shard.phlo_price),
        ("content", "mirrors") => strings(&old.mirrors),
        ("content", "cache_bytes") => unsigned(old.cache_bytes),
        ("site_data", "store_quota") => unsigned(old.store_quota),
        ("wallet", "embers_api") => Value::from(old.embers_api.as_deref().unwrap_or_default()),
        ("wallet", "max_fee") => Value::from(old.max_fee),
        ("appearance", "restore_sidebar") => Value::from(old.restore_sidebar),
        _ => return None,
    })
}

/// What becomes of the old theme (`workspace.json`'s `theme`; design
/// decision 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeFold {
    /// There was no `workspace.json`: nothing to fold.
    Unchanged,
    /// Dark, the old default, or a theme that cannot be used: the new
    /// default, which follows the system. `was` is the old theme, if it
    /// could be read.
    System { was: Option<String> },
    /// `theme = "default-light"`.
    Light,
    /// `theme = "custom"`, the old `palette.css` moved to
    /// `themes/custom.css`.
    Custom,
}

/// A converted `settings.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversion {
    pub text: String,
    /// The settings written (`table.name`), and whether each value can be
    /// used. Names only: never values.
    pub written: Vec<(String, bool)>,
}

/// `settings.toml` for the old `conf` (none when it could not be read) and
/// the old theme: the template, with only the settings `conf` sets, at the
/// value the old parser gave them, and the theme when it is not the
/// default. A file that sets nothing converts to the template byte for
/// byte.
pub fn convert(conf: Option<&str>, theme: &ThemeFold) -> Result<Conversion, String> {
    let file = Path::new("settings.toml");
    let old = conf.map(LegacySettings::parse);
    let set = conf.map(keys_set).unwrap_or_default();
    let mut text = settings::TEMPLATE.to_string();
    let mut written = Vec::with_capacity(set.len() + 1);
    for k in &settings::KEYS {
        let value = match (k.table, k.name, &old) {
            ("appearance", "theme", _) => match theme {
                ThemeFold::Light => Value::from("default-light"),
                ThemeFold::Custom => Value::from("custom"),
                ThemeFold::Unchanged | ThemeFold::System { .. } => continue,
            },
            (_, _, Some(old)) if set.iter().any(|s| s.table == k.table && s.name == k.name) => {
                match legacy_value(k, old) {
                    Some(value) => value,
                    None => continue,
                }
            }
            _ => continue,
        };
        text = settings::edit_setting(file, &text, k.table, k.name, value).map_err(|e| e.to_string())?;
        written.push((k.path(), true));
    }
    let mut applied = settings::Settings::default();
    let mut diagnostics = Vec::new();
    settings::apply(&mut applied, file, &text, &mut diagnostics).map_err(|d| d.to_string())?;
    for (path, usable) in &mut written {
        *usable = !diagnostics.iter().any(|d| d.kind == DiagnosticKind::Value && d.key == *path);
    }
    Ok(Conversion { text, written })
}

/// Older versions' templates, which tests convert: each must become the
/// new template byte for byte.
#[cfg(test)]
pub(crate) mod old_templates {
    /// The template of commit `7853bc2` ("full implementation", 298 bytes).
    pub const TEMPLATE_7853BC2: &str = "# F1R3Gaze settings. Lists are comma-separated.
# home = gaze://newtab
# observers = https://observer-1.example, https://observer-2.example, https://observer-3.example
# validator = https://validator.example
# shard_id = root
# quorum = 2
# mirrors = https://cdn.example/blob/
# https_only = false
";

    /// The template of commit `f6ee26a` ("wallet added", 433 bytes): the one
    /// in the profile this machine's F1R3Gaze made.
    pub const TEMPLATE_F6EE26A: &str = "# F1R3Gaze settings. Lists are comma-separated.
# home = gaze://newtab
# observers = https://observer-1.example, https://observer-2.example, https://observer-3.example
# validator = https://validator.example
# shard_id = root
# quorum = 2
# mirrors = https://cdn.example/blob/
# https_only = false
# Wallet balances, history and transfers (the Embers service F1R3Sky uses):
# embers_api = https://embers.example
# max_fee = 10000000
";
}

#[cfg(test)]
mod tests;
