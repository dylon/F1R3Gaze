//! Wallets listed again from their key files.
//!
//! With the file keystore, a wallet's key is in `keys/<hash>.key`, where the
//! hash names `wallet:<address>` and the address is derived from the key
//! itself. So a key file proves which wallet it holds. If the list
//! (`wallets.tsv`) is lost or damaged, start-up lists each such wallet
//! again, labelled "(recovered)". Key files are only read: never written,
//! copied or removed. Recovery never chooses the wallet that pays.

use crate::address::Address;
use crate::wallets::{Entry, wallet_key_name};
use gaze_fs::{Fs, Kind};
use gaze_shard::keys::{key_file_name, parse_key_file};
use std::io;
use std::path::{Path, PathBuf};

/// The label of a wallet listed again from its key.
pub const RECOVERED_LABEL: &str = "(recovered)";

/// What a key file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyFile {
    /// The key of the wallet with this address, under the wallet's name.
    Wallet(Address),
    /// A valid key kept under another name: not a wallet's.
    Other,
    /// Not a key: the reason.
    Corrupt(String),
    /// It cannot be read: the error.
    Unreadable(String),
}

/// The keystore folder, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyScan {
    /// Every `<64 hex>.key` file, by name.
    pub keys: Vec<(PathBuf, KeyFile)>,
    /// Unfinished saves that hold the only copy of a wallet's key: a
    /// temporary file whose key is the wallet's for its name, while the key
    /// file itself is missing.
    pub stranded: Vec<(PathBuf, Address)>,
}

/// Whether `stem` is 64 lower-case hexadecimal digits.
fn hash_stem(stem: &str) -> bool {
    stem.len() == 64 && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The stem (`<64 hex>`) an unfinished save of a key file was for: an older
/// F1R3Gaze's `<stem>.tmp`, or gaze-fs's `.<stem>.key.tmp-<pid>-<n>`.
fn temp_stem(name: &str) -> Option<&str> {
    if let Some(stem) = name.strip_suffix(".tmp")
        && hash_stem(stem)
    {
        return Some(stem);
    }
    let rest = name.strip_prefix('.')?;
    let (stem, tail) = rest.split_once(".key.tmp-")?;
    let (pid, n) = tail.split_once('-')?;
    match hash_stem(stem) && pid.parse::<u32>().is_ok() && n.parse::<u64>().is_ok() {
        true => Some(stem),
        false => None,
    }
}

/// The wallet whose key `bytes` hold, if they hold a key that the file
/// named `file` (`<stem>.key`) would keep for that wallet.
fn wallet_in(bytes: &[u8], file: &str) -> Result<Option<Address>, String> {
    let key = parse_key_file(bytes)?;
    let address = Address::from_key(key.verifying_key());
    Ok(match key_file_name(&wallet_key_name(&address)) == file {
        true => Some(address),
        false => None,
    })
}

/// Reads the keystore folder `dir`. A missing folder holds no keys.
pub fn scan_keys(fs: &dyn Fs, dir: &Path) -> io::Result<KeyScan> {
    let mut names: Vec<String> = match fs.list(dir) {
        Ok(names) => names.into_iter().map(|n| n.to_string_lossy().into_owned()).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(KeyScan::default()),
        Err(e) => return Err(e),
    };
    names.sort();
    let mut scan = KeyScan {
        keys: Vec::with_capacity(names.len()),
        stranded: Vec::new(),
    };
    for name in &names {
        let path = dir.join(name);
        match fs.kind(&path) {
            Ok(Kind::File | Kind::Symlink) => {}
            _ => continue,
        }
        if let Some(stem) = name.strip_suffix(".key")
            && hash_stem(stem)
        {
            let what = match fs.read(&path) {
                Err(e) => KeyFile::Unreadable(e.to_string()),
                Ok(bytes) => match wallet_in(&bytes, name) {
                    Ok(Some(address)) => KeyFile::Wallet(address),
                    Ok(None) => KeyFile::Other,
                    Err(why) => KeyFile::Corrupt(why),
                },
            };
            scan.keys.push((path, what));
            continue;
        }
        if let Some(stem) = temp_stem(name) {
            let file = format!("{stem}.key");
            if names.contains(&file) {
                continue;
            }
            if let Ok(bytes) = fs.read(&path)
                && let Ok(Some(address)) = wallet_in(&bytes, &file)
            {
                scan.stranded.push((path, address));
            }
        }
    }
    Ok(scan)
}

/// An entry for each wallet whose key is in `scan` but which `listed` does
/// not name, in the order of their key files, labelled [`RECOVERED_LABEL`].
pub fn recover_entries(scan: &KeyScan, listed: &[Entry]) -> Vec<Entry> {
    let mut recovered: Vec<Entry> = Vec::new();
    for (_, what) in &scan.keys {
        if let KeyFile::Wallet(address) = what
            && !listed.iter().any(|e| &e.address == address)
            && !recovered.iter().any(|e| &e.address == address)
        {
            recovered.push(Entry {
                address: address.clone(),
                label: RECOVERED_LABEL.to_string(),
            });
        }
    }
    recovered
}

/// The listed wallet whose key the file named `file` would hold.
pub fn wallet_of_key_file<'a>(file: &str, listed: &'a [Entry]) -> Option<&'a Entry> {
    listed.iter().find(|e| key_file_name(&wallet_key_name(&e.address)) == file)
}

#[cfg(test)]
mod tests;
