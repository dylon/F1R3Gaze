use super::*;

fn machine(home: Option<&str>, env: &[(&str, &str)]) -> Machine {
    Machine {
        home: home.map(PathBuf::from),
        env: env.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect(),
        roaming_app_data: None,
        local_app_data: None,
    }
}

fn ours(base: &str) -> PathBuf {
    Path::new(base).join(VENDOR).join(APP)
}

#[test]
fn xdg_defaults_follow_home() {
    let l = platform_layout(Platform::Xdg, &machine(Some("/home/u"), &[])).expect("a layout");
    assert_eq!(l.config, ours("/home/u/.config"));
    assert_eq!(l.data, ours("/home/u/.local/share"));
    assert_eq!(l.state, ours("/home/u/.local/state"));
    assert_eq!(l.cache, ours("/home/u/.cache"));
    assert_eq!(l.runtime, ours("/home/u/.local/state").join("runtime"));
    assert!(l.runtime_is_fallback);
    assert_eq!(l.system_config, [ours("/etc/xdg")]);
    assert_eq!(
        l.system_themes,
        [ours("/usr/local/share").join("themes"), ours("/usr/share").join("themes")]
    );
    assert_eq!(l.legacy, [Path::new("/home/u/.local/share/f1r3gaze")]);
    assert_eq!(l.kind, Kind::Platform(Platform::Xdg));
}

/// Each of the five base variables unset, empty, relative or absolute: 4⁵ =
/// 1 024 environments. A root follows its variable exactly when the
/// variable is an absolute path (XDG Base Directory 0.8).
#[test]
fn every_xdg_environment_maps_by_the_specification() {
    const VARS: [(&str, &str); 5] = [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
        ("XDG_CACHE_HOME", ".cache"),
        ("XDG_RUNTIME_DIR", ""),
    ];
    let mut checked = 0;
    for code in 0..4usize.pow(5) {
        let mut env = Vec::with_capacity(5);
        let mut values: [Option<String>; 5] = Default::default();
        for (i, (var, _)) in VARS.iter().enumerate() {
            let value = match (code / 4usize.pow(i as u32)) % 4 {
                0 => None,
                1 => Some(String::new()),
                2 => Some(format!("relative/{var}")),
                _ => Some(format!("/abs/{var}")),
            };
            if let Some(value) = &value {
                env.push((*var, value.clone()));
            }
            values[i] = value;
        }
        let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let l = platform_layout(Platform::Xdg, &machine(Some("/home/u"), &env_refs)).expect("a layout");
        let expect = |i: usize| -> PathBuf {
            match &values[i] {
                Some(v) if v.starts_with('/') => ours(v),
                _ => ours(&format!("/home/u/{}", VARS[i].1)),
            }
        };
        assert_eq!(l.config, expect(0), "{env:?}");
        assert_eq!(l.data, expect(1), "{env:?}");
        assert_eq!(l.state, expect(2), "{env:?}");
        assert_eq!(l.cache, expect(3), "{env:?}");
        match &values[4] {
            Some(v) if v.starts_with('/') => {
                assert_eq!((l.runtime.clone(), l.runtime_is_fallback), (ours(v), false), "{env:?}")
            }
            _ => assert_eq!(
                (l.runtime.clone(), l.runtime_is_fallback),
                (expect(2).join("runtime"), true),
                "{env:?}"
            ),
        }
        checked += 1;
    }
    assert_eq!(checked, 1024);
}

#[test]
fn xdg_lists_keep_their_order_and_drop_bad_entries() {
    let m = machine(
        Some("/home/u"),
        &[
            ("XDG_CONFIG_DIRS", "/home/u/.config/kdedefaults::relative:/etc/xdg"),
            ("XDG_DATA_DIRS", "relative:"),
        ],
    );
    let l = platform_layout(Platform::Xdg, &m).expect("a layout");
    assert_eq!(l.system_config, [ours("/home/u/.config/kdedefaults"), ours("/etc/xdg")]);
    assert_eq!(
        l.system_themes,
        [ours("/usr/local/share").join("themes"), ours("/usr/share").join("themes")],
        "an XDG list with no usable entry falls back to the default"
    );
}

#[test]
fn macos_uses_the_bundle_id_its_caches_and_tmpdir() {
    let m = machine(
        Some("/Users/u"),
        &[("TMPDIR", "/var/folders/xy/T/"), ("XDG_CONFIG_HOME", "/ignored")],
    );
    let l = platform_layout(Platform::MacOs, &m).expect("a layout");
    let support = Path::new("/Users/u/Library/Application Support/io.f1r3fly.f1r3gaze");
    assert_eq!(l.config, support.join("config"), "XDG variables do not apply on macOS");
    assert_eq!(l.data, support.join("data"));
    assert_eq!(l.state, support.join("state"));
    assert_eq!(l.cache, Path::new("/Users/u/Library/Caches/io.f1r3fly.f1r3gaze"));
    assert_eq!((l.runtime.clone(), l.runtime_is_fallback), (Path::new("/var/folders/xy/T/io.f1r3fly.f1r3gaze").to_path_buf(), false));
    assert_eq!(l.system_config, [Path::new("/Library/Application Support/io.f1r3fly.f1r3gaze/config")]);
    assert_eq!(l.system_themes, [Path::new("/Library/Application Support/io.f1r3fly.f1r3gaze/themes")]);
    assert_eq!(l.legacy, [Path::new("/Users/u/Library/Application Support/F1R3Gaze")]);
    // Without a usable $TMPDIR the lock lives under state.
    let l = platform_layout(Platform::MacOs, &machine(Some("/Users/u"), &[("TMPDIR", "tmp")])).expect("a layout");
    assert_eq!((l.runtime.clone(), l.runtime_is_fallback), (support.join("state/runtime"), true));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_container_home_is_distinct_from_an_unsandboxed_home() {
    assert!(is_macos_container_home(Path::new(
        "/Users/u/Library/Containers/io.f1r3fly.f1r3gaze/Data"
    )));
    assert!(!is_macos_container_home(Path::new("/Users/u")));
    assert!(!is_macos_container_home(Path::new("/tmp/isolated-test-home")));
}

#[test]
fn windows_prefers_known_folders_then_the_environment() {
    let roaming = r"C:\Users\u\AppData\Roaming";
    let local = r"C:\Users\u\AppData\Local";
    let mut m = machine(
        Some(r"C:\Users\u"),
        &[("APPDATA", r"D:\Redirected\Roaming"), ("LOCALAPPDATA", r"D:\Redirected\Local"), ("ProgramData", r"C:\ProgramData"), ("XDG_CONFIG_HOME", "/ignored")],
    );
    m.roaming_app_data = Some(PathBuf::from(roaming));
    m.local_app_data = Some(PathBuf::from(local));
    let l = platform_layout(Platform::Windows, &m).expect("a layout");
    assert_eq!(l.config, Path::new(roaming).join(VENDOR).join(APP));
    let ours_local = Path::new(local).join(VENDOR).join(APP);
    assert_eq!(l.data, ours_local.join("data"));
    assert_eq!(l.state, ours_local.join("state"));
    assert_eq!(l.cache, ours_local.join("cache"));
    assert_eq!(l.runtime, ours_local.join("runtime"));
    assert!(!l.runtime_is_fallback);
    let system = Path::new(r"C:\ProgramData").join(VENDOR).join(APP);
    assert_eq!(l.system_config, std::slice::from_ref(&system));
    assert_eq!(l.system_themes, [system.join("themes")]);
    assert_eq!(
        l.legacy,
        [
            Path::new(r"D:\Redirected\Roaming").join("F1R3Gaze"),
            Path::new(roaming).join("F1R3Gaze"),
            Path::new(r"C:\Users\u").join("F1R3Gaze"),
        ]
    );
    // Without the Known Folders, the environment.
    m.roaming_app_data = None;
    m.local_app_data = None;
    let l = platform_layout(Platform::Windows, &m).expect("a layout");
    assert_eq!(l.config, Path::new(r"D:\Redirected\Roaming").join(VENDOR).join(APP));
    assert_eq!(l.data, Path::new(r"D:\Redirected\Local").join(VENDOR).join(APP).join("data"));
}

#[test]
fn windows_without_application_data_is_an_error() {
    let m = machine(Some(r"C:\Users\u"), &[("APPDATA", "relative")]);
    assert_eq!(platform_layout(Platform::Windows, &m), Err(LayoutError::NoAppData));
    assert!(LayoutError::NoAppData.to_string().contains("--profile"));
}

#[test]
fn no_home_is_an_error_that_suggests_a_profile() {
    assert_eq!(platform_layout(Platform::Xdg, &machine(None, &[])), Err(LayoutError::NoHome));
    assert_eq!(platform_layout(Platform::MacOs, &machine(None, &[])), Err(LayoutError::NoHome));
    assert!(LayoutError::NoHome.to_string().contains("--profile"));
    // Every base set absolutely: no home needed for the roots.
    let full = [
        ("XDG_CONFIG_HOME", "/c"),
        ("XDG_DATA_HOME", "/d"),
        ("XDG_STATE_HOME", "/s"),
        ("XDG_CACHE_HOME", "/k"),
    ];
    let l = platform_layout(Platform::Xdg, &machine(None, &full)).expect("a layout");
    assert_eq!(l.legacy, [Path::new("/d/f1r3gaze")]);
}

#[test]
fn portable_roots_are_hermetic() {
    let l = Layout::portable(Path::new("/p"));
    assert_eq!(
        Class::ALL.map(|c| l.root(c).to_path_buf()),
        ["/p/config", "/p/data", "/p/state", "/p/cache", "/p/runtime"].map(PathBuf::from)
    );
    assert!(l.system_config.is_empty() && l.system_themes.is_empty());
    assert_eq!(l.legacy, [Path::new("/p")]);
    assert_eq!(l.kind, Kind::Portable(PathBuf::from("/p")));
}

/// The legacy candidates include the old `default_dir` (profile.rs before
/// the storage change), whenever it named an absolute directory.
#[test]
fn legacy_candidates_include_the_old_default_directory() {
    fn old_default_dir(platform: Platform, m: &Machine) -> PathBuf {
        let home = m.home.clone().unwrap_or_default();
        match platform {
            Platform::MacOs => home.join("Library/Application Support/F1R3Gaze"),
            Platform::Windows => m.env.get("APPDATA").map(PathBuf::from).unwrap_or(home).join("F1R3Gaze"),
            Platform::Xdg => m
                .env
                .get("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share"))
                .join("f1r3gaze"),
        }
    }
    let cases = [
        (Platform::Xdg, machine(Some("/home/u"), &[])),
        (Platform::Xdg, machine(Some("/home/u"), &[("XDG_DATA_HOME", "/data")])),
        (Platform::MacOs, machine(Some("/Users/u"), &[])),
        (Platform::Windows, machine(Some(r"C:\Users\u"), &[("APPDATA", r"C:\Users\u\AppData\Roaming"), ("LOCALAPPDATA", r"C:\Users\u\AppData\Local")])),
    ];
    for (platform, m) in cases {
        let l = platform_layout(platform, &m).expect("a layout");
        let old = old_default_dir(platform, &m);
        assert!(l.legacy.contains(&old), "{platform:?}: {old:?} not in {:?}", l.legacy);
    }
}

#[test]
fn paths_map_to_their_class() {
    let l = platform_layout(Platform::Xdg, &machine(Some("/home/u"), &[])).expect("a layout");
    assert_eq!(l.class_of(&l.settings_file()), Some((Class::Config, PathBuf::from("settings.toml"))));
    assert_eq!(l.class_of(&l.grants_file()), Some((Class::Data, PathBuf::from("permissions/grants.tsv"))));
    // The fallback runtime root sits under state, and wins as the longer root.
    assert_eq!(l.class_of(&l.lock_file()), Some((Class::Runtime, PathBuf::from("instance.lock"))));
    assert_eq!(l.class_of(&l.window_file()), Some((Class::State, PathBuf::from("window.json"))));
    assert_eq!(l.class_of(Path::new("/elsewhere")), None);
}

#[test]
fn the_skeleton_and_the_description_cover_every_root() {
    let l = Layout::portable(Path::new("/p"));
    let skeleton = l.skeleton();
    for class in Class::ALL {
        assert!(skeleton.contains(&l.root(class).to_path_buf()), "{class:?}");
    }
    for (i, dir) in skeleton.iter().enumerate() {
        if let Some(parent) = dir.parent()
            && let Some(at) = skeleton.iter().position(|d| d == parent)
        {
            assert!(at < i, "{} comes before its parent", dir.display());
        }
    }
    let description = l.describe();
    let expected = format!("config\t{}\ndata\t{}\n", l.config.display(), l.data.display());
    assert!(description.starts_with(&expected), "{description}");
    assert!(description.contains("legacy\t/p\n"));
}

#[test]
fn absolute_means_what_the_target_platform_says() {
    let abs = |p: Platform, s: &str| is_absolute_on(p, OsStr::new(s));
    assert!(abs(Platform::Xdg, "/x") && !abs(Platform::Xdg, "x") && !abs(Platform::Xdg, ""));
    assert!(!abs(Platform::Xdg, r"C:\x"));
    assert!(abs(Platform::Windows, r"C:\x") && abs(Platform::Windows, "c:/x"));
    assert!(abs(Platform::Windows, r"\\server\share"));
    assert!(!abs(Platform::Windows, "/x") && !abs(Platform::Windows, "C:x") && !abs(Platform::Windows, "x"));
}

#[test]
fn a_profile_argument_is_a_portable_root_made_absolute() {
    let m = machine(Some("/home/u"), &[]);
    let l = locate(Some(PathBuf::from("relative/profile")), &m).expect("a layout");
    let root = std::env::current_dir().expect("the current directory").join("relative/profile");
    assert_eq!(l.kind, Kind::Portable(root.clone()));
    assert_eq!(l.config, root.join("config"));
    let l = locate(Some(PathBuf::from("/abs/profile")), &m).expect("a layout");
    let root = std::path::absolute("/abs/profile").expect("the rooted path");
    assert_eq!(l.data, root.join("data"));
}
