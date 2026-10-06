//! The engine of an open profile (ledger S14): each file at its place in the
//! profile's roots, and nothing written that the session must not write.

use super::*;
use crate::profile::Locking;
use crate::profile::layout::Class;
use crate::profile::report::EventKind;
use crate::test_support::ScratchProfile;
use gaze_fs::StdFs;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A session that only reads.
fn read_only(profile: &ScratchProfile) -> Rc<Engine> {
    Engine::open(profile.open_with(Arc::new(StdFs), Locking::ReadOnly("only looking")))
}

/// Every file under `dir`.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(names) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in names.flatten() {
        let path = entry.path();
        match path.is_dir() {
            true => files.extend(walk(&path)),
            false => files.push(path),
        }
    }
    files
}

#[test]
fn observers_are_loopback_only_on_this_machine() {
    for (observers, loopback) in [
        (&[][..], true),
        (&["http://localhost:40403"][..], true),
        (&["http://LOCALHOST:40403/"][..], true),
        (&["http://node.localhost:40403"][..], true),
        (&["http://localhost.:40403"][..], true),
        (&["http://127.0.0.1:40403", "http://127.9.9.9"][..], true),
        (&["http://[::1]:40403"][..], true),
        (&["http://localhost:40403", "https://observer.example"][..], false),
        (&["https://observer.example"][..], false),
        (&["http://10.0.0.1"][..], false),
        (&["http://localhost.example"][..], false),
        (&["http://[::2]"][..], false),
        (&["not an address"][..], false),
    ] {
        let observers: Vec<String> = observers.iter().map(|o| o.to_string()).collect();
        assert_eq!(observers_are_loopback(&observers), loopback, "{observers:?}");
    }
}

#[test]
fn the_engine_uses_the_reconciled_user_id() {
    let profile = ScratchProfile::new("engine-user-id");
    let first = profile.engine();
    let id = first.bridge.cfg.user.clone();
    assert_eq!(id.len(), 32, "{id}");
    let written = std::fs::read_to_string(profile.layout().user_id_file()).expect("user-id");
    assert_eq!(written.trim(), id, "the id start-up wrote");
    drop(first);
    assert_eq!(profile.engine().bridge.cfg.user, id, "read again, never made anew");
}

#[test]
fn blocked_wallets_refuse_changes() {
    let profile = ScratchProfile::new("engine-wallets-read-only");
    let engine = read_only(&profile);
    let refused = engine.wallets.create("Savings").expect_err("a read-only session");
    assert!(refused.contains("only looking"), "{refused}");
    assert!(!profile.layout().wallet_dir().join("wallets.tsv").exists(), "the list is not written");
    let keys = std::fs::read_dir(profile.layout().keys_dir()).map(Iterator::count).unwrap_or(0);
    assert_eq!(keys, 0, "and no key is stored first");
}

/// A wallet list start-up could not read is never replaced, even in a
/// session that may write.
#[cfg(unix)]
#[test]
fn an_unreadable_wallet_list_is_never_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let profile = ScratchProfile::new("engine-wallets-unreadable");
    drop(profile.engine());
    let list = profile.layout().wallet_dir().join("wallets.tsv");
    std::fs::create_dir_all(list.parent().expect("wallet/")).expect("wallet/");
    std::fs::write(&list, "").expect("an empty list");
    std::fs::set_permissions(&list, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read(&list).is_ok() {
        // Root reads anything: nothing to check.
        std::fs::set_permissions(&list, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        return;
    }
    let engine = profile.engine();
    let created = engine.wallets.create("Savings");
    std::fs::set_permissions(&list, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    assert!(created.is_err(), "the list is blocked");
    assert_eq!(std::fs::read(&list).expect("read"), b"", "left as it was");
}

#[test]
fn blocked_grants_are_never_saved() {
    let profile = ScratchProfile::new("engine-grants-read-only");
    let engine = read_only(&profile);
    let site = Site::of_url("https://a.example/").expect("a site");
    let refused = engine.broker.borrow_mut().forget(&site).expect_err("not saved");
    assert!(refused.contains("only looking"), "{refused}");
    assert!(!profile.layout().grants_file().exists());
}

/// A site index start-up could not read is never replaced: a store opened
/// later does not write it.
#[cfg(unix)]
#[test]
fn an_unreadable_site_index_is_never_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let profile = ScratchProfile::new("engine-site-index-unreadable");
    drop(profile.engine());
    let index = profile.layout().origins_file();
    std::fs::create_dir_all(index.parent().expect("site-data/")).expect("site-data/");
    std::fs::write(&index, "[\"https://old.example\"]").expect("an index");
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read(&index).is_ok() {
        // Root reads anything: nothing to check.
        std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        return;
    }
    let engine = profile.engine();
    let site = Site::of_url("https://a.example/").expect("a site");
    let opened = engine.store_for(&site).map(drop);
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    opened.expect("the store opens");
    assert_eq!(std::fs::read(&index).expect("read"), b"[\"https://old.example\"]", "never replaced");
}

#[test]
fn a_damaged_store_is_backed_up_when_it_opens() {
    let profile = ScratchProfile::new("engine-store-salvage");
    let site = Site::of_url("https://a.example/").expect("a site");
    let path = path_for(&profile.layout().site_data_dir(), site.as_str());
    {
        let engine = profile.engine();
        let store = engine.store_for(&site).expect("a store");
        store
            .borrow_mut()
            .serve(&[Norm::str("set"), Norm::str("k"), Norm::str("v")])
            .expect("a write");
    }
    let mut bytes = std::fs::read(&path).expect("the store");
    let good = bytes.len();
    bytes.extend_from_slice(b"\x07torn");
    std::fs::write(&path, &bytes).expect("damage the store");
    let engine = profile.engine();
    engine.store_for(&site).expect("it opens, cut back");
    assert_eq!(std::fs::read(&path).expect("the store").len(), good, "cut back to its records");
    let copies: Vec<PathBuf> = walk(&profile.layout().backups(Class::Data))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "gzs"))
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(std::fs::read(&copies[0]).expect("the copy"), bytes, "the whole file is kept");
    assert!(
        engine.profile.report.events().iter().any(|e| matches!(e.kind, EventKind::Salvaged { .. })),
        "and reported"
    );
}

#[test]
fn site_data_is_not_opened_read_only() {
    let profile = ScratchProfile::new("engine-store-read-only");
    let engine = read_only(&profile);
    let site = Site::of_url("https://a.example/").expect("a site");
    let refused = engine.store_for(&site).map(drop).expect_err("read only");
    assert!(refused.contains("only looking"), "{refused}");
    assert!(engine.clear_store("https://a.example/").is_err());
    assert!(!profile.layout().origins_file().exists());
}

#[test]
fn a_read_only_engine_never_fills_the_content_cache() {
    let profile = ScratchProfile::new("engine-cache-read-only");
    let engine = read_only(&profile);
    assert_eq!(engine.blobs.cache.read_only_reason(), Some("only looking"));
    assert!(engine.blobs.cache.clear().is_err());
}

/// A test profile is never an old single-folder one: its first open makes
/// a fresh profile, and moves nothing (`ScratchProfile` writes
/// `config/settings.toml`, never `settings.conf`).
#[test]
fn a_scratch_profile_is_never_an_old_one() {
    let profile = ScratchProfile::new("engine-scratch-fresh");
    let engine = profile.engine();
    assert_eq!(engine.profile.migration(), &crate::profile::migrate::Migration::Fresh);
    let root = profile.layout().config.parent().expect("the root").to_path_buf();
    assert!(!root.join("MIGRATED.txt").exists());
    assert!(engine.settings.shard.observers.is_empty(), "no event thread dials a node");
}

/// The freshness records: in memory for a shard on this machine; in the
/// trust file otherwise, and enforced but never written when the session
/// only reads.
#[test]
fn freshness_records_are_kept_where_the_shard_says() {
    use gaze_shard::FreshnessLog;
    let record = |log: &dyn FreshnessLog| {
        let mut all = BTreeMap::new();
        all.insert("rho:id:a".to_string(), 7);
        log.record(&all)
    };
    // The scratch profile's shard has no observers: a development shard.
    let local = ScratchProfile::new("engine-freshness-local");
    let profile = local.open_on(Arc::new(StdFs));
    record(freshness_log(&profile).as_ref()).expect("kept in memory");
    assert!(!local.layout().trust_file().exists(), "nothing written for a local shard");
    // A shard elsewhere: the records are in the trust file.
    let remote = ScratchProfile::new("engine-freshness-remote");
    std::fs::write(remote.layout().settings_file(), "[shard]\nobservers = [\"https://observer.example\"]\n")
        .expect("settings.toml");
    let profile = remote.open_on(Arc::new(StdFs));
    record(freshness_log(&profile).as_ref()).expect("saved");
    let saved = std::fs::read(remote.layout().trust_file()).expect("the trust file");
    assert!(String::from_utf8_lossy(&saved).contains("root\trho:id:a\t7"));
    drop(profile);
    // Read only: loaded and enforced, never written.
    let profile = remote.open_with(Arc::new(StdFs), Locking::ReadOnly("only looking"));
    let log = freshness_log(&profile);
    assert_eq!(log.load().get("rho:id:a"), Some(&7), "enforced");
    let mut higher = BTreeMap::new();
    higher.insert("rho:id:a".to_string(), 9);
    assert!(log.record(&higher).is_err(), "not saved");
    assert_eq!(std::fs::read(remote.layout().trust_file()).expect("the trust file"), saved);
}
