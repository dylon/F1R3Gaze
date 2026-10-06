//! Tests of start-up's repair (ledger S6, part 2). They check, on MemFs:
//! - the rule of the design's table A.2 for every state of every managed
//!   file, exhaustively within each root and pairwise across roots;
//! - that nothing present before a start is lost;
//! - that a second start writes nothing but its busy flag;
//! - the model's invariants at every crash, and the start after it;
//! - wallet keys, examples, links, read-only sessions and the sweep.

use super::*;
use crate::profile::migrate::{begin, finish, mark_fresh};
use crate::ui_state::Visit;
use gaze_fs::{GARBAGE, MemFs, TraceFs, every_crash, every_two_crashes};
use gaze_shard::keys::{key_file_name, parse_key_file};
use gaze_wallet::{Entry, RECOVERED_LABEL, wallet_key_name};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The portable root every test uses.
const ROOT: &str = "/p";
/// When the tests' starts run: 2026-10-05 18:21:02 UTC.
const STAMP: u64 = 1_791_224_462;
/// The backups folder a start at [`STAMP`] claims.
const SESSION: &str = "2026-10-05T18-21-02Z";
/// The id a start makes when it needs one.
const NEW_ID: &str = "0123456789abcdef0123456789abcdef";

fn layout() -> Layout {
    Layout::portable(Path::new(ROOT))
}

// ── Starts ───────────────────────────────────────────────────────────────

/// What one start reported and left for the session.
struct Started {
    reconciled: Reconciled,
    events: Vec<Event>,
}

impl Started {
    /// The events about `path`.
    fn about(&self, path: &Path) -> Vec<&Event> {
        self.events.iter().filter(|e| e.path == path).collect()
    }
}

/// One start, in main.rs's order. With write access: the busy flag (and the
/// barrier after a start that did not finish), the marker of a profile
/// with nothing to migrate (step 4; /p holds no old profile), the folders
/// and the sweep, the repair, and the flag removed. Read-only: the folders'
/// check and the repair, which then only report.
fn start_with(fs: Arc<dyn Fs + Send + Sync>, access: Access, keystore: KeystoreKind) -> Result<Started, String> {
    let l = layout();
    let report = Report::silent();
    let now = UNIX_EPOCH + Duration::from_secs(STAMP);
    let backups = Backups::new(Arc::new(l.clone()), fs.clone(), now);
    let new_id = || NEW_ID.to_string();
    let writes = access == Access::Write;
    if writes {
        begin(&l, fs.as_ref()).map_err(|e| format!("begin: {e}"))?;
        mark_fresh(&l, fs.as_ref(), now, &report).map_err(|e| format!("the marker: {e}"))?;
    }
    let start = Start {
        layout: &l,
        fs: fs.as_ref(),
        backups: &backups,
        report: &report,
        access: &access,
        keystore,
        new_user_id: &new_id,
    };
    prepare(&start)?;
    let reconciled = reconcile(&start);
    if writes {
        finish(&l, fs.as_ref()).map_err(|e| format!("finish: {e}"))?;
    }
    Ok(Started {
        reconciled,
        events: report.events(),
    })
}

/// A start with write access and the file keystore, which must finish.
fn start(fs: &Arc<MemFs>) -> Started {
    start_with(fs.clone(), Access::Write, KeystoreKind::File).expect("the start finishes")
}

/// A second start on what the first left: it writes only its busy flag.
fn second_start_writes_only_its_flag(after: &MemFs, keystore: KeystoreKind, what: &str) {
    let l = layout();
    let trace = Arc::new(TraceFs::new(Arc::new(MemFs::clone(after))));
    start_with(trace.clone(), Access::Write, keystore).expect("a second start");
    let writes: Vec<(&str, PathBuf)> = trace
        .log()
        .into_iter()
        .filter(|t| t.writes())
        .map(|t| (t.op, t.path))
        .collect();
    let expected = [
        ("create_new", l.busy_file()),
        ("remove_file", l.busy_file()),
        ("sync_dir", l.data.clone()),
    ];
    assert_eq!(writes, expected, "{what}: a second start");
}

// ── Fixtures ─────────────────────────────────────────────────────────────

/// A wallet: its secret key as the file keystore keeps it, and its address.
fn wallet(byte: u8) -> (String, Address) {
    let hex = gaze_net::hex(&[byte; 32]);
    let key = parse_key_file(hex.as_bytes()).expect("a valid secret");
    (hex, Address::from_key(key.verifying_key()))
}

/// Where the file keystore keeps a wallet's key.
fn key_path(address: &Address) -> PathBuf {
    layout().keys_dir().join(key_file_name(&wallet_key_name(address)))
}

/// A wallet's line in the list.
fn list_line(address: &Address, label: &str) -> String {
    entry_line(&Entry {
        address: address.clone(),
        label: label.into(),
    })
}

/// A line the grants file accepts.
fn grant_line() -> String {
    format!("https://a.example\t{}\turn:f1r3:site\tallow\tread\n", "a".repeat(64))
}

/// A freshness file with one record.
const FRESHNESS: &str =
    "# F1R3Gaze freshness records: shard, binding, highest finalized block seen\nroot\tsite.example\t42\n";

/// The states a file is tested in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Missing,
    Valid,
    Corrupt,
    Empty,
    /// A folder where the file should be: reading it fails.
    Unreadable,
}

const STATES: [State; 5] = [State::Missing, State::Valid, State::Corrupt, State::Empty, State::Unreadable];

/// Content each file accepts, other than its default where it has one.
fn valid(m: Managed) -> Vec<u8> {
    match m {
        Managed::Settings => b"[appearance]\ntheme = \"default-light\"\n".to_vec(),
        Managed::SettingsExample => settings::TEMPLATE.as_bytes().to_vec(),
        Managed::DarkExample => example_css(Scheme::Dark).into_bytes(),
        Managed::LightExample => example_css(Scheme::Light).into_bytes(),
        Managed::Marker => b"{\"layout\":1,\"created\":\"2026-10-01T09:00:00Z\"}\n".to_vec(),
        Managed::UserId => b"fedcba9876543210fedcba9876543210\n".to_vec(),
        Managed::WalletList => list_line(&wallet(1).1, "Spending").into_bytes(),
        Managed::ActiveWallet => format!("{}\n", wallet(1).1).into_bytes(),
        Managed::Grants => grant_line().into_bytes(),
        Managed::SiteIndex => br#"["https://a.example","https://b.example"]"#.to_vec(),
        Managed::Freshness => FRESHNESS.as_bytes().to_vec(),
        Managed::Window => WindowState {
            zoom: 1.5,
            maximized: true,
            ..WindowState::default()
        }
        .to_bytes(),
        Managed::Session => SessionState {
            sidebar_open: true,
            panel: "history".into(),
            ..SessionState::default()
        }
        .to_bytes(),
        Managed::History => History {
            visits: vec![Visit {
                url: "https://a.example/".into(),
                title: "A".into(),
                at: 1_791_224_000,
            }],
            ..History::default()
        }
        .to_bytes(),
    }
}

/// Content each file does not accept. A line file gets a usable line and
/// one that is not.
fn corrupt(m: Managed) -> Vec<u8> {
    match m {
        Managed::Settings => b"[appearance\ntheme = \"default-light\"\n".to_vec(),
        Managed::SettingsExample => b"# my own notes\n".to_vec(),
        Managed::DarkExample => b":root { --gaze-bg: #123456; }\n".to_vec(),
        Managed::LightExample => b"edited\n".to_vec(),
        Managed::Marker => b"{\"layout\":".to_vec(),
        Managed::UserId => b"two words\n".to_vec(),
        Managed::WalletList => [valid(m), b"not an address\n".to_vec()].concat(),
        Managed::ActiveWallet => b"nobody\n".to_vec(),
        Managed::Grants => [valid(m), b"half a line\n".to_vec()].concat(),
        Managed::SiteIndex => br#"["https://a.example",7,"https://b.example"]"#.to_vec(),
        Managed::Freshness => [valid(m), b"root\tsite.example\tnot a block\n".to_vec()].concat(),
        Managed::Window | Managed::Session | Managed::History => b"{\"version\":1,".to_vec(),
    }
}

/// Whether an empty file is valid: an empty list, or settings that set
/// nothing.
fn empty_is_valid(m: Managed) -> bool {
    matches!(m, Managed::Settings | Managed::WalletList | Managed::Grants | Managed::Freshness)
}

/// Whether a file in `state` is damaged.
fn damaged(m: Managed, state: State) -> bool {
    match state {
        State::Corrupt => true,
        State::Empty => !empty_is_valid(m),
        State::Missing | State::Valid | State::Unreadable => false,
    }
}

/// What a damaged file holds once repaired, `None` if it is removed: the
/// design's table A.2, written out here rather than taken from
/// `Managed::check`.
fn repaired(m: Managed, original: &[u8]) -> Option<Vec<u8>> {
    match m {
        Managed::Settings | Managed::SettingsExample => Some(settings::TEMPLATE.as_bytes().to_vec()),
        Managed::DarkExample => Some(example_css(Scheme::Dark).into_bytes()),
        Managed::LightExample => Some(example_css(Scheme::Light).into_bytes()),
        Managed::Marker => Some(b"{\"layout\":1,\"repaired\":true}\n".to_vec()),
        Managed::UserId => Some(format!("{NEW_ID}\n").into_bytes()),
        // The usable lines of `corrupt`, byte for byte.
        Managed::WalletList | Managed::Grants | Managed::Freshness => Some(valid(m)),
        Managed::ActiveWallet => None,
        Managed::SiteIndex => Some(match original.is_empty() {
            true => b"[]".to_vec(),
            false => br#"["https://a.example","https://b.example"]"#.to_vec(),
        }),
        Managed::Window => Some(WindowState::default().to_bytes()),
        Managed::Session => Some(SessionState::default().to_bytes()),
        Managed::History => Some(History::default().to_bytes()),
    }
}

/// The marker step 4 makes for a profile with nothing to migrate.
const FRESH_MARKER: &[u8] = b"{\"layout\":1,\"created\":\"2026-10-05T18:21:02Z\"}\n";

/// What a missing file is created with, `None` if it is left missing (the
/// matrix seeds no wallet keys, so nothing is recovered). The marker is
/// step 4's, not the repair's.
fn created(m: Managed) -> Option<Vec<u8>> {
    match m {
        Managed::Marker => Some(FRESH_MARKER.to_vec()),
        Managed::Settings | Managed::SettingsExample => Some(settings::TEMPLATE.as_bytes().to_vec()),
        Managed::DarkExample => Some(example_css(Scheme::Dark).into_bytes()),
        Managed::LightExample => Some(example_css(Scheme::Light).into_bytes()),
        Managed::UserId => Some(format!("{NEW_ID}\n").into_bytes()),
        Managed::Window => Some(WindowState::default().to_bytes()),
        Managed::Session => Some(SessionState::default().to_bytes()),
        Managed::History => Some(History::default().to_bytes()),
        Managed::WalletList
        | Managed::ActiveWallet
        | Managed::Grants
        | Managed::SiteIndex
        | Managed::Freshness => None,
    }
}

/// An event's kind without its fields.
fn tag(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Created => "created",
        EventKind::Refreshed => "refreshed",
        EventKind::Repaired { .. } => "repaired",
        EventKind::Regenerated { .. } => "regenerated",
        EventKind::Unreadable { .. } => "unreadable",
        EventKind::Unrepaired { .. } => "unrepaired",
        EventKind::Newer { .. } => "newer",
        EventKind::Recovered { .. } => "recovered",
        EventKind::CorruptKey { .. } => "corrupt-key",
        EventKind::MissingKey { .. } => "missing-key",
        EventKind::StrayKey { .. } => "stray-key",
        EventKind::Swept { .. } => "swept",
        other => panic!("start-up's repair does not report {other:?}"),
    }
}

/// The kinds and severities of `events`.
fn kinds(events: &[&Event]) -> Vec<(&'static str, Severity)> {
    events.iter().map(|e| (tag(&e.kind), e.severity)).collect()
}

/// How a damaged file's repair is reported (table A.2).
fn repair_event(m: Managed) -> (&'static str, Severity) {
    match m {
        Managed::Settings
        | Managed::Marker
        | Managed::ActiveWallet
        | Managed::Window
        | Managed::Session
        | Managed::History => ("regenerated", Severity::Warning),
        Managed::SettingsExample | Managed::DarkExample | Managed::LightExample => ("refreshed", Severity::Quiet),
        // Sites see the profile as a new user.
        Managed::UserId => ("regenerated", Severity::Alert),
        Managed::WalletList | Managed::Grants | Managed::SiteIndex => ("repaired", Severity::Warning),
        // A stale answer for the lost records' names could be accepted.
        Managed::Freshness => ("repaired", Severity::Alert),
    }
}

/// How a file that cannot be read is reported.
fn unreadable_severity(m: Managed) -> Severity {
    match m {
        Managed::Freshness => Severity::Alert,
        _ => Severity::Warning,
    }
}

/// A profile with every folder, the files of `states` in their states and
/// every other managed file valid.
fn profile(states: &[(Managed, State)]) -> MemFs {
    let fs = MemFs::new();
    let l = layout();
    for dir in l.skeleton() {
        fs.seed_dir(dir);
    }
    for m in Managed::ALL {
        let state = states.iter().find(|(n, _)| *n == m).map_or(State::Valid, |(_, s)| *s);
        let path = m.path(&l);
        match state {
            State::Missing => {}
            State::Valid => fs.seed_file(&path, &valid(m)),
            State::Corrupt => fs.seed_file(&path, &corrupt(m)),
            State::Empty => fs.seed_file(&path, b""),
            State::Unreadable => fs.seed_dir(&path),
        }
    }
    fs
}

/// Whether `path` is inside a backups folder.
fn in_backups(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "backups")
}

/// Where a start at [`STAMP`] backs up the file at `path`.
fn backup_of(path: &Path) -> PathBuf {
    let l = layout();
    let (class, below) = l.class_of(path).expect("a managed file is in a root");
    l.backups(class).join(SESSION).join(below)
}

// ── The rule ─────────────────────────────────────────────────────────────

/// Checks one start against the rule for every file of `states`; `before`
/// is what the files held.
fn check_rule(states: &[(Managed, State)], before: &BTreeMap<PathBuf, Vec<u8>>, after: &MemFs, started: &Started) {
    let l = layout();
    let files = after.files();
    let dirs: BTreeSet<PathBuf> = after.dirs().into_iter().collect();
    for &(m, state) in states {
        let path = m.path(&l);
        let about = started.about(&path);
        let blocked = started.reconciled.blocked.why(m);
        let what = format!("{} {state:?}, in {states:?}", m.name());
        match (damaged(m, state), state) {
            (true, _) => {
                let original = &before[&path];
                assert_eq!(files.get(&path), repaired(m, original).as_ref(), "{what}: the repair");
                let backup = backup_of(&path);
                assert_eq!(files.get(&backup), Some(original), "{what}: the backup");
                assert_eq!(kinds(&about), [repair_event(m)], "{what}");
                assert_eq!(about[0].backup.as_ref(), Some(&backup), "{what}");
                assert!(
                    about[0].message.contains(&backup.display().to_string()),
                    "{what}: the message names the backup: {}",
                    about[0].message
                );
                assert_eq!(blocked, None, "{what}");
            }
            (false, State::Missing) => {
                match created(m) {
                    Some(bytes) => {
                        assert_eq!(files.get(&path), Some(&bytes), "{what}: created");
                        assert_eq!(kinds(&about), [("created", Severity::Quiet)], "{what}");
                    }
                    None => {
                        assert!(!files.contains_key(&path), "{what}: left missing");
                        assert!(about.is_empty(), "{what}: {about:?}");
                    }
                }
                assert_eq!(blocked, None, "{what}");
            }
            (false, State::Valid | State::Empty | State::Corrupt) => {
                assert_eq!(files.get(&path), before.get(&path), "{what}: unchanged");
                assert!(about.is_empty(), "{what}: {about:?}");
                assert_eq!(blocked, None, "{what}");
            }
            (false, State::Unreadable) => {
                assert!(dirs.contains(&path), "{what}: left as it is");
                assert!(blocked.is_some(), "{what}: nothing is saved to it");
                assert_eq!(kinds(&about), [("unreadable", unreadable_severity(m))], "{what}");
            }
        }
    }
    let damaged_files = states.iter().filter(|(m, s)| damaged(*m, *s)).count();
    let backed_up = files.keys().filter(|p| in_backups(p)).count();
    assert_eq!(backed_up, damaged_files, "exactly the damaged files are backed up, in {states:?}");
}

/// Every file present before a start, but its busy flag and dead writers'
/// temporary files outside the keystore, still holds its bytes at its path,
/// or a file in a backups folder holds them.
fn nothing_is_lost(before: &BTreeMap<PathBuf, Vec<u8>>, after: &BTreeMap<PathBuf, Vec<u8>>, what: &str) {
    let l = layout();
    let own = std::process::id();
    let kept: BTreeSet<&[u8]> = after
        .iter()
        .filter(|(p, _)| in_backups(p))
        .map(|(_, bytes)| bytes.as_slice())
        .collect();
    for (path, bytes) in before {
        let dead_temp = path
            .file_name()
            .and_then(gaze_fs::temp_owner)
            .is_some_and(|pid| pid != own)
            && !path.starts_with(l.keys_dir());
        if *path == l.busy_file() || dead_temp {
            continue;
        }
        assert!(
            after.get(path) == Some(bytes) || kept.contains(bytes.as_slice()),
            "{what}: {} was lost",
            path.display()
        );
    }
}

/// One case: a start checked against the rule, nothing lost, nothing left
/// pending, and a second start that writes nothing.
fn run_case(states: &[(Managed, State)]) {
    let fs = Arc::new(profile(states));
    let before = fs.files();
    let started = start(&fs);
    let what = format!("{states:?}");
    check_rule(states, &before, &fs, &started);
    nothing_is_lost(&before, &fs.files(), &what);
    assert_eq!(fs.pending_len(), 0, "{what}: nothing is left pending");
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, &what);
}

/// Every combination of [`STATES`] over `files`.
fn combinations(files: &[Managed]) -> Vec<Vec<(Managed, State)>> {
    let total = STATES.len().pow(u32::try_from(files.len()).expect("a few files"));
    let mut all: Vec<Vec<(Managed, State)>> = Vec::with_capacity(total);
    all.push(Vec::with_capacity(files.len()));
    for &m in files {
        let mut next = Vec::with_capacity(all.len() * STATES.len());
        for case in &all {
            for state in STATES {
                let mut extended = case.clone();
                extended.push((m, state));
                next.push(extended);
            }
        }
        all = next;
    }
    all
}

/// Runs `cases` on every core.
fn run_all(cases: &[Vec<(Managed, State)>]) {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = cases.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for part in cases.chunks(chunk) {
            scope.spawn(move || part.iter().for_each(|case| run_case(case)));
        }
    });
}

#[test]
fn every_state_of_every_file_in_each_root() {
    let in_root = |class: Class| -> Vec<Managed> {
        Managed::ALL
            .into_iter()
            .filter(|m| m.class() == class && *m != Managed::Marker)
            .collect()
    };
    let mut cases = Vec::with_capacity(625 + 15_625 + 125);
    for class in [Class::Config, Class::Data, Class::State] {
        cases.extend(combinations(&in_root(class)));
    }
    assert_eq!(cases.len(), 625 + 15_625 + 125);
    run_all(&cases);
}

#[test]
fn every_pair_of_files_across_roots() {
    let mut cases = Vec::with_capacity(61 * 25);
    for (i, &a) in Managed::ALL.iter().enumerate() {
        for &b in &Managed::ALL[i + 1..] {
            if a.class() != b.class() {
                cases.extend(combinations(&[a, b]));
            }
        }
    }
    assert_eq!(cases.len(), 61 * 25);
    run_all(&cases);
}

#[test]
fn a_second_start_writes_nothing() {
    // Each state of each file on its own, the marker's too.
    let mut cases = Vec::with_capacity(Managed::ALL.len() * STATES.len());
    for m in Managed::ALL {
        for state in STATES {
            cases.push(vec![(m, state)]);
        }
    }
    run_all(&cases);
    // A first launch, and a start that listed wallets again from their keys.
    let fs = Arc::new(MemFs::new());
    start(&fs);
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, "first launch");
    let fs = Arc::new(all_damaged());
    start(&fs);
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, "every file damaged, wallets recovered");
}

#[test]
fn first_launch_creates_the_skeleton() {
    let l = layout();
    let fs = Arc::new(MemFs::new());
    let started = start(&fs);
    let dirs: BTreeSet<PathBuf> = fs.dirs().into_iter().collect();
    for dir in l.skeleton() {
        assert!(dirs.contains(&dir), "{} is created", dir.display());
        assert_eq!(fs.mode(&dir).expect("its mode"), Some(0o700), "{}", dir.display());
    }
    let files = fs.files();
    let expected: BTreeMap<PathBuf, Vec<u8>> = Managed::ALL
        .into_iter()
        .filter_map(|m| created(m).map(|bytes| (m.path(&l), bytes)))
        .collect();
    assert_eq!(files, expected, "exactly the files with defaults, and no busy flag");
    for path in files.keys() {
        assert_eq!(fs.mode(path).expect("its mode"), Some(0o600), "{}", path.display());
    }
    assert!(
        started.events.iter().all(|e| e.severity == Severity::Quiet),
        "a first launch has nothing to show: {:?}",
        started.events
    );
    assert_eq!(fs.pending_len(), 0);
}

#[test]
fn every_fix_passes_its_own_check() {
    let l = layout();
    let (_, unlisted) = wallet(2);
    let with_key = KeyScan {
        keys: vec![(key_path(&unlisted), KeyFile::Wallet(unlisted.clone()))],
        stranded: Vec::new(),
    };
    let new_id = || NEW_ID.to_string();
    for keys in [KeyScan::default(), with_key] {
        let input = CheckInput {
            keys: &keys,
            new_user_id: &new_id,
        };
        for m in Managed::ALL {
            let path = m.path(&l);
            assert!(
                matches!(m.check(&path, &corrupt(m), &input), Check::Damaged { .. }),
                "{}: the corrupt fixture is damaged",
                m.name()
            );
            for bytes in [corrupt(m), Vec::new(), b"\xff\xfe".to_vec(), GARBAGE.to_vec()] {
                match m.check(&path, &bytes, &input) {
                    Check::Damaged {
                        fix: Fix::Replace { bytes: fixed, .. },
                        ..
                    } => assert_eq!(
                        m.check(&path, &fixed, &input),
                        Check::Valid,
                        "{}: the repair {:?}",
                        m.name(),
                        String::from_utf8_lossy(&fixed)
                    ),
                    // Its repaired state is absent, which is left alone.
                    Check::Damaged { fix: Fix::Remove { .. }, .. } => {
                        assert_eq!(m.missing(&input), Missing::Leave, "{}", m.name());
                    }
                    Check::Valid | Check::Newer { .. } => {}
                }
            }
            if let Missing::Create { bytes, .. } = m.missing(&input) {
                assert_eq!(m.check(&path, &bytes, &input), Check::Valid, "{}: created", m.name());
            }
        }
    }
}

// ── Crashes ──────────────────────────────────────────────────────────────

/// A profile with every file damaged, a listed wallet's key, and the keys
/// of two wallets the damaged list does not name.
fn all_damaged() -> MemFs {
    let fs = profile(&Managed::ALL.map(|m| (m, State::Corrupt)));
    for byte in [1, 2, 3] {
        let (hex, address) = wallet(byte);
        fs.seed_file(key_path(&address), hex.as_bytes());
    }
    fs
}

/// The model's invariants at a crash state. `before` is the profile before
/// the start and `done` what a start without crashes leaves:
/// - `NeverAbsent`: a managed file that was there is there, unless its
///   repair removes it;
/// - `RepairInPlace` and `PublishedComplete`: it holds its original or its
///   repair, and a file that was missing is absent or whole;
/// - `CorruptKept`: its original is at its path or in a backup.
fn check_crash_state(before: &BTreeMap<PathBuf, Vec<u8>>, done: &BTreeMap<PathBuf, Vec<u8>>, fs: &MemFs, when: &str) {
    let l = layout();
    let files = fs.files();
    let backed_up: BTreeSet<&[u8]> = files
        .iter()
        .filter(|(p, _)| in_backups(p))
        .map(|(_, bytes)| bytes.as_slice())
        .collect();
    for m in Managed::ALL {
        let path = m.path(&l);
        let now = files.get(&path);
        let repair = done.get(&path);
        match (before.get(&path), now) {
            (Some(original), Some(now)) => assert!(
                now == original || Some(now) == repair,
                "{when}: {} holds {:?}",
                m.name(),
                String::from_utf8_lossy(now)
            ),
            (Some(_), None) => assert!(repair.is_none(), "{when}: {} is absent", m.name()),
            (None, Some(now)) => assert_eq!(Some(now), repair, "{when}: a new {} is whole", m.name()),
            (None, None) => {}
        }
        if let Some(original) = before.get(&path) {
            assert!(
                now == Some(original) || backed_up.contains(original.as_slice()),
                "{when}: the original {} is neither in place nor in a backup",
                m.name()
            );
        }
    }
}

/// The profile apart from backups and this process's own temporary files:
/// a real restart is another process, which sweeps them (see
/// [`the_sweep_removes_only_dead_writers_temps`]).
fn comparable(files: &BTreeMap<PathBuf, Vec<u8>>) -> BTreeMap<PathBuf, Vec<u8>> {
    let own = std::process::id();
    files
        .iter()
        .filter(|(p, _)| !in_backups(p) && p.file_name().and_then(gaze_fs::temp_owner) != Some(own))
        .map(|(p, bytes)| (p.clone(), bytes.clone()))
        .collect()
}

/// The start after a crash finishes with the profile a start without
/// crashes leaves, loses nothing, and (but on NTFS, where folders cannot be
/// synced) leaves nothing pending.
fn check_recovery(crashed: &MemFs, before: &BTreeMap<PathBuf, Vec<u8>>, done: &BTreeMap<PathBuf, Vec<u8>>, posix: bool, when: &str) {
    let fs = Arc::new(MemFs::clone(crashed));
    if let Err(e) = start_with(fs.clone(), Access::Write, KeystoreKind::File) {
        panic!("{when}: the next start fails: {e}");
    }
    if posix {
        assert_eq!(fs.pending(), Vec::<String>::new(), "{when}: the next start leaves something pending");
    }
    let files = fs.files();
    assert_eq!(comparable(&files), comparable(done), "{when}: the next start");
    nothing_is_lost(before, &files, when);
}

/// Every crash of a start on `shape`, under POSIX and NTFS semantics.
fn every_crash_of(shape: impl Fn() -> MemFs, least: usize) {
    for (name, start_fs) in [("posix", shape()), ("ntfs", shape().ntfs())] {
        let before = start_fs.files();
        let finished = Arc::new(MemFs::clone(&start_fs));
        start_with(finished.clone(), Access::Write, KeystoreKind::File).expect("a start without crashes");
        let done = finished.files();
        let checked = every_crash(
            &start_fs,
            |fs| start_with(fs.clone(), Access::Write, KeystoreKind::File),
            |crash, fs| {
                let when = format!("{name}: {crash}");
                check_crash_state(&before, &done, fs, &when);
                check_recovery(fs, &before, &done, name == "posix", &when);
            },
        );
        eprintln!("{name}: {checked} crash states checked");
        assert!(checked >= least, "{name}: only {checked} crash states");
    }
}

#[test]
fn a_repaired_file_is_never_absent_at_any_crash() {
    every_crash_of(all_damaged, 300);
}

#[test]
fn first_launch_survives_every_crash() {
    every_crash_of(MemFs::new, 100);
}

#[test]
fn two_crashes_keep_every_byte() {
    let shape = || {
        profile(&[
            (Managed::Session, State::Corrupt),
            (Managed::Grants, State::Corrupt),
            (Managed::Settings, State::Missing),
        ])
    };
    let start_fs = shape();
    let before = start_fs.files();
    let finished = Arc::new(shape());
    start(&finished);
    let done = finished.files();
    let checked = every_two_crashes(
        &start_fs,
        // The barrier is the start's own, after an unfinished start.
        |_| {},
        |fs| start_with(fs.clone(), Access::Write, KeystoreKind::File),
        |crash, fs| check_crash_state(&before, &done, fs, &crash.to_string()),
    );
    eprintln!("{checked} states after two crashes checked");
    assert!(checked >= 1_000, "only {checked} crash states");
}

#[test]
fn a_finished_start_leaves_nothing_pending() {
    for (what, shape) in [
        ("first launch", MemFs::new()),
        ("steady", profile(&[])),
        ("every file damaged", all_damaged()),
        ("every file missing", profile(&Managed::ALL.map(|m| (m, State::Missing)))),
    ] {
        let fs = Arc::new(shape);
        start(&fs);
        assert_eq!(fs.pending_len(), 0, "{what}");
    }
}

// ── Wallet keys ──────────────────────────────────────────────────────────

#[test]
fn corrupt_keys_are_reported_and_never_touched() {
    let l = layout();
    let (hex1, a1) = wallet(1);
    let (_, a2) = wallet(2);
    let fs = profile(&[]);
    let list = [valid(Managed::WalletList), list_line(&a2, "Savings").into_bytes()].concat();
    fs.seed_file(Managed::WalletList.path(&l), &list);
    fs.seed_file(key_path(&a1), hex1.as_bytes());
    fs.seed_file(key_path(&a2), b"not a key");
    let dangling = l.keys_dir().join(format!("{}.key", "e".repeat(64)));
    fs.seed_symlink(&dangling, "/nowhere");
    let before = fs.files();
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert_eq!(fs.files(), before, "key files are never written, copied or removed");
    assert!(!fs.dirs().iter().any(|d| in_backups(d)), "no backup was made");
    let corrupt = started.about(&key_path(&a2));
    assert_eq!(kinds(&corrupt), [("corrupt-key", Severity::Alert)]);
    assert_eq!(corrupt[0].kind, EventKind::CorruptKey { wallet: Some(a2.to_string()) });
    assert!(
        corrupt[0].message.contains("\"Savings\"") && corrupt[0].message.contains(a2.as_str()),
        "the alert names the wallet: {}",
        corrupt[0].message
    );
    assert_eq!(kinds(&started.about(&dangling)), [("unreadable", Severity::Alert)]);
    assert!(started.about(&key_path(&a1)).is_empty(), "a good key says nothing");
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, "corrupt keys");
}

#[test]
fn wallets_are_listed_again_from_their_keys() {
    let l = layout();
    let list = Managed::WalletList.path(&l);
    let (hex1, a1) = wallet(1);
    let (hex2, a2) = wallet(2);
    let (hex3, a3) = wallet(3);
    let recovered_line = |a: &Address| list_line(a, RECOVERED_LABEL);
    let other_pid = std::process::id().wrapping_add(1);

    // (a) The list is missing; two wallets have keys. They are listed in
    // the order of their key files' names.
    let fs = profile(&[(Managed::WalletList, State::Missing), (Managed::ActiveWallet, State::Missing)]);
    fs.seed_file(key_path(&a2), hex2.as_bytes());
    fs.seed_file(key_path(&a3), hex3.as_bytes());
    let fs = Arc::new(fs);
    let started = start(&fs);
    let mut order = vec![a2.clone(), a3.clone()];
    order.sort_by_key(key_path);
    let expected: String = order.iter().map(recovered_line).collect();
    assert_eq!(fs.files()[&list], expected.into_bytes(), "(a)");
    assert_eq!(started.reconciled.recovered, order, "(a)");
    assert_eq!(
        kinds(&started.about(&list)),
        [("recovered", Severity::Warning), ("recovered", Severity::Warning)],
        "(a)"
    );
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, "(a)");

    // (b) A damaged list and a key it does not name.
    let fs = profile(&[(Managed::WalletList, State::Corrupt)]);
    fs.seed_file(key_path(&a1), hex1.as_bytes());
    fs.seed_file(key_path(&a2), hex2.as_bytes());
    let fs = Arc::new(fs);
    let started = start(&fs);
    let files = fs.files();
    assert_eq!(files[&list], [valid(Managed::WalletList), recovered_line(&a2).into_bytes()].concat(), "(b)");
    assert_eq!(files[&backup_of(&list)], corrupt(Managed::WalletList), "(b)");
    assert_eq!(
        kinds(&started.about(&list)),
        [("repaired", Severity::Warning), ("recovered", Severity::Warning)],
        "(b)"
    );
    assert_eq!(started.reconciled.recovered, std::slice::from_ref(&a2), "(b)");
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, "(b)");

    // (c) A valid list that does not name a wallet whose key is there.
    let fs = profile(&[]);
    fs.seed_file(key_path(&a1), hex1.as_bytes());
    fs.seed_file(key_path(&a3), hex3.as_bytes());
    let fs = Arc::new(fs);
    let started = start(&fs);
    let files = fs.files();
    assert_eq!(files[&list], [valid(Managed::WalletList), recovered_line(&a3).into_bytes()].concat(), "(c)");
    assert_eq!(files[&backup_of(&list)], valid(Managed::WalletList), "(c)");
    assert_eq!(kinds(&started.about(&list)), [("recovered", Severity::Warning)], "(c)");
    second_start_writes_only_its_flag(&fs, KeystoreKind::File, "(c)");

    // (d) A valid key under a name that is not a wallet's: not recovered.
    let fs = profile(&[(Managed::WalletList, State::Missing)]);
    fs.seed_file(l.keys_dir().join(key_file_name("session:example")), hex2.as_bytes());
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert!(!fs.files().contains_key(&list), "(d)");
    assert!(started.reconciled.recovered.is_empty(), "(d)");

    // (e) A damaged key under a wallet's name: not recovered, reported.
    let fs = profile(&[(Managed::WalletList, State::Missing)]);
    fs.seed_file(key_path(&a2), b"not a key");
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert!(!fs.files().contains_key(&list), "(e)");
    assert_eq!(kinds(&started.about(&key_path(&a2))), [("corrupt-key", Severity::Alert)], "(e)");
    assert_eq!(started.about(&key_path(&a2))[0].kind, EventKind::CorruptKey { wallet: None }, "(e)");

    // (f) An unfinished save holding the only copy of a key: left as it
    // is, and reported with the command that imports it.
    let fs = profile(&[(Managed::WalletList, State::Missing)]);
    let stranded = l
        .keys_dir()
        .join(format!(".{}.tmp-{other_pid}-0", key_file_name(&wallet_key_name(&a2))));
    fs.seed_file(&stranded, hex2.as_bytes());
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert!(!fs.files().contains_key(&list), "(f)");
    assert_eq!(fs.files()[&stranded], hex2.as_bytes(), "(f): the keystore is never swept");
    let stray = started.about(&stranded);
    assert_eq!(kinds(&stray), [("stray-key", Severity::Warning)], "(f)");
    assert!(stray[0].message.contains("f1r3gaze wallet import"), "(f): {}", stray[0].message);

    // (g) With the operating system's keystore, key files mean nothing.
    let fs = profile(&[(Managed::WalletList, State::Missing)]);
    fs.seed_file(key_path(&a2), hex2.as_bytes());
    let fs = Arc::new(fs);
    let started = start_with(fs.clone(), Access::Write, KeystoreKind::Os).expect("a start");
    assert!(!fs.files().contains_key(&list), "(g)");
    assert!(
        !started.events.iter().any(|e| e.path.starts_with(l.keys_dir())),
        "(g): {:?}",
        started.events
    );
    second_start_writes_only_its_flag(&fs, KeystoreKind::Os, "(g)");
}

#[test]
fn recovery_never_chooses_a_payer() {
    let l = layout();
    let (hex2, a2) = wallet(2);
    for list in [State::Missing, State::Corrupt, State::Valid] {
        let fs = profile(&[(Managed::WalletList, list), (Managed::ActiveWallet, State::Missing)]);
        fs.seed_file(key_path(&a2), hex2.as_bytes());
        let fs = Arc::new(fs);
        let started = start(&fs);
        assert_eq!(started.reconciled.recovered, std::slice::from_ref(&a2), "{list:?}");
        assert!(!fs.files().contains_key(&Managed::ActiveWallet.path(&l)), "{list:?}: no wallet pays until one is chosen");
    }
}

// ── Particular files ─────────────────────────────────────────────────────

#[test]
fn examples_are_refreshed_with_their_old_content_kept() {
    let l = layout();
    for m in [Managed::SettingsExample, Managed::DarkExample, Managed::LightExample] {
        let fs = Arc::new(profile(&[(m, State::Corrupt)]));
        let started = start(&fs);
        let path = m.path(&l);
        let files = fs.files();
        assert_eq!(files.get(&path), created(m).as_ref(), "{}: rewritten", m.name());
        assert_eq!(files[&backup_of(&path)], corrupt(m), "{}: the old text is kept", m.name());
        assert_eq!(kinds(&started.about(&path)), [("refreshed", Severity::Quiet)], "{}", m.name());
    }
}

#[test]
fn a_read_only_run_changes_nothing() {
    let l = layout();
    let why = "another F1R3Gaze holds this profile";
    // Half the files damaged, half missing, and a dead writer's temporary
    // file.
    let states: Vec<(Managed, State)> = Managed::ALL
        .iter()
        .enumerate()
        .map(|(i, &m)| (m, if i % 2 == 0 { State::Corrupt } else { State::Missing }))
        .collect();
    let mem = Arc::new(profile(&states));
    let dead = l.config.join(format!(".settings.toml.tmp-{}-0", std::process::id().wrapping_add(1)));
    mem.seed_file(&dead, b"half");
    // And a profile with no folders at all.
    let empty = Arc::new(MemFs::new());
    for (what, mem) in [("damaged and missing", mem), ("no folders", empty)] {
        let (files, dirs) = (mem.files(), mem.dirs());
        let trace = Arc::new(TraceFs::new(mem.clone()));
        let started = start_with(
            trace.clone(),
            Access::ReadOnly { why: why.into() },
            KeystoreKind::File,
        )
        .expect("a read-only start");
        let writes: Vec<String> = trace
            .log()
            .into_iter()
            .filter(|t| t.writes())
            .map(|t| t.json())
            .collect();
        assert!(writes.is_empty(), "{what}: {writes:?}");
        assert_eq!(mem.files(), files, "{what}");
        assert_eq!(mem.dirs(), dirs, "{what}");
        assert_eq!(started.reconciled.blocked.all.as_deref(), Some(why), "{what}");
        assert!(started.events.iter().all(|e| e.backup.is_none()), "{what}");
        for e in &started.events {
            assert!(
                e.message.contains("would") || matches!(e.kind, EventKind::MissingKey { .. }),
                "{what}: {}",
                e.message
            );
        }
    }
    // Each file that would change is named.
    let mem = Arc::new(profile(&states));
    let started = start_with(mem, Access::ReadOnly { why: why.into() }, KeystoreKind::File).expect("a read-only start");
    for &(m, state) in &states {
        let named = !started.about(&m.path(&l)).is_empty();
        // The marker is step 4's to make, which a read-only start skips.
        let made = state == State::Missing && m != Managed::Marker && created(m).is_some();
        assert_eq!(named, state == State::Corrupt || made, "{} {state:?}", m.name());
    }
}

/// Something that is not a folder where a folder must be stops a start
/// from writing; a session that only reads says so, and touches nothing.
#[test]
fn a_read_only_run_names_a_file_in_the_way_of_a_folder() {
    let l = layout();
    let fs = Arc::new(MemFs::new());
    fs.seed_file(l.themes_dir(), b"in the way");
    let trace = Arc::new(TraceFs::new(fs.clone()));
    let started = start_with(trace.clone(), Access::ReadOnly { why: "only looking".into() }, KeystoreKind::File)
        .expect("a read-only start");
    assert!(trace.log().iter().all(|t| !t.writes()));
    let about = started.about(&l.themes_dir());
    assert_eq!(kinds(&about), [("unrepaired", Severity::Warning)], "{about:#?}");
    assert!(about[0].message.contains("in the way of a folder"), "{}", about[0].message);
    // A writing start cannot make its folders: start-up's caller goes
    // read-only (profile.rs, start_up).
    let refused = start_with(fs.clone(), Access::Write, KeystoreKind::File).map(drop).expect_err("the folders");
    assert!(refused.contains("in the way of a folder"), "{refused}");
}

#[test]
fn the_sweep_removes_only_dead_writers_temps() {
    let l = layout();
    let fs = profile(&[]);
    let own = std::process::id();
    let other = own.wrapping_add(1);
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for dir in [
        l.config.clone(),
        l.themes_dir(),
        l.data.clone(),
        l.wallet_dir(),
        l.exports_dir(),
        l.permissions_dir(),
        l.site_data_dir(),
        l.trust_dir(),
        l.replay_logs_dir(),
        l.state.clone(),
        l.runtime.clone(),
    ] {
        let dead = dir.join(format!(".x.tmp-{other}-7"));
        let ours = dir.join(format!(".x.tmp-{own}-7"));
        fs.seed_file(&dead, b"dead");
        fs.seed_file(&ours, b"ours");
        removed.push(dead);
        kept.push(ours);
    }
    // Never swept: the keystore, where an unfinished save may hold a key's
    // only copy; the cache; backups; the migration's folder.
    let (hex, a) = wallet(4);
    let key_name = key_file_name(&wallet_key_name(&a));
    let stem = key_name.strip_suffix(".key").expect("a key file's name");
    for path in [
        l.keys_dir().join(format!(".{key_name}.tmp-{other}-1")),
        l.keys_dir().join(format!("{stem}.tmp")),
        l.content_dir().join("aa").join(format!(".x.tmp-{other}-1")),
        l.content_dir().join("aa/x.part"),
        l.backups(Class::Config).join("2026-01-01T00-00-00Z").join(format!(".settings.toml.tmp-{other}-1")),
        l.migration_dir().join(format!(".plan.json.tmp-{other}-1")),
    ] {
        fs.seed_file(&path, hex.as_bytes());
        kept.push(path);
    }
    // A linked settings.toml: its writes make their temporary files next to
    // the link's target.
    fs.seed_file("/dotfiles/settings.toml", &valid(Managed::Settings));
    fs.seed_symlink(Managed::Settings.path(&l), "/dotfiles/settings.toml");
    let linked_dead = PathBuf::from(format!("/dotfiles/.settings.toml.tmp-{other}-2"));
    let unrelated = PathBuf::from(format!("/dotfiles/.vimrc.tmp-{other}-2"));
    fs.seed_file(&linked_dead, b"dead");
    fs.seed_file(&unrelated, b"not F1R3Gaze's");
    removed.push(linked_dead);
    kept.push(unrelated);
    // A session that only reads (`f1r3gaze profile check`) names the same
    // files, and removes none.
    let looked = Arc::new(MemFs::clone(&fs));
    let before = looked.files();
    let trace = Arc::new(TraceFs::new(looked.clone()));
    let read_only = start_with(trace.clone(), Access::ReadOnly { why: "only looking".into() }, KeystoreKind::File)
        .expect("a read-only start");
    assert!(trace.log().iter().all(|t| !t.writes()), "{:#?}", trace.log());
    assert_eq!(looked.files(), before);
    let would: usize = read_only
        .events
        .iter()
        .map(|e| match e.kind {
            EventKind::Swept { removed } if e.message.contains("would be removed") => removed,
            _ => 0,
        })
        .sum();
    assert_eq!(would, removed.len(), "{:#?}", read_only.events);
    let fs = Arc::new(fs);
    let started = start(&fs);
    let files = fs.files();
    for path in &removed {
        assert!(!files.contains_key(path), "{} is swept", path.display());
    }
    for path in &kept {
        assert!(files.contains_key(path), "{} is kept", path.display());
    }
    assert_eq!(fs.pending_len(), 0, "every removal is durable");
    let swept: usize = started
        .events
        .iter()
        .map(|e| match e.kind {
            EventKind::Swept { removed } => removed,
            _ => 0,
        })
        .sum();
    assert_eq!(swept, removed.len());
}

#[test]
fn a_newer_state_file_is_left_alone() {
    let l = layout();
    let fs = profile(&[]);
    for m in [Managed::Window, Managed::Session, Managed::History] {
        fs.seed_file(m.path(&l), br#"{"version":9,"tabs":"a newer shape"}"#);
    }
    fs.seed_file(Managed::Marker.path(&l), br#"{"layout":2}"#);
    let before = fs.files();
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert_eq!(fs.files(), before, "nothing is rewritten or backed up");
    for (m, version) in [(Managed::Window, 9), (Managed::Session, 9), (Managed::History, 9), (Managed::Marker, 2)] {
        let about = started.about(&m.path(&l));
        assert_eq!(kinds(&about), [("newer", Severity::Notice)], "{}", m.name());
        assert!(
            matches!(about[0].kind, EventKind::Newer { version: v, known: 1 } if v == version),
            "{}: {:?}",
            m.name(),
            about[0].kind
        );
        assert!(started.reconciled.blocked.why(m).is_some(), "{}: nothing is saved to it", m.name());
    }
}

#[test]
fn a_link_to_nothing_is_left_alone() {
    let l = layout();
    let settings = Managed::Settings.path(&l);
    let fs = profile(&[(Managed::Settings, State::Missing)]);
    fs.seed_symlink(&settings, "/nowhere/settings.toml");
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert!(matches!(fs.kind(&settings), Ok(Kind::Symlink)), "the link is kept");
    assert!(fs.kind(Path::new("/nowhere")).is_err(), "nothing is made where it points");
    assert_eq!(kinds(&started.about(&settings)), [("unreadable", Severity::Warning)]);
    assert!(started.reconciled.blocked.why(Managed::Settings).is_some());
}

#[test]
fn a_file_that_cannot_be_backed_up_is_left_alone() {
    let l = layout();
    let settings = Managed::Settings.path(&l);
    let fs = profile(&[(Managed::Settings, State::Corrupt)]);
    fs.seed_file(l.backups(Class::Config), b"a file where the backups go");
    let fs = Arc::new(fs);
    let started = start(&fs);
    assert_eq!(fs.files()[&settings], corrupt(Managed::Settings), "never repaired without a backup");
    assert_eq!(kinds(&started.about(&settings)), [("unrepaired", Severity::Warning)]);
    assert!(started.reconciled.blocked.why(Managed::Settings).is_some());
}

#[test]
fn the_site_index_keeps_its_names() {
    let l = layout();
    let path = Managed::SiteIndex.path(&l);
    let cases: [(&[u8], &[u8], usize, usize); 5] = [
        (br#"["https://a.example","https://b.exa"#, br#"["https://a.example"]"#, 1, 1),
        (br#"["https://a.example",7,"https://b.example"]"#, br#"["https://a.example","https://b.example"]"#, 2, 1),
        (br#"["https://a.example""#, br#"["https://a.example"]"#, 1, 0),
        (br#"{"https://a.example":1}"#, b"[]", 0, 1),
        (b"\xff garbage", b"[]", 0, 1),
    ];
    for (damaged, kept, n, dropped) in cases {
        let fs = profile(&[]);
        fs.seed_file(&path, damaged);
        let fs = Arc::new(fs);
        let started = start(&fs);
        let what = String::from_utf8_lossy(damaged).into_owned();
        assert_eq!(fs.files()[&path], kept, "{what}");
        let about = started.about(&path);
        assert_eq!(about.len(), 1, "{what}");
        assert_eq!(about[0].kind, EventKind::Repaired { kept: n, dropped }, "{what}");
    }
}

#[test]
fn settings_rewrite_keeps_its_mode() {
    let l = layout();
    let settings = Managed::Settings.path(&l);
    let session = Managed::Session.path(&l);
    let fs = profile(&[(Managed::Settings, State::Corrupt), (Managed::Session, State::Corrupt)]);
    fs.set_mode(&settings, 0o644).expect("chmod");
    fs.set_mode(&session, 0o644).expect("chmod");
    let fs = Arc::new(fs);
    start(&fs);
    assert_eq!(fs.mode(&settings).expect("its mode"), Some(0o644), "a file the user opened up stays open");
    assert_eq!(fs.mode(&session).expect("its mode"), Some(0o600), "state is the owner's alone");
}
