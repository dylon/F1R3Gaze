#!/usr/bin/env bash
# storage-model.sh — model-check F1R3Gaze's start-up protocol with TLC.
#
#   scripts/storage-model.sh           # SANY, then every run below
#   scripts/storage-model.sh --quick   # SANY, liveness, faults and controls (CI)
#
# Options: --workers N (default 16), --heap SIZE (default 16g, the JVM's
# -Xmx), --memory SIZE (default 24G, the systemd scope's MemoryMax),
# --jar PATH (default $TLA2TOOLS or /usr/share/java/tla2tools.jar),
# --out DIR (default target/scratch/storage-model/<UTC time>), --no-scope
# (run TLC directly, without systemd-run; CI uses it).
#
# Runs, all on a copy of docs/storage/tla in --out, so TLC's states/
# directories and trace files never land in docs/:
#   deep      MCProfileStartup.cfg, two crashes: every invariant holds.
#   live      MCProfileStartupLive.cfg, one crash: Termination holds too.
#   wide      MCProfileStartupWide.cfg, four items: the same verdicts.
#   ntfs      MCProfileStartupNtfs.cfg, the deep run under Windows semantics
#             (W1, W2): every invariant but ReadyIsDurable, and Termination.
#   faults    the deep config with Fault set to each deliberate defect:
#             TLC must exit 12 and name the invariant the defect breaks.
#   hazards   the two barrier faults narrowed to the hazard each stands
#             for: TLC must exit 12 on the named invariant.
#   controls  two faults that must pass, which show why the deep config
#             needs power loss and two crashes.
#
# Exit codes: 0 every run as predicted; 1 a run was not; 2 a tool is missing.
# Design and results: docs/storage/README.md ("The formal model") and
# docs/storage/ledger.md (S2).

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
WORKERS=16
HEAP=16g
MEMORY=24G
JAR=${TLA2TOOLS:-/usr/share/java/tla2tools.jar}
OUT=""
QUICK=0
SCOPE=1

usage() {
    sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 64
}

while (($#)); do
    case $1 in
    --workers) WORKERS=${2:?--workers needs a number}; shift ;;
    --heap) HEAP=${2:?--heap needs a size}; shift ;;
    --memory) MEMORY=${2:?--memory needs a size}; shift ;;
    --jar) JAR=${2:?--jar needs a path}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --quick) QUICK=1 ;;
    --no-scope) SCOPE=0 ;;
    -h | --help) usage ;;
    *) printf 'unknown option: %s\n' "$1" >&2; usage ;;
    esac
    shift
done

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

command -v java >/dev/null || { log "java is missing"; exit 2; }
[[ -f $JAR ]] || { log "tla2tools.jar is missing: $JAR"; exit 2; }
# TLC runs inside $OUT, so a relative --jar is resolved here, once.
JAR=$(cd -- "$(dirname -- "$JAR")" && pwd)/$(basename -- "$JAR")
if ((SCOPE)); then
    command -v systemd-run >/dev/null || { log "systemd-run is missing (use --no-scope)"; exit 2; }
fi

OUT=${OUT:-$ROOT/target/scratch/storage-model/$(date -u +%Y%m%dT%H%M%SZ)}
mkdir -p "$OUT"
OUT=$(cd -- "$OUT" && pwd)
cp "$ROOT"/docs/storage/tla/*.tla "$ROOT"/docs/storage/tla/*.cfg "$OUT"/ >/dev/null
cd "$OUT"
JAVA_OPTS=(-Xmx"$HEAP" -XX:+UseParallelGC -Djava.io.tmpdir="$OUT")

# Run TLC in a resource-limited scope, so a large state space cannot make
# the machine unresponsive.
tlc() {
    local name=$1 cfg=$2
    local cmd=(java "${JAVA_OPTS[@]}" -cp "$JAR" tlc2.TLC -workers "$WORKERS"
        -coverage 1 -cleanup -metadir "$OUT/states-$name" -config "$cfg" MCProfileStartup.tla)
    if ((SCOPE)); then
        systemd-run --user --scope --quiet -p MemoryMax="$MEMORY" -p MemorySwapMax=0 \
            -p CPUQuota=$((WORKERS * 100))% -- "${cmd[@]}"
    else
        "${cmd[@]}"
    fi
}

FAILED=0
summary=()

# A run that must find no error.
expect_pass() {
    local name=$1 cfg=$2 status=0
    log "$name: $cfg"
    tlc "$name" "$cfg" >"$name.log" 2>&1 || status=$?
    local states
    states=$(grep -E 'distinct states found' "$name.log" | tail -1 || true)
    if ((status == 0)) && grep -q 'Model checking completed. No error has been found' "$name.log"; then
        summary+=("PASS  $name  exit 0  ${states:-}")
    else
        summary+=("FAIL  $name  exit $status, expected 0 (see $OUT/$name.log)")
        FAILED=1
    fi
}

# A run that must fail with exit 12 on the named invariant.
expect_violation() {
    local name=$1 cfg=$2 invariant=$3 status=0
    log "$name: $cfg (expect $invariant)"
    tlc "$name" "$cfg" >"$name.log" 2>&1 || status=$?
    local found
    found=$(grep -oE 'Invariant [A-Za-z]+ is violated' "$name.log" | head -1 || true)
    local depth
    depth=$(grep -cE '^State [0-9]+:' "$name.log" || true)
    if ((status == 12)) && [[ $found == "Invariant $invariant is violated" ]]; then
        summary+=("PASS  $name  exit 12  $found  (trace of $depth states)")
    else
        summary+=("FAIL  $name  exit $status, ${found:-no invariant named}, expected exit 12 on $invariant (see $OUT/$name.log)")
        FAILED=1
    fi
}

# A copy of a config (BASE, default the deep one) with Fault set, one more
# constant optionally set ("PowerLoss = FALSE"), and one invariant
# optionally left out.
BASE=MCProfileStartup.cfg
fault_cfg() {
    local fault=$1 setting=${2:-} drop=${3:-}
    local cfg="fault-${BASE%.cfg}-$fault${setting:+-${setting%% *}}${drop:+-without-$drop}.cfg"
    sed "s/Fault = \"none\"/Fault = \"$fault\"/" "$BASE" >"$cfg"
    if [[ -n $setting ]]; then
        local key=${setting%% =*}
        sed -i "s/^\( *\)$key = .*/\1$setting/" "$cfg"
    fi
    if [[ -n $drop ]]; then
        sed -i "/^ *$drop\$/d" "$cfg"
    fi
    printf '%s' "$cfg"
}

log "SANY"
java "${JAVA_OPTS[@]}" -cp "$JAR" tla2sany.SANY MCProfileStartup.tla >sany.log 2>&1 || {
    log "SANY failed (see $OUT/sany.log)"
    exit 1
}
grep -qiE '\*\*\* ?(errors|parse error)|Semantic errors|Lexical error' sany.log && {
    log "SANY reported errors (see $OUT/sany.log)"
    exit 1
}

if ((!QUICK)); then
    expect_pass deep MCProfileStartup.cfg
    expect_pass wide MCProfileStartupWide.cfg
fi
expect_pass ntfs MCProfileStartupNtfs.cfg
expect_pass live MCProfileStartupLive.cfg

# Each deliberate defect, and the invariant TLC finds broken first (the
# shortest counterexample; docs/storage/ledger.md, S2).
declare -A BREAKS=(
    [regenerate-before-backup]=CorruptKept
    [replace-destination]=NoOverwrite
    [marker-first]=MarkerImpliesComplete
    [no-fsync-before-publish]=PublishedComplete
    [no-dir-fsync-before-unlink]=NoLoss
    [resume-stale-temp]=PublishedComplete
    [no-start-barrier]=NoLoss
)
for fault in regenerate-before-backup replace-destination marker-first \
    no-fsync-before-publish no-dir-fsync-before-unlink resume-stale-temp \
    no-start-barrier; do
    expect_violation "fault-$fault" "$(fault_cfg "$fault")" "${BREAKS[$fault]}"
done

# Hazards behind the two barrier faults, each isolated:
# - fsyncing only the found copy (instead of every directory) lets a marker
#   become durable before the last retire did;
# - without the barrier, one crash already leaves earlier operations pending
#   when start-up reports ready.
# settle-only breaks two invariants at one depth, and which one TLC reports
# first depends on how its workers are scheduled (ledger S4, part 2). Each is
# checked on its own, with the other left out of the configuration.
expect_violation fault-settle-only \
    "$(fault_cfg settle-only '' MarkerImpliesComplete)" ReadyIsDurable
expect_violation hazard-settle-only-marker \
    "$(fault_cfg settle-only '' ReadyIsDurable)" MarkerImpliesComplete
expect_violation hazard-no-barrier-one-crash-pending \
    "$(fault_cfg no-start-barrier 'MaxCrashes = 1')" ReadyIsDurable

# Windows: without committing a file's new name (W2), a move across volumes
# can lose its item, since each volume keeps its own prefix.
BASE=MCProfileStartupNtfs.cfg
expect_violation fault-ntfs-no-commit-name "$(fault_cfg no-commit-name)" NoLoss
BASE=MCProfileStartup.cfg

# Controls: a missing fsync is invisible without power loss, and without the
# barrier the data itself survives any single crash: losing data needs two.
expect_pass control-no-fsync-without-power-loss "$(fault_cfg no-fsync-before-publish 'PowerLoss = FALSE')"
expect_pass control-no-barrier-one-crash-data-safe \
    "$(fault_cfg no-start-barrier 'MaxCrashes = 1' ReadyIsDurable)"

printf '%s\n' "${summary[@]}" | tee summary.txt
log "results in $OUT"
exit "$FAILED"
