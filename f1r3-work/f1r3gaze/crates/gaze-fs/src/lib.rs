//! `gaze-fs` — how F1R3Gaze writes files so that a crash never costs data.
//!
//! Every write to the profile goes through this crate (a clippy lint,
//! `disallowed-methods` in `clippy.toml`, rejects `std::fs::write` and
//! `std::fs::rename` elsewhere). It provides:
//!
//! * [`Fs`]: the file-system operations, one system call each, and their
//!   real implementation [`StdFs`].
//! * Writes built from them, each following the order the start-up model
//!   proves safe (docs/storage/tla/ProfileStartup.tla):
//!   - [`write_atomic`]: replace a file's content;
//!   - [`write_new`]: create a file that must not exist yet;
//!   - [`publish_no_replace`]: give a file a free name;
//!   - [`copy_verified`] and [`move_no_replace`]: move a file, across file
//!     systems too;
//!   - [`create_dir_durably`]: make folders that survive a power cut;
//!   - [`sweep_temps`]: clean up after a crash.
//! * [`check_lines`]: salvage the usable lines of a damaged line-oriented
//!   file.
//! * [`scratch_dir`]: test directories kept off `/tmp`.
//! * `MemFs` (feature `testing`): an in-memory file system that models power
//!   cuts, so tests can crash a write at every step; `every_crash` and
//!   `every_two_crashes` enumerate those crashes.
//! * `TraceFs` (feature `trace`, part of `testing`): records every operation
//!   another `Fs` performs.
//!
//! The durability assumptions, in the words of the model:
//! - A1: a directory operation (create, link, rename, remove) is atomic.
//! - A2: it is durable only once its directory is synced.
//! - A3: a file's data is durable only once the file is synced.
//!
//! These hold on every journaling or copy-on-write file system: ext4, XFS,
//! btrfs, APFS, NTFS. They do not hold on FAT or exFAT, so a `--profile` on
//! such a volume is not crash-safe.

#![forbid(unsafe_code)]
// Tests write their fixtures directly (clippy.toml's disallowed-methods
// apply to the code they test).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod atomic;
mod fs;
mod lines;
mod scratch;

#[cfg(any(test, feature = "testing"))]
mod crash;
#[cfg(any(test, feature = "testing"))]
mod mem;
#[cfg(any(test, feature = "trace"))]
mod trace;

pub use atomic::{
    Perm, copy_verified, create_dir_durably, move_no_replace, parent, process_id,
    publish_no_replace, replace_unsynced, resolve_link, same_contents, stale_temps, stale_temps_of,
    sweep_temps, sweep_temps_of, temp_owner, temp_path, write_atomic, write_new,
};
#[cfg(any(test, feature = "testing"))]
pub use atomic::with_process_id;
pub use fs::{Fs, Kind, PRIVATE_DIR_MODE, PRIVATE_FILE_MODE, StdFs};
pub use lines::{LineCheck, check_lines};
pub use scratch::{scratch_base, scratch_dir};

#[cfg(any(test, feature = "testing"))]
pub use crash::{Crash, every_crash, every_two_crashes};
#[cfg(any(test, feature = "testing"))]
pub use mem::{GARBAGE, MemFs};
#[cfg(any(test, feature = "trace"))]
pub use trace::{TraceFs, Traced, fnv1a64};

#[cfg(test)]
mod tests;
