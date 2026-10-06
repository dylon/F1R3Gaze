//! `settings.toml`: the user's settings, on top of system-wide defaults.
//!
//! The file is TOML (<https://toml.io>) with one table per area. Its
//! template, [`TEMPLATE`], lists every setting commented out at its default
//! under a few lines saying what it does; `settings.toml.example` beside it
//! is a copy start-up keeps fresh.
//!
//! # Reading
//!
//! [`load`] reads layers, lowest first: the built-in defaults, the
//! system-wide files (the most important last), then the user's file. Each
//! setting takes its value from the highest layer that sets it to something
//! usable. A value that cannot be used, an unknown setting, or a setting in
//! the wrong place gives a [`Diagnostic`] with its file, line and column, and
//! changes nothing else. A file that is not UTF-8 TOML is not used at all;
//! for the user's own, start-up keeps a backup and writes the template in
//! its place (profile/reconcile.rs).
//!
//! # Writing
//!
//! [`write_setting`] changes one value with `toml_edit`, so comments, blank
//! lines and the order of everything else survive. An existing value is
//! replaced in place, keeping the spaces around it and a comment after it; a
//! missing one is added after the other settings of its table, and a
//! missing table at the end of the file. A file that is not valid TOML is
//! never rewritten. The result is read back before it is written, and
//! written atomically ([`gaze_fs::write_atomic`]), through a symbolic link
//! to its target, keeping the file's mode.
//!
//! Both crates parse with the same `toml_parser`, so the reader and the
//! writer agree on what is valid TOML, and both bound the nesting they
//! accept, so no file can exhaust the stack.

use crate::theme::ThemeChoice;
use gaze_fs::{Fs, Perm};
use gaze_shard::ShardConfig;
use std::fmt;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use toml::Spanned;
use toml::de::{DeTable, DeValue};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

/// What `settings.toml` sets, after layering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `[appearance] theme`.
    pub theme: ThemeChoice,
    /// `[appearance] restore_sidebar`: reopen the sidebar at start when it
    /// was open as the last window closed. By default every window starts
    /// with it collapsed; the panel last shown is remembered either way.
    pub restore_sidebar: bool,
    /// `[browsing] home`: the page opened at start, in a new tab, and when
    /// the last tab closes.
    pub home: String,
    /// `[browsing] https_only`: refuse plain-HTTP top-level documents.
    pub https_only: bool,
    /// `[shard]`: `observers`, `validator`, `shard_id`, `quorum` and
    /// `phlo_price`. Its `user` is the profile's user id, not a setting.
    pub shard: ShardConfig,
    /// `[wallet] embers_api`: the Embers service for wallet balances,
    /// history and transfers (the one F1R3Sky uses). Deploys go to the node
    /// directly either way.
    pub embers_api: Option<String>,
    /// `[wallet] max_fee`: upper bound on a transfer contract's
    /// `phlo_price × phlo_limit`.
    pub max_fee: i64,
    /// `[content] mirrors`: blob mirrors tried for every hash, after a
    /// site's own.
    pub mirrors: Vec<String>,
    /// `[content] cache_bytes`: the content cache's size.
    pub cache_bytes: u64,
    /// `[site_data] store_quota`: the most each site may store.
    pub store_quota: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::System,
            restore_sidebar: false,
            home: "gaze://newtab".into(),
            https_only: false,
            shard: ShardConfig::default(),
            embers_api: None,
            max_fee: 10_000_000,
            mirrors: Vec::new(),
            cache_bytes: 512 * 1024 * 1024,
            store_quota: gaze_broker::DEFAULT_STORE_QUOTA,
        }
    }
}

/// The text of a new `settings.toml`, and of `settings.toml.example`. Each
/// setting's example line is `#   name = default`: deleting its first four
/// characters sets it to its default.
pub const TEMPLATE: &str = r##"# F1R3Gaze settings
#
# Every setting is listed below, commented out at its default, under a few
# lines saying what it does. To change one, delete the "#" and the three
# spaces before it, then edit the value. A setting F1R3Gaze cannot use is
# reported when it starts, and keeps its default.
#
# The format is TOML (https://toml.io). F1R3Gaze changes this file only when
# you choose a theme in Appearance, and keeps your comments when it does.
# settings.toml.example, next to this file, is always a fresh copy of it.

[appearance]
# The window's colours. "system" follows the operating system's light or
# dark preference, and is dark when there is none. "default-dark" and
# "default-light" are the built-in schemes. Any other name is a theme file
# in the themes folder next to this file: "nord" is themes/nord.css.
#   theme = "system"
#
# Reopen the sidebar at start if it was open when the last window closed.
# Either way, the panel shown last is remembered.
#   restore_sidebar = false

[browsing]
# The page F1R3Gaze opens at start, in a new tab, and when the last tab
# closes.
#   home = "gaze://newtab"
#
# Refuse pages served over plain HTTP.
#   https_only = false

[shard]
# Read-only observer nodes, run by distinct operators. An answer is
# confirmed when `quorum` of them agree.
#   observers = ["http://localhost:40453"]
#
# The validator node that deploys are sent to.
#   validator = "http://localhost:40403"
#
# The shard's id, part of every deploy.
#   shard_id = "root"
#
# How many observers must agree.
#   quorum = 2
#
# The phlo price offered for each deploy.
#   phlo_price = 1

[wallet]
# The Embers service for wallet balances, history and transfers (the one
# F1R3Sky uses); "" for none. Deploys go to the validator either way.
#   embers_api = ""
#
# The most a transfer may cost: its phlo price times its phlo limit.
#   max_fee = 10_000_000

[content]
# Blob mirrors tried for every hash, after a site's own, as URLs.
#   mirrors = []
#
# The size of the content cache, in bytes (512 MiB).
#   cache_bytes = 536_870_912

[site_data]
# The most each site may store, in bytes (10 MiB).
#   store_quota = 10_485_760
"##;

/// The tables, in the template's order.
pub const TABLES: [&str; 6] = ["appearance", "browsing", "shard", "wallet", "content", "site_data"];

/// The schemes a page's address may have.
const PAGE_SCHEMES: [&str; 6] = ["gaze", "https", "http", "f1r3", "f1r3h", "file"];

/// The schemes of a service's address: a node, a mirror, Embers.
const SERVICE_SCHEMES: [&str; 2] = ["https", "http"];

/// What is wrong with one value, and where it is in its file.
#[derive(Debug)]
struct Problem {
    span: Range<usize>,
    message: String,
}

impl Problem {
    fn new(span: Range<usize>, message: impl Into<String>) -> Problem {
        Problem {
            span,
            message: message.into(),
        }
    }
}

type Decode = fn(&Spanned<DeValue<'_>>, &mut Settings) -> Result<(), Problem>;

/// One setting: where it lives, how its value is read, and how the value in
/// use is shown.
pub struct Key {
    pub table: &'static str,
    pub name: &'static str,
    /// Sets the field from a value, or leaves it as it was and says why.
    decode: Decode,
    /// The value in use, as TOML.
    show: fn(&Settings) -> String,
}

impl Key {
    /// `table.name`.
    pub fn path(&self) -> String {
        format!("{}.{}", self.table, self.name)
    }

    /// The value `settings` uses, as TOML.
    pub fn show(&self, settings: &Settings) -> String {
        (self.show)(settings)
    }
}

/// Every setting, in the template's order.
pub static KEYS: [Key; 14] = [
    Key {
        table: "appearance",
        name: "theme",
        decode: |v, s| {
            s.theme = ThemeChoice::parse(string(v)?).map_err(|e| Problem::new(v.span(), e))?;
            Ok(())
        },
        show: |s| quoted(s.theme.as_str()),
    },
    Key {
        table: "appearance",
        name: "restore_sidebar",
        decode: |v, s| {
            s.restore_sidebar = boolean(v)?;
            Ok(())
        },
        show: |s| s.restore_sidebar.to_string(),
    },
    Key {
        table: "browsing",
        name: "home",
        decode: |v, s| {
            s.home = url(v, &PAGE_SCHEMES)?;
            Ok(())
        },
        show: |s| quoted(&s.home),
    },
    Key {
        table: "browsing",
        name: "https_only",
        decode: |v, s| {
            s.https_only = boolean(v)?;
            Ok(())
        },
        show: |s| s.https_only.to_string(),
    },
    Key {
        table: "shard",
        name: "observers",
        decode: |v, s| {
            s.shard.observers = urls(v, &SERVICE_SCHEMES)?;
            Ok(())
        },
        show: |s| quoted_list(&s.shard.observers),
    },
    Key {
        table: "shard",
        name: "validator",
        decode: |v, s| {
            s.shard.validator = url(v, &SERVICE_SCHEMES)?;
            Ok(())
        },
        show: |s| quoted(&s.shard.validator),
    },
    Key {
        table: "shard",
        name: "shard_id",
        decode: |v, s| {
            s.shard.shard_id = match string(v)? {
                "" => return Err(Problem::new(v.span(), "the shard id cannot be empty")),
                id => id.to_string(),
            };
            Ok(())
        },
        show: |s| quoted(&s.shard.shard_id),
    },
    Key {
        table: "shard",
        name: "quorum",
        decode: |v, s| {
            s.shard.quorum = usize::try_from(integer(v, 1)?)
                .map_err(|_| Problem::new(v.span(), "is too large for this machine"))?;
            Ok(())
        },
        show: |s| s.shard.quorum.to_string(),
    },
    Key {
        table: "shard",
        name: "phlo_price",
        decode: |v, s| {
            s.shard.phlo_price = integer(v, 1)?;
            Ok(())
        },
        show: |s| s.shard.phlo_price.to_string(),
    },
    Key {
        table: "wallet",
        name: "embers_api",
        decode: |v, s| {
            s.embers_api = match string(v)? {
                "" => None,
                _ => Some(url(v, &SERVICE_SCHEMES)?),
            };
            Ok(())
        },
        show: |s| quoted(s.embers_api.as_deref().unwrap_or_default()),
    },
    Key {
        table: "wallet",
        name: "max_fee",
        decode: |v, s| {
            s.max_fee = integer(v, 0)?;
            Ok(())
        },
        show: |s| s.max_fee.to_string(),
    },
    Key {
        table: "content",
        name: "mirrors",
        decode: |v, s| {
            s.mirrors = urls(v, &SERVICE_SCHEMES)?;
            Ok(())
        },
        show: |s| quoted_list(&s.mirrors),
    },
    Key {
        table: "content",
        name: "cache_bytes",
        decode: |v, s| {
            s.cache_bytes = integer(v, 0)?.unsigned_abs();
            Ok(())
        },
        show: |s| s.cache_bytes.to_string(),
    },
    Key {
        table: "site_data",
        name: "store_quota",
        decode: |v, s| {
            s.store_quota = integer(v, 0)?.unsigned_abs();
            Ok(())
        },
        show: |s| s.store_quota.to_string(),
    },
];

/// The setting at `table.name`.
pub fn key(table: &str, name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.table == table && k.name == name)
}

fn quoted(text: &str) -> String {
    Value::from(text).to_string()
}

fn quoted_list(items: &[String]) -> String {
    Value::Array(items.iter().map(String::as_str).collect()).to_string()
}

/// What a value is, for "expected …, found …".
fn kind_of(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a date-time",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

fn string<'v>(v: &'v Spanned<DeValue<'_>>) -> Result<&'v str, Problem> {
    match v.get_ref() {
        DeValue::String(text) => Ok(text),
        other => Err(Problem::new(v.span(), format!("expected a string, found {}", kind_of(other)))),
    }
}

fn boolean(v: &Spanned<DeValue<'_>>) -> Result<bool, Problem> {
    match v.get_ref() {
        DeValue::Boolean(b) => Ok(*b),
        other => Err(Problem::new(
            v.span(),
            format!("expected true or false, found {}", kind_of(other)),
        )),
    }
}

/// An integer of at least `min`.
fn integer(v: &Spanned<DeValue<'_>>, min: i64) -> Result<i64, Problem> {
    match v.get_ref() {
        DeValue::Integer(n) => match i64::from_str_radix(n.as_str(), n.radix()) {
            Ok(n) if n >= min => Ok(n),
            Ok(n) => Err(Problem::new(v.span(), format!("{n} is less than {min}"))),
            Err(_) => Err(Problem::new(v.span(), format!("{n} is too large"))),
        },
        other => Err(Problem::new(
            v.span(),
            format!("expected an integer, found {}", kind_of(other)),
        )),
    }
}

/// An address with one of `schemes`, kept as written.
fn url(v: &Spanned<DeValue<'_>>, schemes: &[&str]) -> Result<String, Problem> {
    let text = string(v)?;
    let expected = || {
        let names: Vec<String> = schemes.iter().map(|s| format!("{s}:")).collect();
        names.join(", ")
    };
    match url::Url::parse(text) {
        Ok(parsed) if schemes.contains(&parsed.scheme()) => Ok(text.to_string()),
        Ok(parsed) => Err(Problem::new(
            v.span(),
            format!("{text:?} is a {}: address; expected {}", parsed.scheme(), expected()),
        )),
        Err(e) => Err(Problem::new(
            v.span(),
            format!("{text:?} is not an address ({e}); write it whole, like \"https://example.com\""),
        )),
    }
}

/// An array of addresses with one of `schemes`.
fn urls(v: &Spanned<DeValue<'_>>, schemes: &[&str]) -> Result<Vec<String>, Problem> {
    match v.get_ref() {
        DeValue::Array(items) => items.iter().map(|item| url(item, schemes)).collect(),
        other => Err(Problem::new(
            v.span(),
            format!("expected an array of addresses, like [\"https://a.example\"], found {}", kind_of(other)),
        )),
    }
}

/// What a diagnostic is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// The file is not valid TOML; none of it is used.
    Syntax,
    /// The file is not UTF-8 text; none of it is used.
    NotUtf8,
    /// The file could not be read; none of it is used.
    Unreadable,
    /// A value cannot be used; the setting keeps the value from below.
    Value,
    /// A setting or a table F1R3Gaze does not know; it is ignored.
    Unknown,
    /// A known setting outside its table, or a table that is not a table;
    /// it is ignored.
    Misplaced,
}

/// A problem in a settings file, at a line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: PathBuf,
    /// 1-based; 0 when the problem is the whole file.
    pub line: usize,
    /// 1-based, in characters; 0 when the problem is the whole file.
    pub column: usize,
    /// `table.name`, a table, or empty for the whole file.
    pub key: String,
    pub kind: DiagnosticKind,
    pub problem: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if self.line > 0 {
            write!(f, ":{}:{}", self.line, self.column)?;
        }
        if !self.key.is_empty() {
            write!(f, ": {}", self.key)?;
        }
        write!(f, ": {}", self.problem)
    }
}

/// The line and column (1-based; the column in characters) of byte
/// `offset` of `text`.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    (before.matches('\n').count() + 1, before[line_start..].chars().count() + 1)
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn syntax_diagnostic(file: &Path, text: &str, message: &str, span: Option<Range<usize>>) -> Diagnostic {
    let (line, column) = span.map_or((0, 0), |span| position(text, span.start));
    Diagnostic {
        file: file.to_path_buf(),
        line,
        column,
        key: String::new(),
        kind: DiagnosticKind::Syntax,
        problem: format!("not valid TOML: {}; none of this file is used", message.trim()),
    }
}

fn not_utf8_diagnostic(file: &Path, bytes: &[u8], error: std::str::Utf8Error) -> Diagnostic {
    let valid = std::str::from_utf8(&bytes[..error.valid_up_to()])
        .expect("a UTF-8 error's valid_up_to ends a valid prefix");
    let (line, column) = position(valid, valid.len());
    Diagnostic {
        file: file.to_path_buf(),
        line,
        column,
        key: String::new(),
        kind: DiagnosticKind::NotUtf8,
        problem: "not UTF-8 text; none of this file is used".into(),
    }
}

/// The candidate nearest `word` by edit distance, when it is near enough
/// to be a slip of the keys: one edit for words of up to five characters,
/// two for longer ones. Swapping two neighbouring letters counts as one edit
/// (optimal string alignment), as it is one slip. Ties go to the first
/// candidate.
fn closest<'a>(word: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let limit = match word.chars().count() {
        0..=5 => 1,
        _ => 2,
    };
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        if let Some(distance) = liblevenshtein::distance::transposition_distance_bounded(word, candidate, limit)
            && best.is_none_or(|(nearest, _)| distance < nearest)
        {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Why `[table] name` is not a setting.
fn unknown_setting(table: &str, name: &str) -> (DiagnosticKind, String) {
    let in_table = KEYS.iter().filter(|k| k.table == table).map(|k| k.name);
    if let Some(near) = closest(name, in_table) {
        return (DiagnosticKind::Unknown, format!("no such setting; did you mean {near:?}? It is ignored"));
    }
    match KEYS.iter().find(|k| k.name == name) {
        Some(k) => (
            DiagnosticKind::Misplaced,
            format!("this setting belongs in [{}]; it is ignored here", k.table),
        ),
        None => (DiagnosticKind::Unknown, "no such setting; it is ignored".into()),
    }
}

/// Why `name`, at the top of the file, is neither a table nor a setting.
fn unknown_top(name: &str) -> (DiagnosticKind, String) {
    if let Some(k) = KEYS.iter().find(|k| k.name == name) {
        return (
            DiagnosticKind::Misplaced,
            format!("settings go in tables: put this one under [{}]; it is ignored here", k.table),
        );
    }
    if let Some(table) = closest(name, TABLES.iter().copied()) {
        return (DiagnosticKind::Unknown, format!("no such table; did you mean [{table}]? It is ignored"));
    }
    match closest(name, KEYS.iter().map(|k| k.name)).and_then(|near| KEYS.iter().find(|k| k.name == near)) {
        Some(k) => (
            DiagnosticKind::Unknown,
            format!("no such setting; did you mean {:?}, under [{}]? It is ignored", k.name, k.table),
        ),
        None => (DiagnosticKind::Unknown, "no such table or setting; it is ignored".into()),
    }
}

/// Applies the settings in `text`, the content of `file`, on top of
/// `settings`, and appends its diagnostics in file order. A setting whose
/// value cannot be used keeps the value it had. Err when `text` is not
/// valid TOML: then nothing is applied.
pub fn apply(
    settings: &mut Settings,
    file: &Path,
    text: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), Diagnostic> {
    let text = strip_bom(text);
    let root = DeTable::parse(text).map_err(|e| syntax_diagnostic(file, text, e.message(), e.span()))?;
    let at = |span: Range<usize>, kind: DiagnosticKind, key: String, problem: String| {
        let (line, column) = position(text, span.start);
        Diagnostic {
            file: file.to_path_buf(),
            line,
            column,
            key,
            kind,
            problem,
        }
    };
    let mut found = Vec::new();
    for (name, value) in root.get_ref() {
        let table_name: &str = name.get_ref();
        match (TABLES.contains(&table_name), value.get_ref()) {
            (true, DeValue::Table(table)) => {
                for (setting, value) in table {
                    let setting_name: &str = setting.get_ref();
                    let path = format!("{table_name}.{setting_name}");
                    match key(table_name, setting_name) {
                        Some(k) => {
                            if let Err(problem) = (k.decode)(value, settings) {
                                found.push(at(problem.span, DiagnosticKind::Value, path, problem.message));
                            }
                        }
                        None => {
                            let (kind, problem) = unknown_setting(table_name, setting_name);
                            found.push(at(setting.span(), kind, path, problem));
                        }
                    }
                }
            }
            (true, other) => found.push(at(
                name.span(),
                DiagnosticKind::Misplaced,
                table_name.to_string(),
                format!("should be a table, written [{table_name}], not {}; it is ignored", kind_of(other)),
            )),
            (false, _) => {
                let (kind, problem) = unknown_top(table_name);
                found.push(at(name.span(), kind, table_name.to_string(), problem));
            }
        }
    }
    found.sort_by_key(|d| (d.line, d.column));
    diagnostics.append(&mut found);
    Ok(())
}

/// Whether `bytes`, the content of `file`, can be used as a settings file:
/// UTF-8 and valid TOML. Settings it cannot use are not its fault: they
/// are diagnostics, not damage.
pub fn check(file: &Path, bytes: &[u8]) -> Result<(), Diagnostic> {
    let text = std::str::from_utf8(bytes).map_err(|e| not_utf8_diagnostic(file, bytes, e))?;
    let text = strip_bom(text);
    DeTable::parse(text)
        .map(drop)
        .map_err(|e| syntax_diagnostic(file, text, e.message(), e.span()))
}

/// The state of the user's settings file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserFile {
    /// There is none.
    Missing,
    /// It was read, and is valid TOML.
    Valid,
    /// It could not be read. It is left as it is, and settings cannot be
    /// saved this session.
    Unreadable(Diagnostic),
    /// It is not UTF-8 TOML. None of it is used; start-up keeps a backup and
    /// writes the template in its place.
    Corrupt(Diagnostic),
}

/// Settings as loaded, and what was wrong on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    /// Unusable values, unknown or misplaced settings, and system-wide
    /// files that could not be used, in layer order.
    pub diagnostics: Vec<Diagnostic>,
    pub user: UserFile,
}

enum Read {
    Missing,
    Text(String),
    Failed(Diagnostic),
}

fn read(fs: &dyn Fs, file: &Path) -> Read {
    match fs.read(file) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Read::Text(text),
            Err(e) => Read::Failed(not_utf8_diagnostic(file, e.as_bytes(), e.utf8_error())),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Missing,
        Err(e) => Read::Failed(Diagnostic {
            file: file.to_path_buf(),
            line: 0,
            column: 0,
            key: String::new(),
            kind: DiagnosticKind::Unreadable,
            problem: format!("cannot be read ({e}); none of this file is used"),
        }),
    }
}

/// Loads the settings: the built-in defaults, then the system-wide files
/// `system` (given most important first, as `$XDG_CONFIG_DIRS` lists them,
/// and so applied in reverse), then the user's file `user`.
pub fn load(fs: &dyn Fs, system: &[PathBuf], user: &Path) -> Loaded {
    let mut settings = Settings::default();
    let mut diagnostics = Vec::new();
    for file in system.iter().rev() {
        match read(fs, file) {
            Read::Missing => {}
            Read::Text(text) => {
                if let Err(syntax) = apply(&mut settings, file, &text, &mut diagnostics) {
                    diagnostics.push(syntax);
                }
            }
            Read::Failed(failure) => diagnostics.push(failure),
        }
    }
    let user = match read(fs, user) {
        Read::Missing => UserFile::Missing,
        Read::Text(text) => match apply(&mut settings, user, &text, &mut diagnostics) {
            Ok(()) => UserFile::Valid,
            Err(syntax) => UserFile::Corrupt(syntax),
        },
        Read::Failed(failure) => match failure.kind {
            DiagnosticKind::NotUtf8 => UserFile::Corrupt(failure),
            _ => UserFile::Unreadable(failure),
        },
    };
    // Say what each unusable value gave way to, now that every layer is in.
    for diagnostic in diagnostics.iter_mut().filter(|d| d.kind == DiagnosticKind::Value) {
        if let Some((table, name)) = diagnostic.key.split_once('.')
            && let Some(k) = key(table, name)
        {
            diagnostic.problem.push_str(&format!("; using {}", k.show(&settings)));
        }
    }
    Loaded {
        settings,
        diagnostics,
        user,
    }
}

/// Why a setting could not be saved. The file is left as it was.
#[derive(Debug)]
pub enum WriteError {
    /// The file could not be read.
    Unreadable(io::Error),
    /// The file is not UTF-8 TOML.
    Invalid(Diagnostic),
    /// The table or the setting is there, but holds something else.
    Shape(String),
    /// The new file could not be written.
    Write(io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Unreadable(e) => write!(f, "the settings file cannot be read: {e}"),
            WriteError::Invalid(diagnostic) => {
                write!(f, "the settings file is left as it is until it is fixed: {diagnostic}")
            }
            WriteError::Shape(why) => write!(f, "the settings file is left as it is: {why}"),
            WriteError::Write(e) => write!(f, "the settings file cannot be written: {e}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// `text` with every line ending that is a bare line feed made a carriage
/// return and line feed.
fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut previous = '\0';
    for c in text.chars() {
        if c == '\n' && previous != '\r' {
            out.push('\r');
        }
        out.push(c);
        previous = c;
    }
    out
}

/// A value as TOML, without the spaces or comment around it.
fn bare(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

/// Puts `value` where `old` is, keeping the spaces around it and a comment
/// after it. (`Table::insert` would replace the key too, and with it the
/// comment above it, so it is never used on a key that is there.)
fn replace_value(old: &mut Value, value: Value) {
    let decor = old.decor().clone();
    *old = value;
    *old.decor_mut() = decor;
}

/// Adds `name = value` at the end of an inline table. The space before its
/// closing brace belongs to its last value; it moves to the new last value,
/// so the comma follows the old one directly: `{ a = 1, b = 2 }`.
fn insert_inline(table: &mut InlineTable, name: &str, mut value: Value) {
    let closing = match table.iter_mut().last() {
        Some((_, last)) => {
            let space = last.decor().suffix().cloned();
            last.decor_mut().set_suffix("");
            space
        }
        None => None,
    };
    if let Some(space) = closing {
        value.decor_mut().set_suffix(space);
    }
    table.insert(name, value);
}

/// `text`, the content of `file`, with `[table] name` set to `value`; see
/// the module documentation for where it goes. A byte-order mark is kept,
/// and so are CRLF line endings when every line has one; in a file that
/// mixes CRLF and LF, the lines `toml_edit` writes out itself, such as table
/// headers, end in LF.
pub fn edit_setting(file: &Path, text: &str, table: &str, name: &str, value: Value) -> Result<String, WriteError> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let line_feeds = body.matches('\n').count();
    let crlf = line_feeds > 0 && body.matches("\r\n").count() == line_feeds;
    let mut doc: DocumentMut = body
        .parse()
        .map_err(|e: toml_edit::TomlError| WriteError::Invalid(syntax_diagnostic(file, body, e.message(), e.span())))?;
    let wanted = bare(&value);
    match doc.get_mut(table) {
        None => {
            let mut new_table = Table::new();
            new_table.insert(name, Item::Value(value));
            doc.insert(table, Item::Table(new_table));
        }
        Some(Item::Value(Value::InlineTable(inline))) => match inline.get_mut(name) {
            Some(old) => replace_value(old, value),
            None => insert_inline(inline, name, value),
        },
        Some(item) => {
            let Some(existing) = item.as_table_like_mut() else {
                return Err(WriteError::Shape(format!("{table} is not a table")));
            };
            match existing.get_mut(name) {
                Some(slot) => match slot.as_value_mut() {
                    Some(old) => replace_value(old, value),
                    None => return Err(WriteError::Shape(format!("{table}.{name} is a table, not a value"))),
                },
                None => {
                    existing.insert(name, Item::Value(value));
                }
            }
        }
    }
    let mut out = doc.to_string();
    if crlf {
        out = to_crlf(&out);
    }
    // Read the result back with both parsers before anything is written.
    let written: DocumentMut = out
        .parse()
        .map_err(|e: toml_edit::TomlError| WriteError::Invalid(syntax_diagnostic(file, &out, e.message(), e.span())))?;
    DeTable::parse(&out).map_err(|e| WriteError::Invalid(syntax_diagnostic(file, &out, e.message(), e.span())))?;
    let read_back = written
        .get(table)
        .and_then(|t| t.get(name))
        .and_then(Item::as_value)
        .map(bare);
    match read_back {
        Some(got) if got == wanted => Ok(format!("{bom}{out}")),
        got => Err(WriteError::Shape(format!(
            "{table}.{name} would read back as {got:?}, not {wanted}"
        ))),
    }
}

/// Sets `[table] name` to `value` in the settings file at `path` (from the
/// template when there is none), writing nothing when it is already so.
pub fn write_setting(fs: &dyn Fs, path: &Path, table: &str, name: &str, value: Value) -> Result<(), WriteError> {
    let (text, existed) = match fs.read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => (text, true),
            Err(e) => return Err(WriteError::Invalid(not_utf8_diagnostic(path, e.as_bytes(), e.utf8_error()))),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (TEMPLATE.to_string(), false),
        Err(e) => return Err(WriteError::Unreadable(e)),
    };
    let edited = edit_setting(path, &text, table, name, value)?;
    if existed && edited == text {
        return Ok(());
    }
    let dir = gaze_fs::parent(path).map_err(WriteError::Write)?;
    // Was `fs.create_dir_all(dir)`: a new folder's name was never synced, so
    // a power cut could take the folder, and the saved file in it (found
    // once MemFs dropped the entries of folders a cut loses; ledger S7).
    gaze_fs::create_dir_durably(fs, dir).map_err(WriteError::Write)?;
    gaze_fs::write_atomic(fs, path, edited.as_bytes(), Perm::Preserve).map_err(WriteError::Write)
}

/// Saves the theme choice as `[appearance] theme`.
pub fn set_theme(fs: &dyn Fs, path: &Path, choice: &ThemeChoice) -> Result<(), WriteError> {
    write_setting(fs, path, "appearance", "theme", Value::from(choice.as_str()))
}

#[cfg(test)]
mod tests;
