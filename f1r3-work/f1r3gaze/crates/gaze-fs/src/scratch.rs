//! Scratch directories for tests, kept off `/tmp`, which is often RAM
//! (tmpfs).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A fresh, empty directory for one test, `<base>/<name>-<pid>`, where
/// `<base>` is [`scratch_base`] of `$TMPDIR` and the test binary. Anything
/// left there by an earlier run with the same pid is removed first.
// A test's scratch folder holds nothing that must survive a power cut.
#[allow(clippy::disallowed_methods)]
pub fn scratch_dir(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("the path of the test binary");
    let base = scratch_base(std::env::var_os("TMPDIR").as_deref(), &exe, |dir| {
        dir.join("CACHEDIR.TAG").is_file()
    });
    let dir = base.join(format!("{name}-{}", std::process::id()));
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("clear the scratch directory {}: {e}", dir.display()),
    }
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Where scratch directories go: `$TMPDIR/f1r3gaze-tests` when `TMPDIR` is
/// an absolute path, else `<target>/scratch`, where `<target>` is the
/// nearest ancestor of the test binary that `is_target_dir` recognises
/// (Cargo marks its target directory with `CACHEDIR.TAG`), else the binary's
/// own directory.
pub fn scratch_base(
    tmpdir: Option<&OsStr>,
    exe: &Path,
    is_target_dir: impl Fn(&Path) -> bool,
) -> PathBuf {
    match tmpdir.map(Path::new).filter(|tmp| tmp.is_absolute()) {
        Some(tmp) => tmp.join("f1r3gaze-tests"),
        None => exe
            .ancestors()
            .skip(1)
            .find(|dir| is_target_dir(dir))
            .or_else(|| exe.parent())
            .map(|dir| dir.join("scratch"))
            .unwrap_or_else(|| PathBuf::from("scratch")),
    }
}
