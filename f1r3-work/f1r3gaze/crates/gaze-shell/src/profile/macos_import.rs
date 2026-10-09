//! Copy an existing macOS profile into a fresh App Sandbox container.
//!
//! The selected source remains untouched. The complete copy is published
//! under the container's Application Support directory only after every
//! regular file has been copied and compared with the source.

use super::migrate::Marker;
use gaze_fs::{Fs, Perm, StdFs};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const STAGE_MARKER: &str = ".f1r3gaze-import-stage";
const STAGE_VALUE: &[u8] = b"F1R3Gaze macOS profile import v1\n";
const ROOTS: [&str; 3] = ["config", "data", "state"];

/// Import the directory that contains `config`, `data` and `state` into
/// the matching directory inside a *fresh* Store container. A target that
/// already exists is never merged or replaced.
pub fn import_existing(source: &Path, target: &Path) -> io::Result<()> {
    validate_source(source)?;
    let target_parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    gaze_fs::create_dir_durably(&StdFs, target_parent)?;
    let source_real = fs::canonicalize(source)?;
    let parent_real = fs::canonicalize(target_parent)?;
    if parent_real.starts_with(&source_real) || source_real.starts_with(&parent_real) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source and destination directories overlap",
        ));
    }
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no name"))?
        .to_string_lossy();
    let lock_path = target_parent.join(format!(".{name}.import.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).mode(0o600);
    #[cfg(target_os = "macos")]
    options.custom_flags(libc::O_NOFOLLOW);
    let lock = options.open(&lock_path)?;
    lock.try_lock()?;
    ensure_absent(target)?;

    let stage = target_parent.join(format!(".{name}.importing"));
    clear_previous_stage(&stage)?;
    gaze_fs::create_dir_durably(&StdFs, &stage)?;
    gaze_fs::write_atomic(
        &StdFs,
        &stage.join(STAGE_MARKER),
        STAGE_VALUE,
        Perm::Private,
    )?;

    let copied = (|| {
        for root in ROOTS {
            copy_tree(&source.join(root), &stage.join(root), root)?;
        }
        for root in ROOTS {
            verify_tree(&source.join(root), &stage.join(root), root)?;
        }
        publish_complete(&stage, target)?;
        fs::remove_file(target.join(STAGE_MARKER))?;
        StdFs.sync_dir(target)?;
        StdFs.sync_dir(target_parent)
    })();
    if let Err(error) = copied {
        if let Err(cleanup) = clear_previous_stage(&stage) {
            return Err(io::Error::other(format!(
                "{error}; incomplete import cleanup also failed: {cleanup}"
            )));
        }
        return Err(error);
    }
    drop(lock);
    Ok(())
}

/// Finish cleanup if the app stopped after publishing a complete import
/// but before it removed the staging marker from the published directory.
pub fn finish_completed_import(target: &Path) -> io::Result<()> {
    let marker = target.join(STAGE_MARKER);
    match fs::read(&marker) {
        Ok(value) if value == STAGE_VALUE => {
            fs::remove_file(&marker)?;
            StdFs.sync_dir(target)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unrecognized import marker: {}", marker.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_source(source: &Path) -> io::Result<()> {
    require_directory(source)?;
    for root in ROOTS {
        require_directory(&source.join(root))?;
    }
    let data = source.join("data");
    for transient in [".startup-busy", ".migration"] {
        ensure_absent(&data.join(transient)).map_err(|error| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("close the existing F1R3Gaze before importing: {error}"),
            )
        })?;
    }
    let marker = data.join("layout.json");
    require_file(&marker)?;
    Marker::read(&fs::read(&marker)?).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid source profile marker: {error}"),
        )
    })?;
    Ok(())
}

fn ensure_absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        )),
        Err(error) => Err(error),
    }
}

fn require_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a plain directory", path.display()),
        ))
    }
}

fn require_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a regular file", path.display()),
        ))
    }
}

fn clear_previous_stage(stage: &Path) -> io::Result<()> {
    match fs::symlink_metadata(stage) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "import staging path is not a directory: {}",
                    stage.display()
                ),
            ));
        }
        Ok(_) => {}
    }
    let marker = stage.join(STAGE_MARKER);
    match fs::read(&marker) {
        Ok(value) if value == STAGE_VALUE => {}
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && fs::read_dir(stage)?.next().is_none() => {
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("unrecognized import staging directory: {}", stage.display()),
            ));
        }
    }
    fs::remove_dir_all(stage)?;
    StdFs.sync_dir(stage.parent().expect("staging directory has a parent"))
}

fn included(root: &str, name: &OsString) -> bool {
    !(root == "state" && name == "runtime")
}

fn entries(path: &Path, root: &str) -> io::Result<Vec<OsString>> {
    let mut names = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.retain(|name| included(root, name));
    names.sort();
    Ok(names)
}

fn copy_tree(source: &Path, stage: &Path, root: &str) -> io::Result<()> {
    require_directory(source)?;
    gaze_fs::create_dir_durably(&StdFs, stage)?;
    for name in entries(source, root)? {
        let from = source.join(&name);
        let to = stage.join(&name);
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.file_type().is_dir() {
            copy_tree(&from, &to, "")?;
        } else if metadata.file_type().is_file() {
            gaze_fs::copy_verified(&StdFs, &from, &to)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "profile contains a link or special file: {}",
                    from.display()
                ),
            ));
        }
    }
    Ok(())
}

fn verify_tree(source: &Path, stage: &Path, root: &str) -> io::Result<()> {
    require_directory(source)?;
    require_directory(stage)?;
    let source_names = entries(source, root)?;
    let stage_names = entries(stage, "")?;
    if source_names != stage_names {
        return Err(io::Error::other(format!(
            "profile changed while copying: {}",
            source.display()
        )));
    }
    for name in source_names {
        let from = source.join(&name);
        let to = stage.join(&name);
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.file_type().is_dir() {
            verify_tree(&from, &to, "")?;
        } else if metadata.file_type().is_file() {
            require_file(&to)?;
            if !StdFs.same_contents(&from, &to)? {
                return Err(io::Error::other(format!(
                    "profile changed while copying: {}",
                    from.display()
                )));
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "profile contains a link or special file: {}",
                    from.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn publish_complete(stage: &Path, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(stage.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let to = CString::new(target.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // macOS's exclusive rename publishes the whole directory without ever
    // replacing a profile another instance may have created meanwhile.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn publish_complete(stage: &Path, target: &Path) -> io::Result<()> {
    // This module is compiled on non-macOS Unix only for its unit tests.
    ensure_absent(target)?;
    fs::rename(stage, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let base = gaze_fs::scratch_dir(name);
        let source = base.join("old/Library/Application Support/io.f1r3fly.f1r3gaze");
        let target = base.join("container/Data/Library/Application Support/io.f1r3fly.f1r3gaze");
        for root in ROOTS {
            fs::create_dir_all(source.join(root)).unwrap();
        }
        fs::write(
            source.join("data/layout.json"),
            Marker::repaired().to_bytes(),
        )
        .unwrap();
        fs::write(source.join("config/settings.toml"), b"theme = 'dark'\n").unwrap();
        fs::create_dir_all(source.join("data/wallet")).unwrap();
        fs::write(source.join("data/wallet/wallets.tsv"), b"wallet metadata\n").unwrap();
        fs::write(source.join("state/session.json"), b"session\n").unwrap();
        (source, target)
    }

    #[test]
    fn copies_profile_without_touching_source() {
        let (source, target) = fixture("gaze-macos-profile-import-copy");
        fs::create_dir_all(source.join("state/runtime")).unwrap();
        fs::write(source.join("state/runtime/old.lock"), b"stale").unwrap();
        import_existing(&source, &target).unwrap();
        for path in [
            "config/settings.toml",
            "data/layout.json",
            "data/wallet/wallets.tsv",
            "state/session.json",
        ] {
            assert_eq!(
                fs::read(source.join(path)).unwrap(),
                fs::read(target.join(path)).unwrap()
            );
        }
        assert!(!target.join("state/runtime").exists());
        assert!(!target.join(STAGE_MARKER).exists());
        assert!(
            import_existing(&source, &target).unwrap_err().kind() == io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn refuses_symlinks_and_clears_incomplete_stage() {
        let (source, target) = fixture("gaze-macos-profile-import-link");
        std::os::unix::fs::symlink(source.join("state/session.json"), source.join("state/link"))
            .unwrap();
        let error = import_existing(&source, &target).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!target.exists());
        assert!(
            !target
                .with_file_name(".io.f1r3fly.f1r3gaze.importing")
                .exists()
        );
        fs::remove_file(source.join("state/link")).unwrap();
        import_existing(&source, &target).unwrap();
    }

    #[test]
    fn refuses_busy_or_damaged_source() {
        let (source, target) = fixture("gaze-macos-profile-import-busy");
        fs::write(source.join("data/.startup-busy"), b"busy").unwrap();
        assert_eq!(
            import_existing(&source, &target).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        fs::remove_file(source.join("data/.startup-busy")).unwrap();
        fs::write(source.join("data/layout.json"), b"not json").unwrap();
        assert_eq!(
            import_existing(&source, &target).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!target.exists());
    }

    #[test]
    fn discards_a_recognized_stale_stage_before_retry() {
        let (source, target) = fixture("gaze-macos-profile-import-stale");
        let stage = target.with_file_name(".io.f1r3fly.f1r3gaze.importing");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join(STAGE_MARKER), STAGE_VALUE).unwrap();
        fs::write(stage.join("partial"), b"discard me").unwrap();
        import_existing(&source, &target).unwrap();
        assert!(!stage.exists());
        assert!(target.join("data/layout.json").exists());
    }

    #[test]
    fn cleans_the_marker_after_a_completed_publish() {
        let (_, target) = fixture("gaze-macos-profile-import-completed");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(STAGE_MARKER), STAGE_VALUE).unwrap();
        finish_completed_import(&target).unwrap();
        assert!(!target.join(STAGE_MARKER).exists());
        finish_completed_import(&target).unwrap();
    }
}
