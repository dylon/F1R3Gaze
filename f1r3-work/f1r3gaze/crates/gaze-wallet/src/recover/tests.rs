use super::*;
use gaze_fs::MemFs;
use k256::ecdsa::SigningKey;

const KEYS: &str = "/data/wallet/keys";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_slice(&[byte; 32]).expect("a valid secret")
}

fn address(key: &SigningKey) -> Address {
    Address::from_key(key.verifying_key())
}

fn hex_of(key: &SigningKey) -> String {
    gaze_net::hex(&key.to_bytes())
}

/// The path the file keystore keeps `key` under as a wallet's key.
fn wallet_file(key: &SigningKey) -> String {
    format!("{KEYS}/{}", key_file_name(&wallet_key_name(&address(key))))
}

#[test]
fn key_files_are_told_apart() {
    let fs = MemFs::new();
    let (a, b) = (key(1), key(2));
    fs.seed_file(wallet_file(&a), hex_of(&a).as_bytes());
    // A valid key under a name that is not its wallet's: a session's key.
    let other = format!("{KEYS}/{}", key_file_name("session:example"));
    fs.seed_file(&other, hex_of(&b).as_bytes());
    let corrupt = format!("{KEYS}/{}.key", "f".repeat(64));
    fs.seed_file(&corrupt, b"not a key");
    let dangling = format!("{KEYS}/{}.key", "e".repeat(64));
    fs.seed_symlink(&dangling, "/nowhere");
    fs.seed_file(format!("{KEYS}/notes.txt"), b"ignored");
    fs.seed_file(format!("{KEYS}/{}.KEY", "a".repeat(64)), b"not lower-case: ignored");
    let scan = scan_keys(&fs, Path::new(KEYS)).expect("scan");
    let mut got: Vec<(String, KeyFile)> = scan
        .keys
        .iter()
        .map(|(p, k)| (p.to_string_lossy().into_owned(), k.clone()))
        .collect();
    got.sort_by(|x, y| x.0.cmp(&y.0));
    let mut wanted = vec![
        (wallet_file(&a), KeyFile::Wallet(address(&a))),
        (other, KeyFile::Other),
        (corrupt, KeyFile::Corrupt("corrupt key file: not hexadecimal".into())),
    ];
    wanted.sort_by(|x, y| x.0.cmp(&y.0));
    let (dangling_seen, rest): (Vec<_>, Vec<_>) = got.into_iter().partition(|(p, _)| *p == dangling);
    assert_eq!(rest, wanted);
    assert!(matches!(dangling_seen.as_slice(), [(_, KeyFile::Unreadable(_))]), "{dangling_seen:?}");
    assert!(scan.stranded.is_empty());
    assert_eq!(scan_keys(&fs, Path::new("/no/such/folder")).expect("a missing folder holds no keys"), KeyScan::default());
}

#[test]
fn unfinished_saves_holding_the_only_key_are_found() {
    let fs = MemFs::new();
    let (c, d, e) = (key(3), key(4), key(5));
    let stem = |k: &SigningKey| key_file_name(&wallet_key_name(&address(k))).trim_end_matches(".key").to_string();
    // An older F1R3Gaze's temporary name, with no key file beside it.
    fs.seed_file(format!("{KEYS}/{}.tmp", stem(&c)), hex_of(&c).as_bytes());
    // gaze-fs's temporary name, with no key file beside it.
    fs.seed_file(format!("{KEYS}/.{}.key.tmp-4242-0", stem(&d)), hex_of(&d).as_bytes());
    // A temporary file whose key file exists: not stranded.
    fs.seed_file(wallet_file(&e), hex_of(&e).as_bytes());
    fs.seed_file(format!("{KEYS}/.{}.key.tmp-4242-1", stem(&e)), hex_of(&e).as_bytes());
    // A temporary file holding another wallet's key than its name says.
    fs.seed_file(format!("{KEYS}/{}.tmp", "0".repeat(64)), hex_of(&c).as_bytes());
    let scan = scan_keys(&fs, Path::new(KEYS)).expect("scan");
    let mut stranded: Vec<Address> = scan.stranded.iter().map(|(_, a)| a.clone()).collect();
    stranded.sort();
    let mut wanted = vec![address(&c), address(&d)];
    wanted.sort();
    assert_eq!(stranded, wanted);
    assert_eq!(recover_entries(&scan, &[]).len(), 1, "a stranded key is reported, not recovered");
}

#[test]
fn only_unlisted_wallets_are_recovered() {
    let fs = MemFs::new();
    let (a, f) = (key(1), key(6));
    fs.seed_file(wallet_file(&a), hex_of(&a).as_bytes());
    fs.seed_file(wallet_file(&f), hex_of(&f).as_bytes());
    let scan = scan_keys(&fs, Path::new(KEYS)).expect("scan");
    let listed = [Entry {
        address: address(&a),
        label: "savings".into(),
    }];
    assert_eq!(
        recover_entries(&scan, &listed),
        [Entry {
            address: address(&f),
            label: RECOVERED_LABEL.into(),
        }]
    );
    let file = key_file_name(&wallet_key_name(&address(&a)));
    assert_eq!(wallet_of_key_file(&file, &listed).map(|e| e.label.as_str()), Some("savings"));
    assert_eq!(wallet_of_key_file(&format!("{}.key", "0".repeat(64)), &listed), None);
}
