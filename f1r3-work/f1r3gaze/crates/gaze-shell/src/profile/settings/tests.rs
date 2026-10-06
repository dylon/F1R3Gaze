use super::*;
use crate::theme::Scheme;
use gaze_fs::MemFs;

const USER: &str = "/c/settings.toml";

fn user() -> &'static Path {
    Path::new(USER)
}

/// Applies one file over the defaults.
fn read(text: &str) -> (Settings, Vec<Diagnostic>) {
    let mut settings = Settings::default();
    let mut diagnostics = Vec::new();
    apply(&mut settings, user(), text, &mut diagnostics).expect("valid TOML");
    (settings, diagnostics)
}

/// The template with every example line made a setting.
fn uncommented_template() -> String {
    TEMPLATE
        .lines()
        .map(|line| line.strip_prefix("#   ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn edit(text: &str, table: &str, name: &str, value: impl Into<Value>) -> String {
    edit_setting(user(), text, table, name, value.into()).expect("edit")
}

#[test]
fn the_template_documents_every_setting_and_sets_none() {
    let (settings, diagnostics) = read(TEMPLATE);
    assert_eq!(settings, Settings::default());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // Every table once, in order; every setting once, as an example line
    // below its own table's header, with documentation above it.
    let headers: Vec<&str> = TEMPLATE.lines().filter(|l| l.starts_with('[')).collect();
    let wanted: Vec<String> = TABLES.iter().map(|t| format!("[{t}]")).collect();
    assert_eq!(headers, wanted);
    let lines: Vec<&str> = TEMPLATE.lines().collect();
    for k in &KEYS {
        let example = format!("#   {} = ", k.name);
        let at: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].starts_with(&example)).collect();
        assert_eq!(at.len(), 1, "{} appears {} times", k.path(), at.len());
        let header = lines[..at[0]].iter().rev().find(|l| l.starts_with('[')).expect("under a table");
        assert_eq!(*header, format!("[{}]", k.table), "{} is under the wrong table", k.path());
        let above = lines[at[0] - 1];
        assert!(
            above.starts_with("# ") && !above.starts_with("#   "),
            "{} has no documentation above it",
            k.path()
        );
    }
    let examples = lines.iter().filter(|l| l.starts_with("#   ")).count();
    assert_eq!(examples, KEYS.len(), "every example line is a setting");
}

#[test]
fn uncommenting_the_template_gives_the_defaults() {
    let (settings, diagnostics) = read(&uncommented_template());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(settings, Settings::default());
}

#[test]
fn every_setting_is_read() {
    let text = r#"
[appearance]
theme = "nord"
restore_sidebar = true
[browsing]
home = "f1r3://example/"
https_only = true
[shard]
observers = ["https://o1.example", "http://o2.example:40453"]
validator = "https://v.example"
shard_id = "test"
quorum = 3
phlo_price = 0x10
[wallet]
embers_api = "https://embers.example"
max_fee = 1_000
[content]
mirrors = ["https://cdn.example/blob/"]
cache_bytes = 0
[site_data]
store_quota = 0o777
"#;
    let (s, diagnostics) = read(text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(s.theme, ThemeChoice::Named("nord".into()));
    assert!(s.restore_sidebar && s.https_only);
    assert_eq!(s.home, "f1r3://example/");
    assert_eq!(s.shard.observers, ["https://o1.example", "http://o2.example:40453"]);
    assert_eq!(s.shard.validator, "https://v.example");
    assert_eq!(s.shard.shard_id, "test");
    assert_eq!(s.shard.quorum, 3);
    assert_eq!(s.shard.phlo_price, 16);
    assert_eq!(s.embers_api.as_deref(), Some("https://embers.example"));
    assert_eq!(s.max_fee, 1_000);
    assert_eq!(s.mirrors, ["https://cdn.example/blob/"]);
    assert_eq!(s.cache_bytes, 0);
    assert_eq!(s.store_quota, 0o777);
    assert_eq!(s.shard.user, ShardConfig::default().user, "the user id is not a setting");
}

#[test]
fn a_bad_value_falls_back_alone_at_its_line_and_column() {
    let text = "[appearance]\ntheme = \"neon/blue\"\nrestore_sidebar = true\n";
    let mut settings = Settings::default();
    let mut diagnostics = Vec::new();
    apply(&mut settings, user(), text, &mut diagnostics).expect("valid TOML");
    assert!(settings.restore_sidebar, "the good setting next to it is used");
    assert_eq!(settings.theme, ThemeChoice::System);
    assert_eq!(diagnostics.len(), 1);
    let d = &diagnostics[0];
    assert_eq!((d.line, d.column, d.kind), (2, 9, DiagnosticKind::Value));
    assert_eq!(d.key, "appearance.theme");
    assert_eq!(
        d.to_string(),
        "/c/settings.toml:2:9: appearance.theme: \"neon/blue\" may hold only letters, digits, '.', '_' and '-'"
    );
}

#[test]
fn values_are_checked_by_type_and_range() {
    // (setting line, column of the value, words the problem must contain)
    let cases: [(&str, &str, usize, &str); 16] = [
        ("appearance", "theme = 7", 9, "expected a string, found an integer"),
        ("appearance", "restore_sidebar = \"yes\"", 19, "expected true or false, found a string"),
        ("browsing", "home = \"example.com\"", 8, "is not an address"),
        ("browsing", "home = \"javascript:alert(1)\"", 8, "is a javascript: address"),
        ("browsing", "https_only = 1", 14, "expected true or false, found an integer"),
        ("shard", "observers = \"https://o.example\"", 13, "expected an array of addresses"),
        ("shard", "observers = [\"https://o.example\", \"ftp://x.example\"]", 35, "is a ftp: address"),
        ("shard", "validator = \"file:///v\"", 13, "is a file: address"),
        ("shard", "shard_id = \"\"", 12, "cannot be empty"),
        ("shard", "quorum = 0", 10, "0 is less than 1"),
        ("shard", "phlo_price = 1.5", 14, "expected an integer, found a float"),
        ("wallet", "embers_api = \"gaze://x\"", 14, "is a gaze: address"),
        ("wallet", "max_fee = -1", 11, "-1 is less than 0"),
        ("content", "mirrors = [1]", 12, "expected a string, found an integer"),
        ("content", "cache_bytes = 1979-05-27", 15, "expected an integer, found a date-time"),
        ("site_data", "store_quota = 99999999999999999999", 15, "is too large"),
    ];
    for (table, line, column, words) in cases {
        let text = format!("[{table}]\n{line}\n");
        let (settings, diagnostics) = read(&text);
        assert_eq!(settings, Settings::default(), "{line}");
        assert_eq!(diagnostics.len(), 1, "{line}: {diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!((d.line, d.column, d.kind), (2, column, DiagnosticKind::Value), "{line}: {d}");
        assert!(d.problem.contains(words), "{line}: {d}");
    }
}

#[test]
fn columns_count_characters_not_bytes() {
    let (_, diagnostics) = read("[browsing]\n\"é\" = 1\nhome = 2\n");
    let columns: Vec<(usize, usize)> = diagnostics.iter().map(|d| (d.line, d.column)).collect();
    assert_eq!(columns, [(2, 1), (3, 8)]);
    let mut settings = Settings::default();
    let syntax = apply(&mut settings, user(), "[shard]\nshard_id = \"é\" x\n", &mut Vec::new()).expect_err("not TOML");
    assert_eq!((syntax.line, syntax.column), (2, 16));
}

#[test]
fn unknown_and_misplaced_settings_are_named_with_a_suggestion() {
    let text = r#"theme = "dark"
apperance = 1
theem = 2
zzz = 3
appearance = 4

[browsing]
hom = "gaze://newtab"
observers = []
whatever = true
"#;
    let mut settings = Settings::default();
    let mut diagnostics = Vec::new();
    apply(&mut settings, user(), text, &mut diagnostics).expect("valid TOML");
    assert_eq!(settings, Settings::default(), "nothing misplaced is used");
    let got: Vec<(usize, &str, DiagnosticKind, &str)> = diagnostics
        .iter()
        .map(|d| (d.line, d.key.as_str(), d.kind, d.problem.as_str()))
        .collect();
    use DiagnosticKind::{Misplaced, Unknown};
    assert_eq!(
        got,
        [
            (1, "theme", Misplaced, "settings go in tables: put this one under [appearance]; it is ignored here"),
            (2, "apperance", Unknown, "no such table; did you mean [appearance]? It is ignored"),
            (3, "theem", Unknown, "no such setting; did you mean \"theme\", under [appearance]? It is ignored"),
            (4, "zzz", Unknown, "no such table or setting; it is ignored"),
            (5, "appearance", Misplaced, "should be a table, written [appearance], not an integer; it is ignored"),
            (8, "browsing.hom", Unknown, "no such setting; did you mean \"home\"? It is ignored"),
            (9, "browsing.observers", Misplaced, "this setting belongs in [shard]; it is ignored here"),
            (10, "browsing.whatever", Unknown, "no such setting; it is ignored"),
        ]
    );
}

#[test]
fn layers_apply_lowest_first_and_the_users_file_last() {
    let fs = MemFs::new();
    // $XDG_CONFIG_DIRS order: the first directory is the most important.
    let site = PathBuf::from("/etc/xdg/site/f1r3fly-io/f1r3gaze/settings.toml");
    let vendor = PathBuf::from("/etc/xdg/f1r3fly-io/f1r3gaze/settings.toml");
    fs.seed_file(
        &vendor,
        b"[appearance]\ntheme = \"light\"\n[browsing]\nhome = \"https://vendor.example\"\n[shard]\nobservers = [\"https://a.example\", \"https://b.example\"]\n[wallet]\nembers_api = \"https://embers.example\"\n",
    );
    fs.seed_file(&site, b"[appearance]\ntheme = \"dark\"\n");
    fs.seed_file(
        USER,
        b"[browsing]\nhome = \"https://mine.example\"\n[shard]\nobservers = [\"https://c.example\"]\n[wallet]\nembers_api = \"\"\n",
    );
    let loaded = load(&fs, &[site, vendor], user());
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    assert_eq!(loaded.user, UserFile::Valid);
    let s = &loaded.settings;
    assert_eq!(s.theme, ThemeChoice::BuiltIn(Scheme::Dark), "the more important system file wins");
    assert_eq!(s.home, "https://mine.example", "the user's file wins");
    assert_eq!(s.shard.observers, ["https://c.example"], "arrays replace, never merge");
    assert_eq!(s.embers_api, None, "\"\" unsets a lower layer's service");
}

#[test]
fn an_unusable_value_keeps_the_layer_below_and_says_which() {
    let fs = MemFs::new();
    let system = PathBuf::from("/etc/xdg/f1r3fly-io/f1r3gaze/settings.toml");
    fs.seed_file(&system, b"[shard]\nquorum = 3\n");
    fs.seed_file(USER, b"[shard]\nquorum = -2\n");
    let loaded = load(&fs, std::slice::from_ref(&system), user());
    assert_eq!(loaded.settings.shard.quorum, 3);
    assert_eq!(loaded.diagnostics.len(), 1);
    assert_eq!(
        loaded.diagnostics[0].to_string(),
        "/c/settings.toml:2:10: shard.quorum: -2 is less than 1; using 3"
    );
}

#[test]
fn a_broken_system_file_is_ignored_and_reported() {
    let fs = MemFs::new();
    let broken = PathBuf::from("/etc/xdg/f1r3fly-io/f1r3gaze/settings.toml");
    let binary = PathBuf::from("/usr/xdg/f1r3fly-io/f1r3gaze/settings.toml");
    fs.seed_file(&broken, b"[shard]\nquorum = 3\n[shard\n");
    fs.seed_file(&binary, b"[shard]\nquorum = 4\n\xff\n");
    fs.seed_file(USER, b"[browsing]\nhttps_only = true\n");
    let loaded = load(&fs, &[broken.clone(), binary.clone()], user());
    assert_eq!(loaded.settings.shard.quorum, 2, "neither file is used, not even in part");
    assert!(loaded.settings.https_only);
    let got: Vec<(&Path, usize, usize, DiagnosticKind)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.file.as_path(), d.line, d.column, d.kind))
        .collect();
    assert_eq!(
        got,
        [
            (binary.as_path(), 3, 1, DiagnosticKind::NotUtf8),
            (broken.as_path(), 3, 7, DiagnosticKind::Syntax),
        ]
    );
}

#[test]
fn the_users_file_is_reported_missing_corrupt_or_unreadable() {
    let fs = MemFs::new();
    assert_eq!(load(&fs, &[], user()).user, UserFile::Missing);

    fs.seed_file(USER, b"[appearance]\ntheme = \"light\"\n[appearance]\n");
    let loaded = load(&fs, &[], user());
    assert_eq!(loaded.settings, Settings::default(), "none of a corrupt file is used");
    match loaded.user {
        UserFile::Corrupt(d) => {
            assert_eq!((d.line, d.kind), (3, DiagnosticKind::Syntax), "{d}");
            assert_eq!(check(user(), b"[appearance]\ntheme = \"light\"\n[appearance]\n"), Err(d));
        }
        other => panic!("{other:?}"),
    }

    fs.seed_file(USER, b"[browsing]\nhome = \"\xc3\x28\"\n");
    match load(&fs, &[], user()).user {
        UserFile::Corrupt(d) => assert_eq!((d.line, d.column, d.kind), (2, 9, DiagnosticKind::NotUtf8)),
        other => panic!("{other:?}"),
    }

    let fs = MemFs::new();
    fs.seed_dir(USER);
    match load(&fs, &[], user()).user {
        UserFile::Unreadable(d) => assert_eq!(d.kind, DiagnosticKind::Unreadable),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_byte_order_mark_and_crlf_line_endings_are_read() {
    let (settings, diagnostics) = read("\u{feff}[appearance]\r\ntheme = \"light\"\r\n");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(settings.theme, ThemeChoice::BuiltIn(Scheme::Light));
    assert_eq!(check(user(), "\u{feff}[appearance]\r\n".as_bytes()), Ok(()));
    // The parser accepts the mark itself (S9, M9d); stripping it keeps the
    // columns of the first line counted as an editor shows them.
    let (_, diagnostics) = read("\u{feff}appearance = 3\n");
    assert_eq!((diagnostics[0].line, diagnostics[0].column), (1, 1), "{diagnostics:?}");
}

#[test]
fn deep_nesting_is_refused_not_a_stack_overflow() {
    let depth = 100_000;
    let text = format!("[content]\nmirrors = {}{}\n", "[".repeat(depth), "]".repeat(depth));
    let mut settings = Settings::default();
    let refused = apply(&mut settings, user(), &text, &mut Vec::new()).expect_err("too deep");
    assert_eq!(refused.kind, DiagnosticKind::Syntax);
    assert!(edit_setting(user(), &text, "appearance", "theme", Value::from("system")).is_err());
}

// Writing.

#[test]
fn choosing_a_theme_in_the_template_puts_it_first_in_its_table() {
    let out = edit(TEMPLATE, "appearance", "theme", "default-light");
    assert_eq!(out, TEMPLATE.replacen("[appearance]\n", "[appearance]\ntheme = \"default-light\"\n", 1));
    assert_eq!(read(&out).0.theme, ThemeChoice::BuiltIn(Scheme::Light));
}

#[test]
fn a_value_is_replaced_in_place_keeping_every_comment() {
    let text = "# mine\n[appearance]\n# chosen by hand\ntheme   =   \"dark\"   # was light\nrestore_sidebar = true\n\n[browsing] # the web\nhome = \"gaze://newtab\"\n";
    let out = edit(text, "appearance", "theme", "default-light");
    assert_eq!(out, text.replace("\"dark\"", "\"default-light\""));
}

#[test]
fn a_missing_setting_goes_after_the_others_in_its_table() {
    let text = "[appearance]\nrestore_sidebar = true\n# about browsing\n\n[browsing]\nhttps_only = true\n";
    let out = edit(text, "appearance", "theme", "nord");
    assert_eq!(
        out,
        "[appearance]\nrestore_sidebar = true\ntheme = \"nord\"\n# about browsing\n\n[browsing]\nhttps_only = true\n"
    );
}

#[test]
fn a_missing_table_is_appended_as_a_standard_table() {
    let text = "[browsing]\nhome = \"gaze://newtab\"\n";
    let out = edit(text, "appearance", "theme", "system");
    assert_eq!(out, "[browsing]\nhome = \"gaze://newtab\"\n\n[appearance]\ntheme = \"system\"\n");
    assert_eq!(edit("", "appearance", "theme", "system"), "[appearance]\ntheme = \"system\"\n");
}

#[test]
fn dotted_and_inline_tables_are_edited_where_they_are() {
    assert_eq!(
        edit("appearance.theme = \"dark\"\n", "appearance", "theme", "default-light"),
        "appearance.theme = \"default-light\"\n"
    );
    assert_eq!(
        edit("appearance = { theme = \"dark\" }\n", "appearance", "theme", "default-light"),
        "appearance = { theme = \"default-light\" }\n"
    );
    assert_eq!(
        edit("appearance = { restore_sidebar = true }\n", "appearance", "theme", "system"),
        "appearance = { restore_sidebar = true, theme = \"system\" }\n"
    );
    assert_eq!(
        edit("appearance = {restore_sidebar = true}\n", "appearance", "theme", "system"),
        "appearance = {restore_sidebar = true, theme = \"system\"}\n"
    );
    assert_eq!(edit("appearance = {}\n", "appearance", "theme", "system"), "appearance = { theme = \"system\" }\n");
}

#[test]
fn a_table_known_only_through_its_subtable_gets_its_own_header() {
    let out = edit("[appearance.extra]\nx = 1\n", "appearance", "theme", "system");
    let (settings, diagnostics) = read(&out);
    assert_eq!(settings.theme, ThemeChoice::System);
    assert_eq!(diagnostics.len(), 1, "the unknown subtable is still reported: {out}");
    assert!(out.contains("[appearance.extra]\nx = 1\n"), "{out}");
    // Prediction (S9): the implicit [appearance] is written out with its
    // own header, before its subtable.
    assert_eq!(out, "[appearance]\ntheme = \"system\"\n[appearance.extra]\nx = 1\n");
}

#[test]
fn a_byte_order_mark_and_crlf_survive_an_edit() {
    let text = "\u{feff}[appearance]\r\nrestore_sidebar = true\r\n";
    assert_eq!(
        edit(text, "appearance", "theme", "system"),
        "\u{feff}[appearance]\r\nrestore_sidebar = true\r\ntheme = \"system\"\r\n"
    );
    // In a file that mixes the two, the header toml_edit writes out ends in
    // LF; no carriage return is added anywhere.
    let mixed = "[appearance]\r\nrestore_sidebar = true\n";
    assert_eq!(
        edit(mixed, "appearance", "theme", "system"),
        "[appearance]\nrestore_sidebar = true\ntheme = \"system\"\n"
    );
}

#[test]
fn edits_are_refused_rather_than_guessed() {
    let refused = [
        ("[appearance\ntheme = \"dark\"\n", "Invalid"),
        ("appearance = 3\n", "Shape"),
        ("[appearance.theme]\nx = 1\n", "Shape"),
        ("[[appearance]]\n", "Shape"),
    ];
    for (text, kind) in refused {
        let got = edit_setting(user(), text, "appearance", "theme", Value::from("system")).expect_err(text);
        let name = match got {
            WriteError::Invalid(_) => "Invalid",
            WriteError::Shape(_) => "Shape",
            WriteError::Unreadable(_) | WriteError::Write(_) => "io",
        };
        assert_eq!(name, kind, "{text}");
    }
}

#[test]
fn saving_a_theme_writes_only_when_it_changes() {
    let fs = MemFs::new();
    set_theme(&fs, user(), &ThemeChoice::BuiltIn(Scheme::Light)).expect("save");
    let first = fs.files()[user()].clone();
    assert_eq!(
        String::from_utf8(first.clone()).expect("UTF-8"),
        TEMPLATE.replacen("[appearance]\n", "[appearance]\ntheme = \"default-light\"\n", 1),
        "a missing file starts from the template"
    );
    assert_eq!(fs.mode(user()).expect("mode"), Some(gaze_fs::PRIVATE_FILE_MODE));
    fs.reset_ops();
    set_theme(&fs, user(), &ThemeChoice::BuiltIn(Scheme::Light)).expect("save again");
    assert_eq!(fs.ops(), 1, "only the read: nothing is written when nothing changes");
    set_theme(&fs, user(), &ThemeChoice::Named("nord".into())).expect("change");
    let loaded = load(&fs, &[], user());
    assert_eq!(loaded.settings.theme, ThemeChoice::Named("nord".into()));
    // A power cut after the save keeps it: the write is durable.
    fs.power_cut(|_| false);
    assert_eq!(load(&fs, &[], user()).settings.theme, ThemeChoice::Named("nord".into()));
}

#[test]
fn a_file_that_cannot_be_used_is_never_rewritten() {
    let fs = MemFs::new();
    fs.seed_file(USER, b"[appearance\n");
    assert!(matches!(
        set_theme(&fs, user(), &ThemeChoice::System),
        Err(WriteError::Invalid(_))
    ));
    fs.seed_file(USER, b"\xff\xfe");
    assert!(matches!(
        set_theme(&fs, user(), &ThemeChoice::System),
        Err(WriteError::Invalid(_))
    ));
    assert_eq!(fs.files()[user()], b"\xff\xfe");
}

#[cfg(unix)]
#[test]
fn saving_keeps_the_files_mode_and_its_symbolic_link() {
    use std::os::unix::fs::PermissionsExt;
    let dir = gaze_fs::scratch_dir("gaze-shell-settings-link");
    let target = dir.join("dotfiles/settings.toml");
    std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    std::fs::write(&target, "[appearance]\ntheme = \"dark\"\n").expect("seed");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let link = dir.join("config/settings.toml");
    std::fs::create_dir_all(link.parent().expect("parent")).expect("mkdir");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    set_theme(&gaze_fs::StdFs, &link, &ThemeChoice::BuiltIn(Scheme::Light)).expect("save");
    assert!(std::fs::symlink_metadata(&link).expect("lstat").file_type().is_symlink());
    assert_eq!(
        std::fs::read_to_string(&target).expect("read"),
        "[appearance]\ntheme = \"default-light\"\n"
    );
    let mode = std::fs::metadata(&target).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "the user's own mode is kept");
}
