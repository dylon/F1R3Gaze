#!/usr/bin/env bash
# storage-smoke.sh — a first start makes a private skeleton, and a second
# writes nothing (docs/storage/README.md, "Start-up"; ledger S14).
#
#   scripts/storage-smoke.sh BIN DIR
#
# BIN is an f1r3gaze binary; DIR a folder of the run's own (it must not be
# near a real F1R3Gaze folder: exit 3). The profile is DIR/profile, a
# portable root; DIR/env holds the isolated environment
# (scripts/lib/storage-isolation.bash).
#
# 1. A first start: `--headless gaze://newtab --click "#lamp"`, which must
#    light the lamp.
# 2. The skeleton (README §9.3) and the default files exist.
# 3. On Unix: every folder is 0700 and every file 0600; the runtime lock
#    also has the sticky bit on Linux (01600).
# 4. Every file but the lock files ({data,runtime}/instance.{lock,pid}) is
#    recorded: path, size, mode, inode, modification time and sha256.
# 5. A second start; the record must be the same.
# 6. `profile check` finds nothing to change (exit 0).
#
# Runs on Linux, macOS and Windows (Git Bash). Exit codes: 0 as described;
# 1 a step failed; 2 a tool or the binary is missing; 3 DIR is unsafe;
# 64 usage.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
(($# == 2)) || { sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 64; }
BIN=$1
DIR=$2

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { log "FAILED: $*"; exit 1; }

[[ -x $BIN ]] || { log "binary not found or not executable: $BIN"; exit 2; }
BIN=$(cd -- "$(dirname -- "$BIN")" && pwd)/$(basename -- "$BIN")
case $(uname -s) in
Linux) OS=linux ;;
Darwin) OS=macos ;;
*) OS=windows ;;
esac

# shellcheck source-path=SCRIPTDIR source=lib/storage-isolation.bash
source "$ROOT/scripts/lib/storage-isolation.bash"
storage_real_roots
mkdir -p -- "$DIR"
DIR=$(cd -- "$DIR" && pwd)
storage_guard "$DIR"
PROFILE=$DIR/profile
rm -rf -- "$PROFILE"
storage_isolate "$DIR/env"

# What a file is, one line: path, size, mode, inode, modification time,
# sha256.
describe() {
    local path=$1 meta sum
    case $OS in
    macos) meta=$(stat -f '%z %Lp %i %m' "$path") ;;
    *) meta=$(stat -c '%s %a %i %Y' "$path") ;;
    esac
    if command -v sha256sum >/dev/null; then
        sum=$(sha256sum "$path" | cut -d' ' -f1)
    else
        sum=$(shasum -a 256 "$path" | cut -d' ' -f1)
    fi
    printf '%s %s %s\n' "${path#"$PROFILE"/}" "$meta" "$sum"
}

# A file's permission bits, in octal.
mode_of() {
    if [[ $OS == macos ]]; then
        stat -f '%Lp' "$1"
    else
        stat -c '%a' "$1"
    fi
}

# Every file but the lock files, sorted.
record() {
    local path
    find "$PROFILE" -type f | LC_ALL=C sort | while IFS= read -r path; do
        case ${path#"$PROFILE"/} in
        data/instance.lock | data/instance.pid | runtime/instance.lock | runtime/instance.pid) ;;
        *) describe "$path" ;;
        esac
    done
}

start() {
    "$BIN" --profile "$PROFILE" --headless gaze://newtab --click "#lamp" >"$DIR/run-$1.txt" 2>"$DIR/run-$1.err" ||
        fail "start $1 exited $? (see $DIR/run-$1.err)"
    grep -q 'id="lamp" class="lit"' "$DIR/run-$1.txt" || fail "start $1 did not light the lamp (see $DIR/run-$1.txt)"
}

log "start 1 in $PROFILE"
start 1

for folder in config config/themes data cache/content runtime; do
    [[ -d $PROFILE/$folder ]] || fail "the skeleton has no $folder/"
done
for file in config/settings.toml config/settings.toml.example config/themes/default-dark.css.example \
    config/themes/default-light.css.example data/layout.json data/user-id state/window.json \
    state/session.json state/history.json; do
    [[ -f $PROFILE/$file ]] || fail "start-up wrote no $file"
done
[[ ! -e $PROFILE/data/.startup-busy ]] || fail "the busy flag was left behind"

if [[ $OS != windows ]]; then
    while IFS= read -r folder; do
        mode=$(mode_of "$folder")
        [[ $mode == 700 ]] || fail "$folder is $mode, not 700"
    done < <(find "$PROFILE" -type d)
    while IFS= read -r file; do
        mode=$(mode_of "$file")
        want=600
        [[ $OS == linux && $file == "$PROFILE/runtime/instance.lock" ]] && want=1600
        [[ $mode == "$want" ]] || fail "$file is $mode, not $want"
    done < <(find "$PROFILE" -type f)
fi

record >"$DIR/after-1.txt"
log "start 2"
start 2
record >"$DIR/after-2.txt"
diff -u "$DIR/after-1.txt" "$DIR/after-2.txt" >"$DIR/second-start.diff" ||
    fail "the second start wrote something (see $DIR/second-start.diff)"

"$BIN" --profile "$PROFILE" profile check >"$DIR/check.txt" 2>&1 || fail "profile check found something to change (see $DIR/check.txt)"
grep -q 'a start would change nothing' "$DIR/check.txt" || fail "profile check said otherwise (see $DIR/check.txt)"
log "ok: a private skeleton, and a second start that wrote nothing ($(wc -l <"$DIR/after-2.txt") files recorded)"
