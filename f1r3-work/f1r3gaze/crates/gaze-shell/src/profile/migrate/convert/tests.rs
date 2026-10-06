//! Tests of the conversion of `settings.conf` into `settings.toml` (ledger
//! S7): the old parser, the old templates, each key, the theme, and a
//! seeded property over 2,000 generated files whose oracle is the old
//! parser and a scheme and range checker of the test's own.

use super::*;
use crate::profile::settings::Diagnostic;
use crate::theme::ThemeChoice;

use old_templates::{TEMPLATE_7853BC2, TEMPLATE_F6EE26A};

#[test]
fn old_settings_parse() {
    // Moved with the parser from profile.rs (settings_parse).
    let s = LegacySettings::parse("observers = https://a, https://b\nquorum=3\n# home = x\nhttps_only = true");
    assert_eq!(s.shard.observers, vec!["https://a", "https://b"]);
    assert_eq!(s.shard.quorum, 3);
    assert_eq!(s.home, "gaze://newtab");
    assert!(s.https_only);
    assert!(!s.restore_sidebar, "the sidebar starts collapsed by default");
    assert!(LegacySettings::parse("restore_sidebar = true").restore_sidebar);
    assert!(!LegacySettings::parse("restore_sidebar = no").restore_sidebar);
}

#[test]
fn every_old_template_converts_byte_for_byte() {
    for (commit, template, bytes) in [
        ("7853bc2", TEMPLATE_7853BC2, 298),
        ("f6ee26a", TEMPLATE_F6EE26A, 433),
        ("e7ff471", LEGACY_TEMPLATE, 528),
    ] {
        assert_eq!(template.len(), bytes, "{commit}'s template");
        assert!(keys_set(template).is_empty(), "{commit}'s template sets nothing");
        for theme in [ThemeFold::Unchanged, ThemeFold::System { was: Some("dark".into()) }] {
            let converted = convert(Some(template), &theme).expect("converts");
            assert_eq!(converted.text, settings::TEMPLATE, "{commit}, {theme:?}");
            assert!(converted.written.is_empty(), "{commit}, {theme:?}");
        }
    }
    // Nothing to convert at all is the template too.
    assert_eq!(convert(None, &ThemeFold::Unchanged).expect("converts").text, settings::TEMPLATE);
}

/// The settings `text` gives, with its diagnostics.
fn read(text: &str) -> (settings::Settings, Vec<Diagnostic>) {
    let mut applied = settings::Settings::default();
    let mut diagnostics = Vec::new();
    settings::apply(&mut applied, Path::new("settings.toml"), text, &mut diagnostics).expect("valid TOML");
    (applied, diagnostics)
}

#[test]
fn conversion_keeps_exactly_what_was_set() {
    let conf = "quorum = 3\nobservers = https://a.example, https://b.example\n# home = https://never.example\nmax_fee = 5\nhttps_only = true\nembers_api = https://embers.example\n";
    let converted = convert(Some(conf), &ThemeFold::Light).expect("converts");
    let (applied, diagnostics) = read(&converted.text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let expected = settings::Settings {
        theme: ThemeChoice::BuiltIn(crate::theme::Scheme::Light),
        https_only: true,
        embers_api: Some("https://embers.example".into()),
        max_fee: 5,
        shard: ShardConfig {
            quorum: 3,
            observers: vec!["https://a.example".into(), "https://b.example".into()],
            ..ShardConfig::default()
        },
        ..settings::Settings::default()
    };
    assert_eq!(applied, expected);
    let names: Vec<&str> = converted.written.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["appearance.theme", "browsing.https_only", "shard.observers", "shard.quorum", "wallet.embers_api", "wallet.max_fee"],
        "in the template's order"
    );
    assert!(converted.written.iter().all(|(_, usable)| *usable));
    // Each setting is placed just after its table's header, and the rest
    // of the template is kept.
    assert!(converted.text.contains("[shard]\nobservers = [\"https://a.example\", \"https://b.example\"]\nquorum = 3\n"), "{}", converted.text);
    assert!(converted.text.contains("[appearance]\ntheme = \"default-light\"\n"), "{}", converted.text);
    for line in settings::TEMPLATE.lines() {
        assert!(converted.text.contains(line), "the template's line {line:?} is kept");
    }
}

#[test]
fn values_the_new_file_refuses_are_written_and_named_unusable() {
    let conf = "quorum = 0\nhome = example.com\ncache_bytes = 18446744073709551615\nmax_fee = 7\n";
    let converted = convert(Some(conf), &ThemeFold::Unchanged).expect("converts");
    let (applied, diagnostics) = read(&converted.text);
    let usable: Vec<(&str, bool)> = converted.written.iter().map(|(n, u)| (n.as_str(), *u)).collect();
    assert_eq!(
        usable,
        [("browsing.home", false), ("shard.quorum", false), ("wallet.max_fee", true), ("content.cache_bytes", false)]
    );
    let refused: Vec<&str> = diagnostics.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(refused, ["browsing.home", "shard.quorum", "content.cache_bytes"]);
    let defaults = settings::Settings::default();
    assert_eq!((applied.shard.quorum, applied.home.as_str(), applied.cache_bytes), (defaults.shard.quorum, defaults.home.as_str(), defaults.cache_bytes));
    assert_eq!(applied.max_fee, 7);
    // A number too large for TOML is written as its digits, so the file
    // stays valid TOML.
    assert!(converted.text.contains("cache_bytes = \"18446744073709551615\""), "{}", converted.text);
}

#[test]
fn the_theme_is_written_only_when_it_is_not_the_default() {
    for (theme, line) in [
        (ThemeFold::Light, Some("theme = \"default-light\"")),
        (ThemeFold::Custom, Some("theme = \"custom\"")),
        (ThemeFold::System { was: Some("dark".into()) }, None),
        (ThemeFold::System { was: None }, None),
        (ThemeFold::Unchanged, None),
    ] {
        let converted = convert(None, &theme).expect("converts");
        match line {
            Some(line) => assert!(converted.text.contains(&format!("[appearance]\n{line}\n")), "{theme:?}"),
            None => assert_eq!(converted.text, settings::TEMPLATE, "{theme:?}"),
        }
    }
}

// ── The property ─────────────────────────────────────────────────────────

/// SplitMix64 (Steele, Lea and Flood, OOPSLA 2014,
/// doi:10.1145/2660193.2660195): a small seeded generator, so a failing text
/// can be regenerated from its seed.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// Values for any key: good ones and bad ones of every kind.
const VALUES: [&str; 38] = [
    "",
    "0",
    "1",
    "2",
    "-1",
    "-5",
    "9223372036854775807",
    "9223372036854775808",
    "18446744073709551615",
    "18446744073709551616",
    "abc",
    "true",
    "false",
    "yes",
    "TRUE",
    "https://a.example",
    "http://b.example/blob/",
    "https://a.example, https://b.example",
    "https://a.example, , https://b.example,",
    "ftp://c.example",
    "example.com",
    "gaze://newtab",
    "f1r3://site.example",
    "file:///tmp/x",
    "https://a.example/?q=\"x\"",
    "a = b",
    "x # not a comment",
    "\"quoted\"",
    "back\\slash",
    "ünïcødé ✓",
    "tab\there",
    "control\u{1}char",
    "root",
    "  spaced out  ",
    "https://a.example,ftp://c.example",
    "3 4",
    "0x10",
    "1_000",
];

/// Names for any line: the old keys, near misses, and others.
const NAMES: [&str; 20] = [
    "home",
    "https_only",
    "observers",
    "validator",
    "shard_id",
    "quorum",
    "phlo_price",
    "mirrors",
    "cache_bytes",
    "store_quota",
    "embers_api",
    "max_fee",
    "restore_sidebar",
    "Quorum",
    "quorom",
    "theme",
    "user",
    "",
    "observers.extra",
    "home home",
];

/// One `settings.conf` generated from `rng`.
fn generate(rng: &mut SplitMix64) -> String {
    let crlf = rng.below(4) == 0;
    let lines = rng.below(14);
    let mut text = String::with_capacity(lines * 40);
    for _ in 0..lines {
        let line = match rng.below(10) {
            0 => format!("# {} = {}", rng.pick(&NAMES), rng.pick(&VALUES)),
            1 => format!("   # indented {}", rng.pick(&VALUES)),
            2 => String::new(),
            3 => format!("{} {}", rng.pick(&NAMES), rng.pick(&VALUES)),
            _ => {
                let spaces = [" = ", "=", " =", "= ", "  =   "][rng.below(5)];
                format!("{}{spaces}{}", rng.pick(&NAMES), rng.pick(&VALUES))
            }
        };
        text.push_str(&line);
        text.push_str(if crlf { "\r\n" } else { "\n" });
    }
    if rng.below(5) == 0 {
        text.pop();
        if crlf {
            text.pop();
        }
    }
    text
}

/// The old value of setting `k`, as `Key::show` writes a value: the
/// oracle, written out here, not taken from the conversion.
fn old_show(k: &Key, old: &LegacySettings) -> String {
    let quoted = |text: &str| Value::from(text).to_string();
    let list = |items: &[String]| Value::Array(items.iter().map(String::as_str).collect()).to_string();
    match (k.table, k.name) {
        ("browsing", "home") => quoted(&old.home),
        ("browsing", "https_only") => old.https_only.to_string(),
        ("shard", "observers") => list(&old.shard.observers),
        ("shard", "validator") => quoted(&old.shard.validator),
        ("shard", "shard_id") => quoted(&old.shard.shard_id),
        ("shard", "quorum") => old.shard.quorum.to_string(),
        ("shard", "phlo_price") => old.shard.phlo_price.to_string(),
        ("content", "mirrors") => list(&old.mirrors),
        ("content", "cache_bytes") => old.cache_bytes.to_string(),
        ("site_data", "store_quota") => old.store_quota.to_string(),
        ("wallet", "embers_api") => quoted(old.embers_api.as_deref().unwrap_or_default()),
        ("wallet", "max_fee") => old.max_fee.to_string(),
        ("appearance", "restore_sidebar") => old.restore_sidebar.to_string(),
        (table, name) => panic!("{table}.{name} has no old key"),
    }
}

/// Whether `text` is an address the new file accepts, with one of
/// `schemes`.
fn address(text: &str, schemes: &[&str]) -> bool {
    url::Url::parse(text).is_ok_and(|u| schemes.contains(&u.scheme()))
}

/// Whether the new file accepts the old value of `k`: the schema of
/// `settings.toml` (README section 6.1), checked here on its own.
fn acceptable(k: &Key, old: &LegacySettings) -> bool {
    const PAGE: [&str; 6] = ["gaze", "https", "http", "f1r3", "f1r3h", "file"];
    const SERVICE: [&str; 2] = ["https", "http"];
    let fits = |n: u64| i64::try_from(n).is_ok();
    match (k.table, k.name) {
        ("browsing", "home") => address(&old.home, &PAGE),
        ("shard", "observers") => old.shard.observers.iter().all(|o| address(o, &SERVICE)),
        ("shard", "validator") => address(&old.shard.validator, &SERVICE),
        ("shard", "shard_id") => !old.shard.shard_id.is_empty(),
        ("shard", "quorum") => old.shard.quorum >= 1 && fits(old.shard.quorum as u64),
        ("shard", "phlo_price") => old.shard.phlo_price >= 1,
        ("content", "mirrors") => old.mirrors.iter().all(|m| address(m, &SERVICE)),
        ("content", "cache_bytes") => fits(old.cache_bytes),
        ("site_data", "store_quota") => fits(old.store_quota),
        ("wallet", "embers_api") => old.embers_api.as_deref().is_none_or(|e| address(e, &SERVICE)),
        ("wallet", "max_fee") => old.max_fee >= 0,
        ("browsing", "https_only") | ("appearance", "restore_sidebar") => true,
        (table, name) => panic!("{table}.{name} has no old key"),
    }
}

/// Which settings `text` sets, decided here from the old parser's rules
/// (a trimmed line that is not a comment, with an `=`, whose trimmed key is
/// an old key), not by `keys_set`, which this test checks.
fn set_by(text: &str) -> Vec<(&'static str, &'static str)> {
    let mut set = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else { continue };
        if let Some((_, table, name)) = LEGACY_KEYS.iter().find(|(old, _, _)| *old == key.trim())
            && !set.contains(&(*table, *name))
        {
            set.push((*table, *name));
        }
    }
    set
}

#[test]
fn conversion_matches_the_old_parser() {
    let defaults = settings::Settings::default();
    let (mut accepted, mut refused) = (0usize, 0usize);
    for seed in 0..2_000u64 {
        let text = generate(&mut SplitMix64(seed));
        let what = format!("seed {seed}: {text:?}");
        let old = LegacySettings::parse(&text);
        let set = set_by(&text);
        let mut listed: Vec<(&str, &str)> = keys_set(&text).iter().map(|k| (k.table, k.name)).collect();
        let mut expected = set.clone();
        listed.sort_unstable();
        expected.sort_unstable();
        assert_eq!(listed, expected, "{seed}: keys_set");
        let converted = convert(Some(&text), &ThemeFold::Unchanged).unwrap_or_else(|e| panic!("{what}: {e}"));
        converted
            .text
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_else(|e| panic!("{what}: toml_edit cannot read the result: {e}"));
        let (applied, diagnostics) = read(&converted.text);
        if set.is_empty() {
            assert_eq!(converted.text, settings::TEMPLATE, "{what}");
        }
        for k in &settings::KEYS {
            let path = k.path();
            let about: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.key == path).collect();
            let written = converted.written.iter().find(|(name, _)| *name == path);
            match set.contains(&(k.table, k.name)) {
                false => {
                    assert_eq!(k.show(&applied), k.show(&defaults), "{what}: {path} was not set");
                    assert!(about.is_empty() && written.is_none(), "{what}: {path}");
                }
                true if acceptable(k, &old) => {
                    accepted += 1;
                    assert_eq!(k.show(&applied), old_show(k, &old), "{what}: {path}");
                    assert!(about.is_empty(), "{what}: {path}: {about:?}");
                    assert_eq!(written, Some(&(path.clone(), true)), "{what}");
                }
                true => {
                    refused += 1;
                    assert_eq!(about.len(), 1, "{what}: {path}: {about:?}");
                    assert_eq!(about[0].kind, DiagnosticKind::Value, "{what}: {path}");
                    assert_eq!(k.show(&applied), k.show(&defaults), "{what}: {path} keeps its default");
                    assert_eq!(written, Some(&(path.clone(), false)), "{what}");
                }
            }
        }
        // Nothing else is reported: no unknown or misplaced settings.
        assert!(diagnostics.iter().all(|d| d.kind == DiagnosticKind::Value), "{what}: {diagnostics:?}");
    }
    // The property is not vacuous: both outcomes occur, many times.
    assert!(accepted > 1_000 && refused > 1_000, "accepted {accepted}, refused {refused}");
}
