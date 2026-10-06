//! Key custody (spec §9.1). A keystore holds named secp256k1 keys: the
//! wallets the user pays with (`wallet:<address>`), and anything else a
//! component needs to keep. A key leaves the keystore only when the user
//! exports a wallet, to a file only its owner can read.

use gaze_fs::{Fs, Perm, StdFs};
use k256::ecdsa::SigningKey;
use std::path::PathBuf;

pub fn fresh_key() -> Result<SigningKey, String> {
    loop {
        let mut b = [0u8; 32];
        getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
        if let Ok(k) = SigningKey::from_slice(&b) {
            return Ok(k);
        }
    }
}

pub trait Keystore: Send + Sync + 'static {
    fn load(&self, name: &str) -> Result<Option<SigningKey>, String>;
    fn store(&self, name: &str, key: &SigningKey) -> Result<(), String>;
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// Keys as hex files, readable only by the user on Unix from the moment they
/// exist, in a directory only the user can enter: the fallback where no OS
/// credential store is available (Linux without secret-service). File names
/// are hashes of the entry names. Writes and removals are synced, so a key
/// is never half-written and a removed key does not come back after a power
/// cut.
pub struct FileKeystore {
    dir: PathBuf,
}

/// The file name a [`FileKeystore`] keeps the key `name` under: the
/// BLAKE2b-256 hash of the name in lower-case hex, then `.key`.
pub fn key_file_name(name: &str) -> String {
    let h = k1ndl1ng_norm::hash::blake2b_256(name.as_bytes()).0;
    format!("{}.key", gaze_net::hex(&h))
}

/// The secret key in a key file: hex text, white space around it ignored.
pub fn parse_key_file(bytes: &[u8]) -> Result<SigningKey, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "corrupt key file: not UTF-8 text".to_string())?;
    let raw = gaze_net::unhex(text.trim()).ok_or("corrupt key file: not hexadecimal")?;
    SigningKey::from_slice(&raw).map_err(|e| format!("corrupt key file: {e}"))
}

impl FileKeystore {
    pub fn new(dir: impl Into<PathBuf>) -> FileKeystore {
        FileKeystore { dir: dir.into() }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(key_file_name(name))
    }
}

impl Keystore for FileKeystore {
    fn load(&self, name: &str) -> Result<Option<SigningKey>, String> {
        match StdFs.read(&self.path(name)) {
            Ok(bytes) => parse_key_file(&bytes).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn store(&self, name: &str, key: &SigningKey) -> Result<(), String> {
        gaze_fs::create_dir_durably(&StdFs, &self.dir).map_err(|e| e.to_string())?;
        let hex = gaze_net::hex(&key.to_bytes());
        gaze_fs::write_atomic(&StdFs, &self.path(name), hex.as_bytes(), Perm::Private)
            .map_err(|e| e.to_string())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        match StdFs.remove_file(&self.path(name)) {
            Ok(()) => StdFs.sync_dir(&self.dir).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Keys in the operating system's credential store (Keychain, Windows
/// Credential Manager).
#[cfg(feature = "os-keyring")]
pub struct OsKeystore {
    service: String,
}

#[cfg(feature = "os-keyring")]
impl OsKeystore {
    pub fn new(service: &str) -> OsKeystore {
        OsKeystore { service: service.into() }
    }
}

#[cfg(feature = "os-keyring")]
impl Keystore for OsKeystore {
    fn load(&self, name: &str) -> Result<Option<SigningKey>, String> {
        let e = keyring::Entry::new(&self.service, name).map_err(|e| e.to_string())?;
        match e.get_password() {
            Ok(t) => {
                let b = gaze_net::unhex(t.trim()).ok_or("corrupt key")?;
                SigningKey::from_slice(&b).map(Some).map_err(|e| e.to_string())
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }
    fn store(&self, name: &str, key: &SigningKey) -> Result<(), String> {
        let e = keyring::Entry::new(&self.service, name).map_err(|e| e.to_string())?;
        e.set_password(&gaze_net::hex(&key.to_bytes())).map_err(|e| e.to_string())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        let e = keyring::Entry::new(&self.service, name).map_err(|e| e.to_string())?;
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }
}

/// A keystore in memory, for tests and for profiles that must leave nothing
/// on disk.
#[derive(Default)]
pub struct MemKeystore(std::sync::Mutex<std::collections::BTreeMap<String, [u8; 32]>>);

impl Keystore for MemKeystore {
    fn load(&self, name: &str) -> Result<Option<SigningKey>, String> {
        let m = self.0.lock().map_err(|_| "poisoned")?;
        Ok(m.get(name).and_then(|b| SigningKey::from_slice(b).ok()))
    }
    fn store(&self, name: &str, key: &SigningKey) -> Result<(), String> {
        self.0.lock().map_err(|_| "poisoned")?.insert(name.to_string(), key.to_bytes().into());
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.lock().map_err(|_| "poisoned")?.remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_by_name() {
        let dir = gaze_fs::scratch_dir("gaze-keys").join("keys");
        let ks = FileKeystore::new(&dir);
        assert!(ks.load("wallet:a").unwrap().is_none());
        let k = fresh_key().unwrap();
        ks.store("wallet:a", &k).unwrap();
        assert_eq!(ks.load("wallet:a").unwrap().unwrap().to_bytes(), k.to_bytes());
        assert!(ks.load("wallet:b").unwrap().is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(ks.path("wallet:a")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "the key directory is the owner's alone");
        }
        ks.remove("wallet:a").unwrap();
        assert!(ks.load("wallet:a").unwrap().is_none());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(left.is_empty(), "no temporary file is left");
    }
}
