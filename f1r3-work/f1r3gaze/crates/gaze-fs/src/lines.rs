//! Salvaging line-oriented files (`grants.tsv`, `wallets.tsv`,
//! `freshness.tsv`): keep every usable line exactly as it was, and say which
//! lines were dropped and why.

/// The result of [`check_lines`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineCheck {
    /// The usable lines and the blank ones, byte for byte, each with its own
    /// line ending.
    pub kept: Vec<u8>,
    /// The other lines: their number, counting from 1, and why each was
    /// dropped.
    pub dropped: Vec<(usize, String)>,
}

impl LineCheck {
    /// Whether every line was usable, so the file needs no repair.
    pub fn is_clean(&self) -> bool {
        self.dropped.is_empty()
    }
}

/// Sorts the lines of `bytes` into usable and dropped. A line is dropped if
/// it is not UTF-8 or `usable` rejects it; blank lines are always kept. The
/// line ending (`\n` or `\r\n`) is not part of what `usable` sees.
pub fn check_lines(bytes: &[u8], usable: impl Fn(&str) -> Result<(), String>) -> LineCheck {
    let mut check = LineCheck {
        kept: Vec::with_capacity(bytes.len()),
        dropped: Vec::new(),
    };
    for (index, line) in bytes.split_inclusive(|&b| b == b'\n').enumerate() {
        let text = line.strip_suffix(b"\n").unwrap_or(line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        match std::str::from_utf8(text) {
            Err(_) => check.dropped.push((index + 1, "not UTF-8".into())),
            Ok(text) if text.trim().is_empty() => check.kept.extend_from_slice(line),
            Ok(text) => match usable(text) {
                Ok(()) => check.kept.extend_from_slice(line),
                Err(why) => check.dropped.push((index + 1, why)),
            },
        }
    }
    check
}
