#!/usr/bin/env bash
# storage-bench.sh — how long F1R3Gaze takes to start, and what its start
# writes, before and after the five-root layout (docs/storage/README.md,
# "Verifying storage"; ledgers S1 and S15).
#
#   scripts/storage-bench.sh [--bin BIN] [--baseline BIN] [--out DIR]
#                            [--cpus LIST] [--runs N] [--quick]
#
#   --bin BIN       the build under test (default target/release/f1r3gaze)
#   --baseline BIN  the build before the layout (default the S1 rebuild,
#                   target/scratch/storage/s1/src/f1r3-work/f1r3gaze/target/release/f1r3gaze)
#   --out DIR       where everything goes (default target/scratch/storage/s15);
#                   it must not be near a real F1R3Gaze folder (exit 3)
#   --cpus LIST     the CPUs hyperfine and the runs are pinned to (default 2-9)
#   --runs N        timed runs per measurement (default 50 for a page, 100
#                   for the other commands)
#   --quick         5 runs, 5 pairs and 1 warm-up each: a check of the script
#                   itself
#
# Every run uses a --profile folder under DIR, in an environment isolated as
# the snapshot harness isolates it (scripts/lib/storage-isolation.bash),
# with HOME in DIR as well.
#
# Profiles:
#   fresh     removed before every run;
#   steady    made by one start of the same build, then reused;
#   old-3     the shape of a profile from before the layout: the settings
#             template of f6ee26a (which sets nothing), a user id, and a
#             workspace with one tab and four visits; restored before every
#             run;
#   old-full  old-3 with a palette, grants, a site store, a replay log, a
#             wallet export, a cache shard and a wallet key with its list.
# The baseline reads old-3 and old-full as its own profiles; the build under
# test moves them on every run (a migration). `moved` is the build under
# test's start on old-full after one migration.
#
# Commands: `--headless gaze://newtab --timeout 5` (a page: about 256 ms of
# it is waiting for the page to settle), `wallet list` (the whole engine and
# no page), and, for the build under test, `profile check` (it reads only;
# its exit 1, "a start would change something", is expected on fresh, old-3
# and old-full).
#
# hyperfine runs one build's starts back to back, then the other's, so on a
# machine whose load drifts the two blocks differ by the drift too (ledger
# S15, part 2, H-drift). Every before / after comparison is therefore also
# measured in alternating pairs (scripts/lib/storage-bench-pairs.py): 30
# pairs for a page and 100 for wallet list, after 3 pairs not kept, each
# pair's order alternating, each start prepared as hyperfine prepares it.
#
# Output in DIR:
#   machine.txt                   CPU, governor, EPP, boost, frequencies,
#                                 kernel, load, tools, the binaries' sha256
#   hyperfine/NAME.{json,md,log}  one per (build, command, profile)
#   strace/NAME.summary           strace -f -c of one start
#   strace/NAME.trace             strace -f -y -tt -T of one start (-T: the
#                                 time each call took)
#   pairs.tsv                     the alternating pairs: each pair's two wall
#                                 times
#   table.md                      every timing, the before/after comparisons
#                                 (hyperfine's, and the pairs' with the
#                                 Wilcoxon signed-rank test) and what each
#                                 start did to its profile
#                                 (scripts/lib/storage-bench-table.py)
# NAME is BUILD-COMMAND-PROFILE, with BUILD `before` or `after`.
#
# Exit codes: 0 done; 1 a step failed; 2 a tool or a binary is missing;
# 3 DIR is unsafe; 64 usage.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# Was a fixed range of lines (2-55): exactly the header, until the header
# changes, as ui-snapshots.sh's and resize-bench.sh's ranges went stale. Now
# the header's comment lines, up to the first empty line.
# usage() { sed -n '2,55p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
usage() { sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed -n 's/^# \{0,1\}//p'; }
log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { log "FAILED: $*"; exit 1; }

BIN=$ROOT/target/release/f1r3gaze
BASELINE=$ROOT/target/scratch/storage/s1/src/f1r3-work/f1r3gaze/target/release/f1r3gaze
OUT=$ROOT/target/scratch/storage/s15
CPUS=2-9
RUNS_PAGE=50
RUNS_OTHER=100
WARMUP=5
PAIRS_PAGE=30
PAIRS_OTHER=100
PAIRS_WARMUP=3
while (($#)); do
    case $1 in
    --bin | --baseline | --out | --cpus | --runs)
        (($# >= 2)) || { usage >&2; exit 64; }
        case $1 in
        --bin) BIN=$2 ;;
        --baseline) BASELINE=$2 ;;
        --out) OUT=$2 ;;
        --cpus) CPUS=$2 ;;
        --runs)
            [[ $2 =~ ^[1-9][0-9]*$ ]] || { log "--runs takes a positive number"; exit 64; }
            RUNS_PAGE=$2
            RUNS_OTHER=$2
            ;;
        esac
        shift 2
        ;;
    --quick)
        RUNS_PAGE=5
        RUNS_OTHER=5
        WARMUP=1
        PAIRS_PAGE=5
        PAIRS_OTHER=5
        PAIRS_WARMUP=1
        shift
        ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 64 ;;
    esac
done

[[ $(uname -s) == Linux ]] || { log "Linux only (strace, taskset and the cpufreq files)"; exit 64; }
for tool in hyperfine strace taskset jq python3 sha256sum; do
    command -v "$tool" >/dev/null || { log "$tool is missing"; exit 2; }
done
python3 -c 'import scipy' 2>/dev/null || { log "python3's scipy is missing (scripts/lib/storage-bench-table.py)"; exit 2; }
for b in "$BIN" "$BASELINE"; do
    [[ -x $b ]] || { log "binary not found or not executable: $b"; exit 2; }
done
BIN=$(cd -- "$(dirname -- "$BIN")" && pwd)/$(basename -- "$BIN")
BASELINE=$(cd -- "$(dirname -- "$BASELINE")" && pwd)/$(basename -- "$BASELINE")

# shellcheck source-path=SCRIPTDIR source=lib/storage-isolation.bash
source "$ROOT/scripts/lib/storage-isolation.bash"
storage_real_roots
mkdir -p -- "$OUT"
OUT=$(cd -- "$OUT" && pwd)
storage_guard "$OUT"
# Commands are handed to hyperfine without a shell, split at spaces.
[[ $OUT != *[[:space:]]* ]] || { log "--out must not contain white space: $OUT"; exit 64; }
mkdir -p -- "$OUT/hyperfine" "$OUT/strace" "$OUT/profiles" "$OUT/fixtures"
storage_isolate "$OUT/env"
export HOME=$OUT/env/home TMPDIR=$OUT/env/tmp
mkdir -p -- "$TMPDIR"
unset F1R3GAZE_PROFILE
storage_paths_self_test "$BIN" "$OUT/env" "$OUT/env/self-test"
rm -rf -- "$OUT/env/self-test"

# ── The machine ──────────────────────────────────────────────────────────
# Every CPU in LIST (a taskset list such as 2-9 or 2,4,6-8).
cpus_of() {
    local part from to
    local -a parts
    IFS=, read -r -a parts <<<"$1"
    for part in "${parts[@]}"; do
        if [[ $part =~ ^([0-9]+)-([0-9]+)$ ]]; then
            from=${BASH_REMATCH[1]}
            to=${BASH_REMATCH[2]}
            seq "$from" "$to"
        elif [[ $part =~ ^[0-9]+$ ]]; then
            printf '%s\n' "$part"
        else
            log "not a CPU list: $1"
            exit 64
        fi
    done
}
cpufreq() { cat "/sys/devices/system/cpu/cpu$1/cpufreq/$2" 2>/dev/null || printf -- '-'; }
machine() {
    local cpu
    printf 'date (UTC)   %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'cpu          %s\n' "$(grep -m1 '^model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')"
    printf 'kernel       %s\n' "$(uname -r)"
    # What an fsync costs depends on the file system and its options
    # (barriers, that is, the drive's cache flushes).
    printf 'file system  %s\n' "$(findmnt -T "$OUT" -n -o SOURCE,FSTYPE,OPTIONS 2>/dev/null || printf -- '-')"
    printf 'boost        %s\n' "$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || printf -- '-')"
    printf 'pinned to    %s\n' "$CPUS"
    printf 'cpu governor driver max-freq(kHz) epp\n'
    while read -r cpu; do
        printf '%-4s %s %s %s %s\n' "$cpu" "$(cpufreq "$cpu" scaling_governor)" "$(cpufreq "$cpu" scaling_driver)" \
            "$(cpufreq "$cpu" scaling_max_freq)" "$(cpufreq "$cpu" energy_performance_preference)"
    done < <(cpus_of "$CPUS")
    printf 'load         %s\n' "$(cut -d' ' -f1-3 /proc/loadavg)"
    printf 'busiest processes:\n'
    ps -eo pcpu=,comm= --sort=-pcpu | head -n 5 | sed 's/^/  /'
    printf 'hyperfine    %s\n' "$(hyperfine --version)"
    printf 'strace       %s\n' "$(strace -V | head -n1)"
    printf 'after        %s  %s\n' "$(sha256sum "$BIN" | cut -d' ' -f1)" "$BIN"
    printf 'before       %s  %s\n' "$(sha256sum "$BASELINE" | cut -d' ' -f1)" "$BASELINE"
    printf 'commit       %s (%s changed paths)\n' "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || printf -- '-')" \
        "$(git -C "$ROOT" status --porcelain 2>/dev/null | wc -l)"
    printf 'runs         page %s, other %s, warm-up %s\n' "$RUNS_PAGE" "$RUNS_OTHER" "$WARMUP"
    printf 'pairs        page %s, other %s, warm-up %s\n' "$PAIRS_PAGE" "$PAIRS_OTHER" "$PAIRS_WARMUP"
}
machine >"$OUT/machine.txt"
while read -r cpu; do
    [[ $(cpufreq "$cpu" scaling_governor) == performance ]] ||
        log "warning: CPU $cpu's governor is $(cpufreq "$cpu" scaling_governor), not performance (machine.txt)"
done < <(cpus_of "$CPUS")

# ── The old profiles ─────────────────────────────────────────────────────
FIX=$OUT/fixtures
rm -rf -- "$FIX"
mkdir -p -- "$FIX/old-3" "$FIX/keysrc/config"
printf '%s\n' \
    '# F1R3Gaze settings. Lists are comma-separated.' \
    '# home = gaze://newtab' \
    '# observers = https://observer-1.example, https://observer-2.example, https://observer-3.example' \
    '# validator = https://validator.example' \
    '# shard_id = root' \
    '# quorum = 2' \
    '# mirrors = https://cdn.example/blob/' \
    '# https_only = false' \
    '# Wallet balances, history and transfers (the Embers service F1R3Sky uses):' \
    '# embers_api = https://embers.example' \
    '# max_fee = 10000000' >"$FIX/old-3/settings.conf"
[[ $(sha256sum "$FIX/old-3/settings.conf" | cut -d' ' -f1) == 8130f83a71c5428f7dc1187bc63089d61c88d9302df6bd440631fadcafc939ff ]] ||
    fail "the f6ee26a template was not written byte for byte"
printf '5f1c2d3e4b5a69788796a5b4c3d2e1f0' >"$FIX/old-3/user-id"
NOW=$(date +%s)
jq -n --argjson now "$NOW" '{theme: "dark", sidebar_open: false, panel: "tabs", tree_tabs: false, active: 0,
    tabs: [{url: "https://example.org/", title: "Example", parent: null}],
    visits: [range(0; 4) | {url: ("https://example.org/" + tostring), title: ("Page " + tostring), at: ($now - .)}]}' \
    >"$FIX/old-3/workspace.json"
cp -a -- "$FIX/old-3" "$FIX/old-full" >/dev/null
# The wallet key, made by the build under test in a profile of its own; the
# key file's format is the same before and after the layout.
printf '%064d\n' 0 | tr 0 3 >"$FIX/key.hex"
printf '[shard]\nobservers = []\n' >"$FIX/keysrc/config/settings.toml"
ADDRESS=$("$BIN" --profile "$FIX/keysrc" wallet import "$FIX/key.hex" "Bench wallet" | tail -n1)
[[ -n $ADDRESS ]] || fail "the key could not be imported"
KEY=$(find "$FIX/keysrc/data/wallet/keys" -name '*.key' | head -n1)
[[ -n $KEY ]] || fail "the key file was not made"
O=$FIX/old-full
mkdir -p -- "$O/keys" "$O/store" "$O/logs" "$O/exports" "$O/cache/ab"
printf ':root { --gaze-bg: #101010; }\n' >"$O/palette.css"
: >"$O/grants.tsv"
cp -- "$KEY" "$O/keys/"
cp -- "$FIX/keysrc/data/wallet/wallets.tsv" "$O/wallets.tsv"
if [[ -f $FIX/keysrc/data/wallet/wallet-active ]]; then
    cp -- "$FIX/keysrc/data/wallet/wallet-active" "$O/wallet-active"
fi
printf 'store records' >"$O/store/$(printf '%064d' 0 | tr 0 d).gzs"
printf 'a replay log' >"$O/logs/tab-1-2.gzlog"
printf '{"an": "export"}' >"$O/exports/$ADDRESS.json"
printf 'a cached blob' >"$O/cache/ab/$(printf '%064d' 0 | tr 0 a)"
# hyperfine's --prepare for the old profiles: a fresh copy of the fixture.
RESTORE=$OUT/restore.sh
cat >"$RESTORE" <<'SH'
#!/usr/bin/env bash
# Replaces the profile $2 with a copy of the fixture $1 (storage-bench.sh).
set -euo pipefail
rm -rf -- "$2"
cp -a -- "$1" "$2"
SH
chmod +x "$RESTORE"

# ── Measuring ────────────────────────────────────────────────────────────
PAGE=(--headless gaze://newtab --timeout 5)
WALLET=(wallet list)
CHECK=(profile check)
CALLS=fsync,fdatasync,sync_file_range,rename,renameat,renameat2,link,linkat,unlink,unlinkat,rmdir,mkdir,mkdirat,openat,flock,ftruncate,chmod,fchmod,fchmodat

# bench NAME RUNS IGNORE PREPARE BIN ARG... — one hyperfine measurement.
# IGNORE is an exit code to accept or `-`; PREPARE a command run before
# every run (split at spaces) or `-`.
bench() {
    local name=$1 runs=$2 ignore=$3 prepare=$4
    shift 4
    local -a args=(-N --style basic --warmup "$WARMUP" --runs "$runs" --command-name "$name"
        --export-json "$OUT/hyperfine/$name.json" --export-markdown "$OUT/hyperfine/$name.md")
    [[ $ignore == - ]] || args+=("--ignore-failure=$ignore")
    [[ $prepare == - ]] || args+=(--prepare "$prepare")
    log "hyperfine $name"
    taskset -c "$CPUS" hyperfine "${args[@]}" "$*" >"$OUT/hyperfine/$name.log" 2>&1 ||
        fail "hyperfine $name (see $OUT/hyperfine/$name.log)"
}

# trace NAME PREPARE IGNORE BIN ARG... — strace of one start, twice: the
# counts (-c), then every call with its paths (-y). The start must exit 0,
# or IGNORE when that is not `-`.
trace() {
    local name=$1 prepare=$2 ignore=$3 status
    shift 3
    log "strace $name"
    # shellcheck disable=SC2086 # PREPARE is split at spaces, as hyperfine splits it.
    [[ $prepare == - ]] || $prepare
    status=0
    strace -f -c -o "$OUT/strace/$name.summary" -e trace="$CALLS" "$@" >/dev/null 2>&1 || status=$?
    [[ $status == 0 || $status == "$ignore" ]] || fail "strace -c $name: exit $status"
    # shellcheck disable=SC2086
    [[ $prepare == - ]] || $prepare
    status=0
    strace -f -y -tt -T -o "$OUT/strace/$name.trace" -e trace="$CALLS,write,pwrite64" "$@" >/dev/null 2>&1 || status=$?
    [[ $status == 0 || $status == "$ignore" ]] || fail "strace $name: exit $status"
}

# measure BUILD BIN COMMAND RUNS IGNORE PROFILE-KIND
measure() {
    local build=$1 bin=$2 command=$3 runs=$4 ignore=$5 kind=$6
    local -a argv
    case $command in
    page) argv=("${PAGE[@]}") ;;
    wallet) argv=("${WALLET[@]}") ;;
    check) argv=("${CHECK[@]}") ;;
    esac
    local name=$build-$command-$kind profile=$OUT/profiles/$build-$command-$kind prepare
    case $kind in
    fresh) prepare="rm -rf -- $profile" ;;
    steady | moved) prepare=- ;;
    old-3 | old-full) prepare="$RESTORE $FIX/$kind $profile" ;;
    esac
    case $kind in
    steady)
        rm -rf -- "$profile"
        "$bin" --profile "$profile" "${PAGE[@]}" >/dev/null 2>&1 || fail "$name: the first start"
        ;;
    moved)
        "$RESTORE" "$FIX/old-full" "$profile"
        "$bin" --profile "$profile" "${PAGE[@]}" >/dev/null 2>&1 || fail "$name: the migrating start"
        [[ -f $profile/MIGRATED.txt ]] || fail "$name: the old profile was not moved"
        ;;
    esac
    bench "$name" "$runs" "$ignore" "$prepare" "$bin" --profile "$profile" "${argv[@]}"
    trace "$name" "$prepare" "$ignore" "$bin" --profile "$profile" "${argv[@]}"
    printf '%s\t%s\n' "$name" "$profile" >>"$OUT/profiles.tsv"
}

: >"$OUT/profiles.tsv"
for kind in fresh steady old-3 old-full; do
    measure before "$BASELINE" page "$RUNS_PAGE" - "$kind"
    measure after "$BIN" page "$RUNS_PAGE" - "$kind"
    measure before "$BASELINE" wallet "$RUNS_OTHER" - "$kind"
    measure after "$BIN" wallet "$RUNS_OTHER" - "$kind"
done
measure after "$BIN" page "$RUNS_PAGE" - moved
measure after "$BIN" wallet "$RUNS_OTHER" - moved
measure after "$BIN" check "$RUNS_OTHER" - steady
measure after "$BIN" check "$RUNS_OTHER" - moved
for kind in fresh old-3 old-full; do
    measure after "$BIN" check "$RUNS_OTHER" 1 "$kind"
done
log "alternating pairs"
taskset -c "$CPUS" true || fail "taskset -c $CPUS"
python3 "$ROOT/scripts/lib/storage-bench-pairs.py" "$OUT" "$CPUS" "$BASELINE" "$BIN" "$PAIRS_PAGE" "$PAIRS_OTHER" "$PAIRS_WARMUP" ||
    fail "the alternating pairs (scripts/lib/storage-bench-pairs.py)"
machine >"$OUT/machine-after.txt"

python3 "$ROOT/scripts/lib/storage-bench-table.py" "$OUT" >"$OUT/table.md" || fail "the table (scripts/lib/storage-bench-table.py)"
log "done: $OUT/table.md"
