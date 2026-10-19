#!/usr/bin/env bash
# storage-migration-ci.sh — move an old single-folder profile for real, in
# CI only (docs/storage/README.md, "Moving an old profile"; ledger S14).
#
#   scripts/storage-migration-ci.sh BIN
#   RUNNER_TEMP=DIR scripts/storage-migration-ci.sh --same-device BIN
#
# It refuses to run outside GitHub Actions (GITHUB_ACTIONS=true): it moves
# whatever is at an old profile's place under the HOME it sets, and on Linux
# it writes to /dev/shm (RAM). With --same-device, the old profile is put in
# $RUNNER_TEMP instead, on the new folders' file system, as on macOS: that
# runs anywhere, to check the script itself (the cache then moves too).
#
# Linux: the old profile is on another file system than the new folders
# (/dev/shm against $RUNNER_TEMP), so every item is copied, checked and
# published, and the content cache stays behind. macOS: one file system; the
# cache moves too. (Windows has no step: its Known Folders cannot be
# redirected by the environment; the kill job covers Windows.)
#
# The old profile: the settings template of f6ee26a (which sets nothing), an
# id, a workspace with one tab and four visits, a palette, grants, a site
# store, a replay log, a wallet export, a cache shard, and a wallet key with
# its list, made by BIN itself in a profile of their own. Then:
#   1. `paths` names the old profile and only the CI's folders;
#   2. `profile check` exits 1 and changes nothing;
#   3. `wallet list` moves the profile and lists the wallet;
#   4. the id and the key moved byte for byte; settings.toml is the one a
#      new profile gets; the session has 1 tab and the history 4 visits; the
#      old folder holds only MIGRATED.txt (and, on Linux, the cache); every
#      file in the settings, data and state folders is 0600;
#   5. a second `wallet list` changes nothing but the lock files.
#
# Exit codes: 0 as described; 1 a step failed; 2 a tool is missing; 3 not in
# GitHub Actions; 64 usage.

set -euo pipefail

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { log "FAILED: $*"; exit 1; }

SAME_DEVICE=0
if [[ ${1:-} == --same-device ]]; then
    SAME_DEVICE=1
    shift
fi
(($# == 1)) || { sed -n '2,34p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 64; }
[[ ${GITHUB_ACTIONS:-} == true ]] || ((SAME_DEVICE)) ||
    { log "refusing to run outside GitHub Actions (it moves an old profile for real); --same-device runs anywhere"; exit 3; }
: "${RUNNER_TEMP:?RUNNER_TEMP is not set}"
command -v jq >/dev/null || { log "jq is missing"; exit 2; }
BIN=$(cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")
[[ -x $BIN ]] || { log "binary not found: $BIN"; exit 2; }

case $(uname -s) in
Linux) OS=linux ;;
Darwin) OS=macos ;;
*) log "Linux and macOS only"; exit 64 ;;
esac
# Where the old profile is: another file system on Linux in CI, else the
# runner's own.
FAR=0
case $OS in
linux) if ((SAME_DEVICE)); then
    CI_HOME=$RUNNER_TEMP/home
else
    FAR=1
    CI_HOME=/dev/shm/f1r3gaze-ci-home
fi
    LEGACY=$CI_HOME/.local/share/f1r3gaze
    ;;
macos)
    CI_HOME=$RUNNER_TEMP/home
    LEGACY="$CI_HOME/Library/Application Support/F1R3Gaze"
    ;;
esac

sha() {
    if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
device() {
    if [[ $OS == macos ]]; then stat -f '%d' "$1"; else stat -c '%d' "$1"; fi
}
mode_of() {
    if [[ $OS == macos ]]; then stat -f '%Lp' "$1"; else stat -c '%a' "$1"; fi
}
# Every file under $1 but the lock files: path, size, inode, time, sha256.
record() {
    local path meta
    find "$1" -type f | LC_ALL=C sort | while IFS= read -r path; do
        case $(basename -- "$path") in
        instance.lock | instance.pid) continue ;;
        esac
        if [[ $OS == macos ]]; then meta=$(stat -f '%z %i %m' "$path"); else meta=$(stat -c '%s %i %Y' "$path"); fi
        printf '%s %s %s\n' "$path" "$meta" "$(sha "$path")"
    done
}

WORK=$RUNNER_TEMP/migration
rm -rf -- "$WORK" "$CI_HOME"
mkdir -p -- "$WORK" "$CI_HOME" "$RUNNER_TEMP/tmp"

# ── The wallet key, made by BIN in a profile of its own ─────────────────
printf '%064d\n' 0 | tr 0 3 >"$WORK/key.hex"
mkdir -p -- "$WORK/keysrc/config"
printf '[shard]\nobservers = []\n' >"$WORK/keysrc/config/settings.toml"
ADDRESS=$("$BIN" --profile "$WORK/keysrc" wallet import "$WORK/key.hex" "CI wallet" | tail -n1)
[[ -n $ADDRESS ]] || fail "the key could not be imported"
KEY=$(find "$WORK/keysrc/data/wallet/keys" -name '*.key' | head -n1)
[[ -n $KEY ]] || fail "the key file was not made"

# ── The old profile ──────────────────────────────────────────────────────
mkdir -p -- "$LEGACY/keys" "$LEGACY/store" "$LEGACY/logs" "$LEGACY/exports" "$LEGACY/cache/ab"
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
    '# max_fee = 10000000' >"$LEGACY/settings.conf"
# The template of f6ee26a, byte for byte (433 bytes).
[[ $(sha "$LEGACY/settings.conf") == 8130f83a71c5428f7dc1187bc63089d61c88d9302df6bd440631fadcafc939ff ]] ||
    fail "the f6ee26a template was not written byte for byte"
printf '5f1c2d3e4b5a69788796a5b4c3d2e1f0' >"$LEGACY/user-id"
NOW=$(date +%s)
jq -n --argjson now "$NOW" '{theme: "dark", sidebar_open: false, panel: "tabs", tree_tabs: false, active: 0,
    tabs: [{url: "https://example.org/", title: "Example", parent: null}],
    visits: [range(0; 4) | {url: ("https://example.org/" + tostring), title: ("Page " + tostring), at: ($now - .)}]}' \
    >"$LEGACY/workspace.json"
printf ':root { --gaze-bg: #101010; }\n' >"$LEGACY/palette.css"
: >"$LEGACY/grants.tsv"
cp -- "$KEY" "$LEGACY/keys/"
cp -- "$WORK/keysrc/data/wallet/wallets.tsv" "$LEGACY/wallets.tsv"
if [[ -f $WORK/keysrc/data/wallet/wallet-active ]]; then
    cp -- "$WORK/keysrc/data/wallet/wallet-active" "$LEGACY/wallet-active"
fi
printf 'store records' >"$LEGACY/store/$(printf '%064d' 0 | tr 0 d).gzs"
printf 'a replay log' >"$LEGACY/logs/tab-1-2.gzlog"
printf '{"an": "export"}' >"$LEGACY/exports/$ADDRESS.json"
printf 'a cached blob' >"$LEGACY/cache/ab/$(printf '%064d' 0 | tr 0 a)"
KEY_NAME=$(basename -- "$KEY")
KEY_SHA=$(sha "$KEY")

# ── The new folders: every XDG variable in $RUNNER_TEMP ─────────────────
export HOME=$CI_HOME TMPDIR=$RUNNER_TEMP/tmp
for class in config data state cache runtime config-dirs data-dirs; do
    mkdir -p -- "$RUNNER_TEMP/xdg/$class"
done
chmod 700 "$RUNNER_TEMP/xdg/runtime"
export XDG_CONFIG_HOME=$RUNNER_TEMP/xdg/config XDG_DATA_HOME=$RUNNER_TEMP/xdg/data
export XDG_STATE_HOME=$RUNNER_TEMP/xdg/state XDG_CACHE_HOME=$RUNNER_TEMP/xdg/cache
export XDG_RUNTIME_DIR=$RUNNER_TEMP/xdg/runtime XDG_CONFIG_DIRS=$RUNNER_TEMP/xdg/config-dirs XDG_DATA_DIRS=$RUNNER_TEMP/xdg/data-dirs
unset F1R3GAZE_PROFILE DBUS_SESSION_BUS_ADDRESS

if ((FAR)); then
    [[ $(device "$LEGACY") != $(device "$RUNNER_TEMP") ]] || fail "/dev/shm and \$RUNNER_TEMP are one file system"
else
    [[ $(device "$LEGACY") == $(device "$RUNNER_TEMP") ]] || fail "the old profile is on another file system"
fi

# 1. paths.
"$BIN" paths >"$WORK/paths.txt"
grep -qxF "$(printf 'legacy\t%s' "$LEGACY")" "$WORK/paths.txt" || fail "paths does not name the old profile (see $WORK/paths.txt)"
while IFS=$'\t' read -r class path; do
    case $class in
    legacy) ;;
    # macOS reads system-wide settings and themes from /Library, which only
    # an administrator writes; on Linux they are $XDG_*_DIRS, set above.
    system-*) [[ $OS == macos || $path == "$RUNNER_TEMP"/* ]] || fail "paths names $path, outside the CI's folders" ;;
    *) [[ $path == "$RUNNER_TEMP"/* || $path == "$CI_HOME"/* ]] || fail "paths names $path, outside the CI's folders" ;;
    esac
done <"$WORK/paths.txt"
CONFIG=$(awk -F'\t' '$1 == "config" {print $2}' "$WORK/paths.txt")
DATA=$(awk -F'\t' '$1 == "data" {print $2}' "$WORK/paths.txt")
STATE=$(awk -F'\t' '$1 == "state" {print $2}' "$WORK/paths.txt")
CACHE=$(awk -F'\t' '$1 == "cache" {print $2}' "$WORK/paths.txt")

# 2. profile check changes nothing.
record "$LEGACY" >"$WORK/legacy-before.txt"
set +e
"$BIN" profile check >"$WORK/check.txt" 2>&1
status=$?
set -e
((status == 1)) || fail "profile check exited $status, not 1 (see $WORK/check.txt)"
grep -q 'would be moved' "$WORK/check.txt" || fail "profile check did not say the profile would be moved"
record "$LEGACY" | diff -u "$WORK/legacy-before.txt" - || fail "profile check changed the old profile"
for root in "$CONFIG" "$DATA" "$STATE" "$CACHE"; do
    [[ ! -e $root ]] || fail "profile check made $root"
done

# 3. The move.
"$BIN" wallet list >"$WORK/list.txt" 2>"$WORK/list.err" || fail "wallet list failed (see $WORK/list.err)"
grep -q "$ADDRESS" "$WORK/list.txt" || fail "the wallet is not listed after the move"

# 4. What was moved.
[[ $(cat "$DATA/user-id") == 5f1c2d3e4b5a69788796a5b4c3d2e1f0 ]] || fail "the user id changed"
[[ $(sha "$DATA/wallet/keys/$KEY_NAME") == "$KEY_SHA" ]] || fail "the key did not move byte for byte"
"$BIN" --profile "$WORK/fresh" wallet list >/dev/null 2>&1 || fail "a fresh profile did not start"
cmp -- "$CONFIG/settings.toml" "$WORK/fresh/config/settings.toml" || fail "settings.toml is not the template a new profile gets"
[[ $(jq '.tabs | length' "$STATE/session.json") == 1 ]] || fail "the session does not hold the 1 tab"
[[ $(jq '.visits | length' "$STATE/history.json") == 4 ]] || fail "the history does not hold the 4 visits"
[[ -f $CONFIG/themes/custom.css ]] || fail "the palette did not become themes/custom.css"
left=$(cd -- "$LEGACY" && find . -mindepth 1 -maxdepth 1 | LC_ALL=C sort | tr '\n' ' ')
if ((FAR)); then
    [[ $left == './MIGRATED.txt ./cache ' ]] || fail "the old folder still holds: $left"
    [[ -f $LEGACY/cache/ab/$(printf '%064d' 0 | tr 0 a) ]] || fail "the cache did not stay behind"
else
    [[ $left == './MIGRATED.txt ' ]] || fail "the old folder still holds: $left"
    [[ -f $CACHE/content/ab/$(printf '%064d' 0 | tr 0 a) ]] || fail "the cache did not move"
fi
while IFS= read -r file; do
    case $(basename -- "$file") in
    instance.lock) continue ;;
    esac
    [[ $(mode_of "$file") == 600 ]] || fail "$file is $(mode_of "$file"), not 600"
done < <(find "$CONFIG" "$DATA" "$STATE" -type f)

# 5. A second start changes nothing but the lock files.
record "$RUNNER_TEMP/xdg" >"$WORK/after-1.txt"
record "$CI_HOME" >>"$WORK/after-1.txt"
"$BIN" wallet list >/dev/null 2>"$WORK/list-2.err" || fail "the second wallet list failed"
record "$RUNNER_TEMP/xdg" >"$WORK/after-2.txt"
record "$CI_HOME" >>"$WORK/after-2.txt"
diff -u "$WORK/after-1.txt" "$WORK/after-2.txt" || fail "the second start changed something"
log "ok: the old profile moved ($OS, $( ((FAR)) && echo 'across file systems' || echo 'on one file system')), and a second start changed nothing"
