#!/usr/bin/env bash
# storage-kill.sh — kill the real f1r3gaze during start-up, and check what
# happened against F1R3Gaze's start-up model.
#
#   scripts/storage-kill.sh           # a kill at every 5th storage operation
#   scripts/storage-kill.sh --quick   # a kill at every 25th (CI)
#
# Options: --jar PATH (default $TLA2TOOLS or /usr/share/java/tla2tools.jar;
# it must be tla2tools 1.7.4, checked by its sha256), --out DIR (default
# target/scratch/storage-kill/<UTC time>).
#
# It runs crates/gaze-shell/tests/crash_points.rs (features crash-points and
# storage-trace, without the window). The binary opens an old profile laid
# flat at a portable root, aborts at its k-th storage operation
# (F1R3GAZE_CRASH_AT=k), and records each operation it finished
# (F1R3GAZE_STORAGE_TRACE). A clean start then finishes the move. Every
# trace, both processes' records with the crash between them, must be a
# behaviour of docs/storage/tla/ProfileStartup.tla; nothing may be lost.
#
# Exit codes: 0 every kill as predicted; 1 a kill was not; 2 a tool is
# missing, or the jar is not the pinned one.
# Design and results: docs/storage/README.md ("Verifying storage") and
# docs/storage/ledger.md (S8, part 2).

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
JAR=${TLA2TOOLS:-/usr/share/java/tla2tools.jar}
# tla2tools.jar 1.7.4 (TLC 2.19 of 2024-08-08), as scripts/storage-model.sh,
# scripts/storage-trace.sh and the CI pin it.
PINNED=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
OUT=""
QUICK=0

usage() {
    sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 64
}

while (($#)); do
    case $1 in
    --jar) JAR=${2:?--jar needs a path}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --quick) QUICK=1 ;;
    -h | --help) usage ;;
    *) printf 'unknown option: %s\n' "$1" >&2; usage ;;
    esac
    shift
done

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

command -v java >/dev/null || { log "java is missing"; exit 2; }
command -v cargo >/dev/null || { log "cargo is missing"; exit 2; }
[[ -f $JAR ]] || { log "tla2tools.jar is missing: $JAR"; exit 2; }
JAR=$(cd -- "$(dirname -- "$JAR")" && pwd)/$(basename -- "$JAR")
if command -v sha256sum >/dev/null; then
    SUM=$(sha256sum "$JAR" | cut -d' ' -f1)
else
    SUM=$(shasum -a 256 "$JAR" | cut -d' ' -f1)
fi
[[ $SUM == "$PINNED" ]] || { log "$JAR is not tla2tools 1.7.4 (sha256 $SUM)"; exit 2; }

OUT=${OUT:-$ROOT/target/scratch/storage-kill/$(date -u +%Y%m%dT%H%M%SZ)}
mkdir -p "$OUT/tmp"
OUT=$(cd -- "$OUT" && pwd)
# The runs' folders, TLC's files and every temporary file go under $OUT,
# never /tmp.
export F1R3GAZE_TLA2TOOLS=$JAR TMPDIR=$OUT/tmp
if ((QUICK)); then
    export F1R3GAZE_TRACE_QUICK=1
fi
# Every kill aborts the binary: no core files (systemd-coredump keeps them
# otherwise). Git Bash on Windows may not support it; Windows Error
# Reporting's dialog is turned off by the CI instead.
ulimit -c 0 2>/dev/null || true

if ((QUICK)); then
    log "kills in $OUT (quick)"
else
    log "kills in $OUT"
fi
cd "$ROOT"
set +e
cargo test --locked -p gaze-shell --no-default-features --features crash-points,storage-trace \
    --test crash_points -- --nocapture 2>&1 | tee "$OUT/test.log"
status=${PIPESTATUS[0]}
set -e
case $status in
0) log "every kill was a behaviour of the model, and lost nothing (see $OUT/test.log)" ;;
*) log "a kill was not as predicted (see $OUT/test.log)"; exit 1 ;;
esac
