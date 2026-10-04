//! Pure presentation helpers for the chrome and the built-in pages: sizes,
//! relative times, address schemes, site names, capability descriptions,
//! console levels, and load-failure wording.
//!
//! Nothing here touches a document or a font. Width-based truncation lives
//! in [`crate::text_fit`], because it measures text with the same engine
//! Blitz lays it out with.

use gaze_broker::{CLOCK, DEBUG, DOC, LOG, NAV, NET, RAND, SHARD, STORE, Site};
use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

const SECOND: u64 = 1;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;
const MONTH: u64 = 30 * DAY;

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A byte count for people: `0 B`, `1023 B`, `1.0 KB`, `12.4 KB`, `150 MB`.
/// Units are powers of 1024. One decimal is kept below 100 of a unit.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    match value < 100.0 {
        true => format!("{value:.1} {}", UNITS[unit]),
        false => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// Group the digits of a non-negative integer: `1250000` → `1,250,000`.
/// Anything that is not a run of ASCII digits is returned unchanged.
pub fn group_digits(number: &str) -> Cow<'_, str> {
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Cow::Borrowed(number);
    }
    let len = number.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in number.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    Cow::Owned(out)
}

/// How long ago `at` was, relative to `now` (both Unix seconds):
/// "just now", "5 min ago", "3 h ago", "2 d ago", "3 wk ago", or a UTC
/// date (`2026-01-31`) beyond 30 days. Future times read "just now".
pub fn relative_time(now: u64, at: u64) -> String {
    let ago = now.saturating_sub(at);
    match ago {
        0..45 => "just now".into(),
        45..HOUR => format!("{} min ago", (ago / MINUTE).max(1)),
        HOUR..DAY => format!("{} h ago", ago / HOUR),
        DAY..WEEK => format!("{} d ago", ago / DAY),
        WEEK..MONTH => format!("{} wk ago", ago / WEEK),
        _ => iso_date(at),
    }
}

/// The UTC calendar date of a Unix time, `YYYY-MM-DD`.
pub fn iso_date(secs: u64) -> String {
    let (year, month, day) = utc_date(secs);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The UTC civil date `(year, month, day)` of a Unix time.
///
/// Howard Hinnant's `civil_from_days`
/// (<http://howardhinnant.github.io/date_algorithms.html#civil_from_days>):
/// days are counted in 400-year eras of 146 097 days, and each year starts
/// on 1 March, so the leap day falls at the end of the year.
pub fn utc_date(secs: u64) -> (i64, u32, u32) {
    let days = (secs / DAY) as i64 + 719_468; // shift the epoch to 0000-03-01
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097); // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let shifted_month = (5 * day_of_year + 2) / 153; // [0, 11], March = 0
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32; // [1, 31]
    let month = match shifted_month {
        0..=9 => shifted_month + 3,
        _ => shifted_month - 9,
    } as u32; // [1, 12]
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// History buckets by elapsed time. Elapsed time needs no timezone, unlike
/// "today" or "yesterday".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Recency {
    LastHour,
    LastDay,
    LastWeek,
    LastMonth,
    Older,
}

impl Recency {
    pub fn of(now: u64, at: u64) -> Recency {
        match now.saturating_sub(at) {
            0..HOUR => Recency::LastHour,
            HOUR..DAY => Recency::LastDay,
            DAY..WEEK => Recency::LastWeek,
            WEEK..MONTH => Recency::LastMonth,
            _ => Recency::Older,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Recency::LastHour => "Last hour",
            Recency::LastDay => "Last 24 hours",
            Recency::LastWeek => "Last 7 days",
            Recency::LastMonth => "Last 30 days",
            Recency::Older => "Older",
        }
    }
}

/// How the visual tone of a message or badge is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Ok,
    Warn,
    Err,
}

impl Tone {
    /// The CSS class modifier (`notice info`, `tag ok`, …).
    pub fn class(self) -> &'static str {
        match self {
            Tone::Info => "info",
            Tone::Ok => "ok",
            Tone::Warn => "warn",
            Tone::Err => "err",
        }
    }

    /// The Font Awesome icon that accompanies the tone.
    pub fn icon(self) -> &'static str {
        match self {
            Tone::Info => "fa-circle-info",
            Tone::Ok => "fa-circle-check",
            Tone::Warn => "fa-triangle-exclamation",
            Tone::Err => "fa-circle-exclamation",
        }
    }
}

/// The kind of address shown in the address bar's identity badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeKind {
    /// `https://`: encrypted.
    Https,
    /// `http://`: not encrypted.
    Http,
    /// `f1r3://`: a site published on the shard, verified by the bridge.
    Shard,
    /// `f1r3h://`: content by hash, verified against the hash.
    Hash,
    /// `file://`: a local file.
    File,
    /// `gaze://`: a page built into the browser.
    BuiltIn,
    /// Anything else (`data:`, `about:`, unknown schemes).
    Other,
}

impl SchemeKind {
    pub fn of(url: &str) -> SchemeKind {
        let scheme = url
            .split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .unwrap_or_default();
        match scheme.as_str() {
            "https" => SchemeKind::Https,
            "http" => SchemeKind::Http,
            "f1r3" => SchemeKind::Shard,
            "f1r3h" => SchemeKind::Hash,
            "file" => SchemeKind::File,
            "gaze" => SchemeKind::BuiltIn,
            _ => SchemeKind::Other,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            SchemeKind::Https => "fa-lock",
            SchemeKind::Http => "fa-triangle-exclamation",
            SchemeKind::Shard => "fa-circle-nodes",
            SchemeKind::Hash => "fa-hashtag",
            SchemeKind::File => "fa-file",
            SchemeKind::BuiltIn => "fa-eye",
            SchemeKind::Other => "fa-globe",
        }
    }

    /// The badge's tone class: plain HTTP warns; verified shard content is
    /// accented.
    pub fn tone(self) -> &'static str {
        match self {
            SchemeKind::Http => "warn",
            SchemeKind::Shard | SchemeKind::Hash => "acc",
            _ => "",
        }
    }

    /// A visible label, only where the icon alone would understate it.
    pub fn label(self) -> Option<&'static str> {
        match self {
            SchemeKind::Http => Some("Not secure"),
            _ => None,
        }
    }

    /// What the badge's hover bubble says.
    pub fn description(self) -> &'static str {
        match self {
            SchemeKind::Https => "Secure connection (HTTPS)",
            SchemeKind::Http => "Plain HTTP: this connection is not encrypted",
            SchemeKind::Shard => "Shard site, resolved and verified through the F1R3FLY shard",
            SchemeKind::Hash => "Content by hash, verified against its hash",
            SchemeKind::File => "A file on this computer",
            SchemeKind::BuiltIn => "A page built into F1R3Gaze",
            SchemeKind::Other => "Address",
        }
    }
}

/// `head…tail` of a string by characters. Used for identifiers such as
/// hashes, whose ends are what people compare. Not for fitting a width.
pub fn abbreviate(text: &str, head: usize, tail: usize) -> Cow<'_, str> {
    let count = text.chars().count();
    if count <= head + tail + 1 {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(head + tail + 3);
    out.extend(text.chars().take(head));
    out.push('…');
    out.extend(text.chars().skip(count - tail));
    Cow::Owned(out)
}

/// A short, human name for a site or URL:
///
/// | input | label |
/// |---|---|
/// | `https://example.org:443` | `example.org` |
/// | `http://host:8080` | `host:8080` |
/// | `file:///…/notes.html` | `notes.html` |
/// | `f1r3://<publisher>/<project>` | `project · abcd…6789` |
/// | `f1r3h://blake2b-256/<hex>` | `Hash abcd…6789` |
/// | `gaze://about` | `Built-in` |
///
/// Unparseable input is returned as it is.
pub fn site_label(site_or_url: &str) -> String {
    let Some(site) = Site::of_url(site_or_url) else {
        return site_or_url.to_string();
    };
    let (scheme, rest) = site.as_str().split_once("://").unwrap_or(("", site.as_str()));
    match scheme {
        "https" | "http" => {
            let default_port = if scheme == "https" { ":443" } else { ":80" };
            rest.strip_suffix(default_port).unwrap_or(rest).to_string()
        }
        "file" => {
            let path = rest.trim_end_matches('/');
            match path.rsplit_once('/') {
                Some((_, name)) if !name.is_empty() => name.to_string(),
                _ => path.to_string(),
            }
        }
        "f1r3" => match rest.split_once('/') {
            Some((publisher, project)) => {
                format!("{project} · {}", abbreviate(publisher, 4, 4))
            }
            None => rest.to_string(),
        },
        "f1r3h" => {
            let hash = rest.rsplit('/').next().unwrap_or(rest);
            format!("Hash {}", abbreviate(hash, 4, 4))
        }
        "gaze" => "Built-in".to_string(),
        _ => site.as_str().to_string(),
    }
}

/// `text` with the default port dropped from every `https://…:443` and
/// `http://…:80` origin in it, the way browsers write origins.
///
/// Prompts name sites in their canonical form (`https://example.org:443`):
/// exact, but noisier than people expect, and the port adds nothing (the
/// scheme implies it). Other ports, other schemes, and trailing sentence
/// dots are left as they are.
pub fn without_default_ports(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("://") {
        let separator = search + found;
        let scheme_start = text[..separator]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .last()
            .map_or(separator, |(at, _)| at);
        let authority_start = separator + 3;
        let authority_end = text[authority_start..]
            .find(|c: char| {
                !(c.is_ascii_alphanumeric()
                    || matches!(c, '.' | '-' | '[' | ']' | ':' | '@' | '%' | '_' | '~'))
            })
            .map_or(text.len(), |at| authority_start + at);
        let authority = text[authority_start..authority_end].trim_end_matches('.');
        let default_port = match text[scheme_start..separator].to_ascii_lowercase().as_str() {
            "https" => Some(":443"),
            "http" => Some(":80"),
            _ => None,
        };
        if let Some(port) = default_port
            && authority.len() > port.len()
            && authority.ends_with(port)
        {
            let port_start = authority_start + authority.len() - port.len();
            if out.is_empty() {
                out.reserve(text.len());
            }
            out.push_str(&text[copied..port_start]);
            copied = port_start + port.len();
        }
        search = authority_end.max(authority_start);
    }
    match copied {
        0 => Cow::Borrowed(text),
        _ => {
            out.push_str(&text[copied..]);
            Cow::Owned(out)
        }
    }
}

/// What a capability lets a page do, for the Permissions panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityInfo {
    pub icon: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

pub fn capability_info(urn: &str) -> CapabilityInfo {
    let info = |icon, name, description| CapabilityInfo {
        icon,
        name,
        description,
    };
    match urn {
        DOC => info(
            "fa-file-lines",
            "Page content",
            "Read and change this page's own document.",
        ),
        LOG => info(
            "fa-terminal",
            "Console",
            "Write messages to the Console panel.",
        ),
        CLOCK => info("fa-clock", "Clock", "Read the time and set timers."),
        RAND => info(
            "fa-dice",
            "Randomness",
            "Draw random numbers, recorded in the replay log.",
        ),
        NET => info(
            "fa-globe",
            "Network",
            "Fetch from its own site. Other sites ask you first.",
        ),
        STORE => info(
            "fa-database",
            "Storage",
            "Keep data for this site on this computer, within its quota.",
        ),
        NAV => info(
            "fa-compass",
            "Navigation",
            "Open pages: its own site here, other sites in a new tab.",
        ),
        SHARD => info(
            "fa-circle-nodes",
            "F1R3FLY shard",
            "Read the shard. Deploys and sessions ask first; your wallet pays.",
        ),
        DEBUG => info(
            "fa-bug",
            "Debugging",
            "Inspect the executive (built-in pages only).",
        ),
        session if session.starts_with("rho:gaze:shard/session/") => info(
            "fa-link",
            "Shard session",
            "An open session with a shard contract.",
        ),
        _ => info("fa-puzzle-piece", "Other capability", "A capability this browser does not describe."),
    }
}

/// A console line's level, from the page's `log!(level, …)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    pub fn of(level: &str) -> LogLevel {
        match level.trim().to_ascii_lowercase().as_str() {
            "error" | "err" | "fatal" => LogLevel::Error,
            "warn" | "warning" => LogLevel::Warn,
            "debug" | "trace" => LogLevel::Debug,
            _ => LogLevel::Info,
        }
    }

    pub fn class(self) -> &'static str {
        match self {
            LogLevel::Error => "err",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }
}

/// A console line as people read it. The executive prints values in their
/// source form, so a lone string arrives quoted (`"store written"`); that is
/// shown unquoted and unescaped. Anything else (numbers, tuples, several
/// literals) is left exactly as printed.
pub fn console_text(raw: &str) -> Cow<'_, str> {
    let Some(inner) = raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
        return Cow::Borrowed(raw);
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other @ ('"' | '\\')) => out.push(other),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            // An unescaped quote means this was not a single literal.
            '"' => return Cow::Borrowed(raw),
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

/// The reason a load failed, without the URL echo that fetch errors carry
/// (`network error: <url>: Dns Failed …` → `network error: Dns Failed …`).
pub fn failure_reason<'a>(url: &str, why: &'a str) -> Cow<'a, str> {
    if url.is_empty() {
        return Cow::Borrowed(why);
    }
    let echo = format!("{url}: ");
    match why.find(&echo) {
        Some(at) => {
            let mut out = String::with_capacity(why.len() - echo.len());
            out.push_str(&why[..at]);
            out.push_str(&why[at + echo.len()..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(why),
    }
}

/// One sentence saying why a page could not be opened.
pub fn failure_summary(why: &str) -> &'static str {
    const SUMMARIES: [(&str, &str); 7] = [
        ("plain HTTP is disabled", "Plain HTTP is turned off in settings."),
        ("Dns Failed", "The server's address could not be found."),
        ("Connection refused", "The server refused the connection."),
        ("timed out", "The server took too long to answer."),
        ("HTTP 404", "Nothing exists at this address (HTTP 404)."),
        ("No such file or directory", "There is no file at this address."),
        ("integrity mismatch", "The page's program did not match its integrity hash, so it was refused."),
    ];
    SUMMARIES
        .iter()
        .find(|(needle, _)| why.contains(needle))
        .map(|(_, summary)| *summary)
        .unwrap_or("The page could not be loaded.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_human_sized() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(12_700), "12.4 KB");
        assert_eq!(human_bytes(150 * 1024 * 1024), "150 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits("1250000"), "1,250,000");
        assert_eq!(group_digits("999"), "999");
        assert_eq!(group_digits("1000"), "1,000");
        assert_eq!(group_digits(""), "");
        assert_eq!(group_digits("?"), "?");
        assert_eq!(group_digits("-5"), "-5");
    }

    #[test]
    fn relative_times_have_exact_boundaries() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 44), "just now");
        assert_eq!(relative_time(now, now - 45), "1 min ago");
        assert_eq!(relative_time(now, now - 5 * MINUTE), "5 min ago");
        assert_eq!(relative_time(now, now - HOUR), "1 h ago");
        assert_eq!(relative_time(now, now - HOUR + 1), "59 min ago");
        assert_eq!(relative_time(now, now - 23 * HOUR), "23 h ago");
        assert_eq!(relative_time(now, now - 2 * DAY), "2 d ago");
        assert_eq!(relative_time(now, now - 20 * DAY), "2 wk ago");
        assert_eq!(relative_time(now, now - 400 * DAY), iso_date(now - 400 * DAY));
        assert_eq!(relative_time(now, now + 30), "just now");
    }

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(utc_date(0), (1970, 1, 1));
        assert_eq!(utc_date(951_782_400), (2000, 2, 29));
        assert_eq!(utc_date(1_700_000_000), (2023, 11, 14));
        assert_eq!(utc_date(4_107_542_400), (2100, 3, 1));
        assert_eq!(iso_date(1_700_000_000), "2023-11-14");
    }

    #[test]
    fn recency_buckets_have_exact_boundaries() {
        let now = 10 * MONTH;
        assert_eq!(Recency::of(now, now), Recency::LastHour);
        assert_eq!(Recency::of(now, now - HOUR + 1), Recency::LastHour);
        assert_eq!(Recency::of(now, now - HOUR), Recency::LastDay);
        assert_eq!(Recency::of(now, now - DAY), Recency::LastWeek);
        assert_eq!(Recency::of(now, now - WEEK), Recency::LastMonth);
        assert_eq!(Recency::of(now, now - MONTH), Recency::Older);
        assert_eq!(Recency::of(now, now + 5), Recency::LastHour);
    }

    #[test]
    fn schemes_are_classified_case_insensitively() {
        assert_eq!(SchemeKind::of("HTTPS://example.org"), SchemeKind::Https);
        assert_eq!(SchemeKind::of("http://example.org"), SchemeKind::Http);
        assert_eq!(SchemeKind::of("f1r3://pub/proj"), SchemeKind::Shard);
        assert_eq!(SchemeKind::of("f1r3h://blake2b-256/ab"), SchemeKind::Hash);
        assert_eq!(SchemeKind::of("file:///tmp/a.html"), SchemeKind::File);
        assert_eq!(SchemeKind::of("gaze://newtab"), SchemeKind::BuiltIn);
        assert_eq!(SchemeKind::of("data:text/plain,hi"), SchemeKind::Other);
        assert_eq!(SchemeKind::Http.label(), Some("Not secure"));
        assert_eq!(SchemeKind::Https.label(), None);
    }

    #[test]
    fn sites_have_short_labels() {
        assert_eq!(site_label("https://example.org:443"), "example.org");
        assert_eq!(site_label("https://example.org/path?q=1"), "example.org");
        assert_eq!(site_label("http://host:8080/x"), "host:8080");
        assert_eq!(site_label("http://host/x"), "host");
        assert_eq!(site_label("file:///tmp/site/notes.html"), "notes.html");
        assert_eq!(
            site_label("f1r3://0123456789abcdef/project/index.html"),
            "project · 0123…cdef"
        );
        assert_eq!(
            site_label("f1r3h://blake2b-256/0123456789abcdef0123"),
            "Hash 0123…0123"
        );
        assert_eq!(site_label("gaze://about"), "Built-in");
        assert_eq!(site_label("not a url"), "not a url");
    }

    #[test]
    fn default_ports_are_dropped_from_origins_in_text() {
        let cases = [
            (
                "prompt.html wants to fetch from https://example.org:443",
                "prompt.html wants to fetch from https://example.org",
            ),
            (
                "http://a.example:80/x and https://b.example:8443 and https://c.example:4443",
                "http://a.example/x and https://b.example:8443 and https://c.example:4443",
            ),
            ("see https://[::1]:443/ now", "see https://[::1]/ now"),
            ("HTTPS://Example.org:443.", "HTTPS://Example.org."),
            ("→https://x.org:443, then http://y.org:80", "→https://x.org, then http://y.org"),
            // Not a default port, not a web scheme, or no port at all.
            ("https://host:1443", "https://host:1443"),
            ("http://host:443/", "http://host:443/"),
            ("f1r3://abc:443/x", "f1r3://abc:443/x"),
            ("https://:443", "https://:443"),
            ("no addresses here", "no addresses here"),
            ("ends with ://", "ends with ://"),
        ];
        for (text, expected) in cases {
            assert_eq!(without_default_ports(text), expected, "{text}");
        }
        assert!(matches!(without_default_ports("https://example.org/"), Cow::Borrowed(_)));
    }

    #[test]
    fn abbreviation_keeps_both_ends() {
        assert_eq!(abbreviate("0123456789", 3, 3), "012…789");
        assert_eq!(abbreviate("0123456", 3, 3), "0123456");
        assert_eq!(abbreviate("αβγδεζηθικ", 2, 2), "αβ…ικ");
    }

    #[test]
    fn every_broker_capability_is_described() {
        for urn in [DOC, LOG, CLOCK, RAND, NET, STORE, NAV, SHARD, DEBUG] {
            let info = capability_info(urn);
            assert!(info.icon.starts_with("fa-"), "{urn}");
            assert!(!info.name.is_empty() && !info.description.is_empty(), "{urn}");
            assert_ne!(info.name, "Other capability", "{urn}");
        }
        assert_eq!(
            capability_info("rho:gaze:shard/session/abc").name,
            "Shard session"
        );
        assert_eq!(capability_info("rho:unknown").name, "Other capability");
    }

    #[test]
    fn log_levels_are_recognised() {
        assert_eq!(LogLevel::of("ERROR"), LogLevel::Error);
        assert_eq!(LogLevel::of("warning"), LogLevel::Warn);
        assert_eq!(LogLevel::of("trace"), LogLevel::Debug);
        assert_eq!(LogLevel::of("info"), LogLevel::Info);
        assert_eq!(LogLevel::of("anything"), LogLevel::Info);
    }

    #[test]
    fn console_strings_are_unquoted_only_when_single_literals() {
        assert_eq!(console_text("\"store written\""), "store written");
        assert_eq!(console_text(r#""say \"hi\"""#), r#"say "hi""#);
        assert_eq!(console_text(r#""a\nb""#), "a\nb");
        assert_eq!(console_text("42"), "42");
        assert_eq!(console_text(r#""a" "b""#), r#""a" "b""#);
        assert_eq!(console_text("\""), "\"");
        assert_eq!(console_text("(\"ok\", 1)"), "(\"ok\", 1)");
    }

    #[test]
    fn failure_wording_drops_the_url_echo() {
        let url = "https://docs.example.invalid/x";
        let why = format!(
            "network error: {url}: Dns Failed: resolve dns name 'docs.example.invalid:443'"
        );
        assert_eq!(
            failure_reason(url, &why),
            "network error: Dns Failed: resolve dns name 'docs.example.invalid:443'"
        );
        assert_eq!(failure_reason(url, "other"), "other");
        assert_eq!(failure_summary(&why), "The server's address could not be found.");
        assert_eq!(failure_summary("https://a: HTTP 404"), "Nothing exists at this address (HTTP 404).");
        assert_eq!(failure_summary("something else"), "The page could not be loaded.");
    }
}
