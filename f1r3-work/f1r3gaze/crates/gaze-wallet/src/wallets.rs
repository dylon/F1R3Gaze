//! The profile's wallets. Keys live in the keystore (`wallet:<address>`);
//! `wallets.tsv` lists addresses and labels, and `wallet-active` names the
//! wallet that pays. The first wallet created or imported becomes active.
//!
//! Both files are written atomically, readable by their owner only, in the
//! profile's `data/wallet` directory. A list that exists but cannot be read
//! is never overwritten: every change is refused until it can be (see
//! [`Wallets::blocked`]); start-up has already saved and repaired a list it
//! could read but not wholly parse.

use crate::address::Address;
use crate::embers::{Embers, WalletState};
use crate::file;
use gaze_fs::{Fs, LineCheck, Perm, StdFs};
use gaze_shard::keys::fresh_key;
use gaze_shard::{Keystore, Payer};
use k256::ecdsa::SigningKey;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub address: Address,
    pub label: String,
}

struct Inner {
    list: Vec<Entry>,
    active: Option<Address>,
    balance: Option<(Instant, Address, u64)>,
}

pub struct Wallets {
    dir: PathBuf,
    keys: Arc<dyn Keystore>,
    pub embers: Option<Embers>,
    inner: Mutex<Inner>,
    /// Why the wallets cannot be changed, if they cannot: a list that exists
    /// but could not be read, or a read-only session.
    blocked: Option<String>,
}

fn clean(label: &str) -> String {
    label.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// The name a wallet's key is kept under in the keystore.
pub fn wallet_key_name(a: &Address) -> String {
    format!("wallet:{a}")
}

/// A wallet's line in the list: its address, a tab, its label.
pub fn entry_line(e: &Entry) -> String {
    format!("{}\t{}\n", e.address, e.label)
}

/// The wallets of a list's usable lines, in order.
pub fn parse_list(bytes: &[u8]) -> Vec<Entry> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| parse_entry(line).ok())
        .collect()
}

/// The file listing the wallets, in the wallet directory.
pub const LIST_FILE: &str = "wallets.tsv";
/// The file naming the wallet that pays, in the wallet directory.
pub const ACTIVE_FILE: &str = "wallet-active";

/// A line of the wallet list: an address, then a tab and the label.
fn parse_entry(line: &str) -> Result<Entry, String> {
    let (address, label) = line.split_once('\t').unwrap_or((line, ""));
    Ok(Entry {
        address: Address::parse(address)?,
        label: label.to_string(),
    })
}

/// Which lines of a wallet list can be used.
pub fn check_list(bytes: &[u8]) -> LineCheck {
    gaze_fs::check_lines(bytes, |line| parse_entry(line).map(drop))
}

/// Whether `wallet-active` names an address.
pub fn check_active(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    Address::parse(text.trim()).map(drop)
}

/// A file's content, `None` if it does not exist; an error if it cannot be
/// read.
fn read_optional(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    match StdFs.read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Wallets {
    /// The wallets listed in `dir`. A list or active-wallet file that cannot
    /// be read leaves the wallets [blocked](Wallets::blocked).
    pub fn open(dir: impl Into<PathBuf>, keys: Arc<dyn Keystore>, embers: Option<Embers>) -> Wallets {
        let dir = dir.into();
        let mut blocked = None;
        let list = match read_optional(&dir.join(LIST_FILE)) {
            Ok(bytes) => parse_list(&bytes.unwrap_or_default()),
            Err(why) => {
                blocked = Some(why);
                Vec::new()
            }
        };
        let active = match read_optional(&dir.join(ACTIVE_FILE)) {
            Ok(bytes) => bytes
                .and_then(|b| Address::parse(&String::from_utf8_lossy(&b)).ok())
                .filter(|a| list.iter().any(|e| &e.address == a)),
            Err(why) => {
                blocked.get_or_insert(why);
                None
            }
        };
        Wallets {
            dir,
            keys,
            embers,
            inner: Mutex::new(Inner {
                list,
                active,
                balance: None,
            }),
            blocked,
        }
    }

    /// The same wallets, refusing every change: for a session that does not
    /// hold the profile's lock.
    pub fn read_only(mut self, why: &str) -> Wallets {
        self.blocked.get_or_insert_with(|| why.to_string());
        self
    }

    /// Why the wallets cannot be changed, if they cannot.
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    fn writable(&self) -> Result<(), String> {
        match &self.blocked {
            Some(why) => Err(format!("the wallets cannot be changed: {why}")),
            None => Ok(()),
        }
    }

    fn save(&self, i: &Inner) -> Result<(), String> {
        self.writable()?;
        gaze_fs::create_dir_durably(&StdFs, &self.dir).map_err(|e| e.to_string())?;
        let body: String = i.list.iter().map(entry_line).collect();
        gaze_fs::write_atomic(&StdFs, &self.dir.join(LIST_FILE), body.as_bytes(), Perm::Private)
            .map_err(|e| e.to_string())?;
        let active = self.dir.join(ACTIVE_FILE);
        match &i.active {
            Some(a) => gaze_fs::write_atomic(&StdFs, &active, a.as_str().as_bytes(), Perm::Private)
                .map_err(|e| e.to_string()),
            None => match StdFs.remove_file(&active) {
                Ok(()) => StdFs.sync_dir(&self.dir).map_err(|e| e.to_string()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
        }
    }

    fn add(&self, key: &SigningKey, label: &str) -> Result<Address, String> {
        // Refused before the key is stored, so a refusal leaves no key behind.
        self.writable()?;
        let a = Address::from_key(key.verifying_key());
        self.keys.store(&wallet_key_name(&a), key)?;
        let mut i = self.inner.lock().map_err(|_| "poisoned")?;
        match i.list.iter_mut().find(|e| e.address == a) {
            Some(e) if !label.is_empty() => e.label = clean(label),
            Some(_) => {}
            None => i.list.push(Entry {
                address: a.clone(),
                label: clean(label),
            }),
        }
        if i.active.is_none() {
            i.active = Some(a.clone());
        }
        self.save(&i)?;
        Ok(a)
    }

    pub fn create(&self, label: &str) -> Result<Address, String> {
        self.add(&fresh_key()?, label)
    }

    /// Import a wallet file saved by F1R3Sky (or a bare hex key).
    pub fn import(&self, text: &str, label: &str) -> Result<Address, String> {
        self.add(&file::deserialize(text)?, label)
    }

    /// The wallet file for `a`, loadable by F1R3Sky.
    pub fn export(&self, a: &Address) -> Result<String, String> {
        Ok(file::serialize(&self.key(a)?))
    }

    pub fn key(&self, a: &Address) -> Result<SigningKey, String> {
        self.keys.load(&wallet_key_name(a))?.ok_or_else(|| format!("no key for wallet {a}"))
    }

    /// Forget a wallet: its key is deleted from the keystore. Export it
    /// first if its funds matter.
    pub fn remove(&self, a: &Address) -> Result<(), String> {
        self.writable()?;
        self.keys.remove(&wallet_key_name(a))?;
        let mut i = self.inner.lock().map_err(|_| "poisoned")?;
        i.list.retain(|e| &e.address != a);
        if i.active.as_ref() == Some(a) {
            i.active = i.list.first().map(|e| e.address.clone());
        }
        i.balance = None;
        self.save(&i)
    }

    pub fn set_active(&self, a: &Address) -> Result<(), String> {
        self.writable()?;
        let mut i = self.inner.lock().map_err(|_| "poisoned")?;
        if !i.list.iter().any(|e| &e.address == a) {
            return Err(format!("no wallet {a}"));
        }
        i.active = Some(a.clone());
        i.balance = None;
        self.save(&i)
    }

    pub fn list(&self) -> Vec<(Entry, bool)> {
        let i = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => return Vec::new(),
        };
        i.list.iter().map(|e| (e.clone(), Some(&e.address) == i.active.as_ref())).collect()
    }

    pub fn active(&self) -> Option<Address> {
        self.inner.lock().ok()?.active.clone()
    }

    pub fn state(&self, a: &Address) -> Result<WalletState, String> {
        let e = self.embers.as_ref().ok_or("no Embers service is configured (settings: embers_api)")?;
        let s = e.state(a)?;
        if let Ok(mut i) = self.inner.lock() {
            i.balance = Some((Instant::now(), a.clone(), s.balance));
        }
        Ok(s)
    }

    /// Transfer from wallet `from`. The prepared contract is checked before
    /// the key signs it. Returns the deploy id.
    pub fn transfer(&self, from: &Address, to: &Address, amount: i64, description: Option<&str>) -> Result<String, String> {
        let e = self.embers.as_ref().ok_or("no Embers service is configured (settings: embers_api)")?;
        let id = e.transfer(&self.key(from)?, to, amount, description)?;
        if let Ok(mut i) = self.inner.lock() {
            i.balance = None;
        }
        Ok(id)
    }
}

impl Payer for Wallets {
    fn payer(&self) -> Result<(SigningKey, String), String> {
        let a = self
            .active()
            .ok_or("no wallet to pay with: create or import one (Wallet panel, or `f1r3gaze wallet new`)")?;
        Ok((self.key(&a)?, a.as_str().to_string()))
    }

    fn balance(&self) -> Option<u64> {
        let a = self.active()?;
        if let Ok(i) = self.inner.lock()
            && let Some((t, ba, v)) = &i.balance
            && ba == &a
            && t.elapsed() < Duration::from_secs(15)
        {
            return Some(*v);
        }
        self.state(&a).ok().map(|s| s.balance)
    }
}
