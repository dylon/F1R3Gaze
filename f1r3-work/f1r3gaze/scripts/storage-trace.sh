#!/usr/bin/env bash
# storage-trace.sh — check real start-ups against F1R3Gaze's start-up model.
#
#   scripts/storage-trace.sh           # every trace: a crash at every 5th operation
#   scripts/storage-trace.sh --quick   # a crash at every 25th operation (CI)
#
# Options: --jar PATH (default $TLA2TOOLS or /usr/share/java/tla2tools.jar;
# it must be tla2tools 1.7.4, checked by its sha256), --out DIR (default
# target/scratch/storage-trace/<UTC time>).
#
# It runs crates/gaze-shell/tests/storage_trace.rs (feature storage-trace,
# without the window): start-up runs on an in-memory file system through a
# recording one, with crashes and power cuts, and every simulated restart is
# another process. Each run is put into the terms of
# docs/storage/tla/ProfileStartup.tla, and TLC checks it against
# ProfileStartupTrace.tla. Every trace must be accepted, and two mutant
# traces rejected. A rejected trace is reported with how many of its
# events the model could follow, and the first one it could not.
#
# Exit codes: 0 every trace as predicted; 1 a trace was not; 2 a tool is
# missing, or the jar is not the pinned one.
# Design and results: docs/storage/README.md ("Verifying storage") and
# docs/storage/ledger.md (S8).

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
JAR=${TLA2TOOLS:-/usr/share/java/tla2tools.jar}
# tla2tools.jar 1.7.4 (TLC 2.19 of 2024-08-08), as scripts/storage-model.sh
# and the CI model job pin it.
PINNED=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
OUT=""
QUICK=0

usage() {
    sed -n '2,23p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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
SUM=$(sha256sum "$JAR" | cut -d' ' -f1)
[[ $SUM == "$PINNED" ]] || { log "$JAR is not tla2tools 1.7.4 (sha256 $SUM)"; exit 2; }

OUT=${OUT:-$ROOT/target/scratch/storage-trace/$(date -u +%Y%m%dT%H%M%SZ)}
mkdir -p "$OUT/tmp"
OUT=$(cd -- "$OUT" && pwd)
# The cases' folders, TLC's files and every temporary file go under $OUT,
# never /tmp.
export F1R3GAZE_TLA2TOOLS=$JAR TMPDIR=$OUT/tmp
if ((QUICK)); then
    export F1R3GAZE_TRACE_QUICK=1
fi

if ((QUICK)); then
    log "traces in $OUT (quick)"
else
    log "traces in $OUT"
fi
cd "$ROOT"
set +e
cargo test --locked -p gaze-shell --no-default-features --features storage-trace \
    --test storage_trace -- --nocapture 2>&1 | tee "$OUT/test.log"
status=${PIPESTATUS[0]}
set -e
case $status in
0) log "every trace accepted, both mutants rejected (see $OUT/test.log)" ;;
*) log "a trace was not as predicted (see $OUT/test.log)"; exit 1 ;;
esac
