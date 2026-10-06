//! What the trace checks share (ledger S8): TLC on a case's trace, and why
//! a rejected trace was rejected. `storage_trace.rs` checks start-ups run on
//! `MemFs`; `crash_points.rs` checks the real binary, killed and started
//! again.
//!
//! These helpers moved here from `storage_trace.rs` when `crash_points.rs`
//! came to need them too (ledger S8, part 2).

use gaze_shell::profile::migrate::trace::{Case, Event, tla};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned tla2tools.jar.
pub fn jar() -> PathBuf {
    let jar = std::env::var_os("F1R3GAZE_TLA2TOOLS").expect(
        "F1R3GAZE_TLA2TOOLS names tla2tools.jar 1.7.4 (scripts/storage-trace.sh and storage-kill.sh set it)",
    );
    PathBuf::from(jar)
}

/// What happened, as JSON lines: the evidence a module was made from.
pub fn ndjson(events: &[Event]) -> String {
    let mut lines = String::with_capacity(160 * events.len());
    for event in events {
        match event {
            Event::Op(t) => lines.push_str(&t.json()),
            Event::ProcessCrash => lines.push_str(r#"{"op":"crash","kind":"process"}"#),
            Event::PowerCut { keep, .. } => {
                let kept: Vec<String> = keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i.to_string()).collect();
                lines.push_str(&format!(r#"{{"op":"crash","kind":"power","keep":[{}]}}"#, kept.join(",")));
            }
        }
        lines.push('\n');
    }
    lines
}

/// Whether TLC accepts `case`: its log is a behaviour of the model. The
/// output is kept in `dir/<name>`.
pub fn accepted(case: &Case<'_>, dir: &Path) -> Result<bool, String> {
    let at = dir.join(case.name);
    std::fs::create_dir_all(&at).map_err(|e| e.to_string())?;
    std::fs::write(at.join("events.ndjson"), ndjson(case.events)).map_err(|e| e.to_string())?;
    let (module, cfg) = tla(case).map_err(|e| format!("{}: {e}", case.name))?;
    let specs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/storage/tla");
    for spec in ["ProfileStartup.tla", "ProfileStartupTrace.tla"] {
        std::fs::copy(specs.join(spec), at.join(spec)).map_err(|e| format!("{spec}: {e}"))?;
    }
    let file = format!("Trace_{}", case.name);
    std::fs::write(at.join(format!("{file}.tla")), module).map_err(|e| e.to_string())?;
    std::fs::write(at.join(format!("{file}.cfg")), cfg).map_err(|e| e.to_string())?;
    // TLC unpacks its standard modules into java.io.tmpdir: one folder per
    // case, never /tmp, and never shared by TLCs running at once.
    match tlc(&at, &file, "TraceEnd")? {
        Some(accepted) => Ok(accepted),
        None => Err(format!("{}: TLC failed; see {}", case.name, at.join(format!("{file}.log")).display())),
    }
}

/// Why the case `name` was rejected: how far the model could follow it.
pub fn diagnose(name: &str, dir: &Path) -> String {
    let (at, file) = (dir.join(name), format!("Trace_{name}"));
    match module_log(&at, &file).and_then(|log| matched_prefix(&at, &file, &log)) {
        Ok(why) => format!("{name}: rejected: {why}"),
        Err(e) => format!("{name}: rejected (no diagnosis: {e})"),
    }
}

/// Runs TLC on `file` in `at`: `Some(true)` if it reports `invariant`
/// violated, `Some(false)` if it finds no violation, `None` if it fails.
pub fn tlc(at: &Path, file: &str, invariant: &str) -> Result<Option<bool>, String> {
    let output = Command::new("java")
        .args(["-Xmx2g", "-XX:+UseParallelGC"])
        .arg(format!("-Djava.io.tmpdir={}", at.display()))
        .arg("-cp")
        .arg(jar())
        .args(["tlc2.TLC", "-workers", "1", "-cleanup", "-metadir", &format!("states-{file}"), "-config"])
        .arg(format!("{file}.cfg"))
        .arg(format!("{file}.tla"))
        .current_dir(at)
        .output()
        .map_err(|e| format!("java: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    std::fs::write(at.join(format!("{file}.log")), &stdout).map_err(|e| e.to_string())?;
    Ok(match (output.status.code(), stdout.contains(&format!("Invariant {invariant} is violated"))) {
        (Some(12), true) => Some(true),
        (Some(0), false) => Some(false),
        _ => None,
    })
}

/// The events of the generated module, one per line, as written.
fn module_log(at: &Path, file: &str) -> Result<Vec<String>, String> {
    let module = std::fs::read_to_string(at.join(format!("{file}.tla"))).map_err(|e| e.to_string())?;
    Ok(module
        .lines()
        .skip_while(|line| !line.starts_with("TR_Log == <<"))
        .skip(1)
        .take_while(|line| *line != ">>")
        .map(|line| line.trim().trim_end_matches(',').to_string())
        .collect())
}

/// How many events of a rejected trace the model matched, found by asking
/// TLC whether `l` can pass each bound (a binary search), and the first
/// event it could not match.
fn matched_prefix(at: &Path, file: &str, log: &[String]) -> Result<String, String> {
    let cfg = std::fs::read_to_string(at.join(format!("{file}.cfg"))).map_err(|e| e.to_string())?;
    let reaches = |n: usize| -> Result<bool, String> {
        let probe = format!("Probe{n}");
        let module = format!("---- MODULE {probe} ----\nEXTENDS {file}\nProbe == l <= {n}\n====\n");
        std::fs::write(at.join(format!("{probe}.tla")), module).map_err(|e| e.to_string())?;
        std::fs::write(at.join(format!("{probe}.cfg")), cfg.replace("INVARIANT TraceEnd", "INVARIANT Probe"))
            .map_err(|e| e.to_string())?;
        tlc(at, &probe, "Probe")?.ok_or_else(|| format!("TLC failed on {probe}"))
    };
    // l starts at 1, so "l <= n" is violated once n events were matched.
    let (mut low, mut high) = (0usize, log.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        match reaches(mid)? {
            true => low = mid,
            false => high = mid - 1,
        }
    }
    let next = log.get(low).cloned().unwrap_or_else(|| "(the end)".into());
    Ok(format!("{low} of {} events matched; the next: {next}", log.len()))
}

/// Checks every case on every core; returns the rejected ones. `case_of`
/// describes a case for `trace::tla`.
pub fn check_all<T: Sync>(cases: &[T], dir: &Path, case_of: fn(&T) -> Case<'_>) -> Vec<String> {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = cases.len().div_ceil(cores).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|item| {
                            let case = case_of(item);
                            match accepted(&case, dir) {
                                Ok(true) => None,
                                Ok(false) => Some(diagnose(case.name, dir)),
                                Err(e) => Some(e),
                            }
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().expect("a checker")).collect()
    })
}
