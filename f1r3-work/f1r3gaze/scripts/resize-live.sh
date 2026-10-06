#!/usr/bin/env bash
# resize-live.sh — record the render loop while a window corner is dragged by
# hand (docs/ui/ledger.md, L9).
#
# Opens a `frame-times` build of f1r3gaze on the current display (the user's
# own desktop session) with a throwaway profile: six tabs, the active one a
# text-heavy page. `perf record` is attached (DWARF call graphs: an AMD Zen 3
# CPU has no LBR). Drag a corner of the window for about ten seconds, then
# close it; the script then analyses what was recorded.
#
#   scripts/resize-live.sh --bin PATH [--coalesce 0|1] [--single-tab] [--no-perf]
#                          [--label NAME] [--out DIR]
#
# --single-tab opens only the text page, given on the command line, for
# builds that restore no tabs (main before the chrome overhaul). --no-perf
# leaves perf out, for runs whose timings are compared with each other.
# --seed-format legacy seeds settings.conf and workspace.json, for builds
# from before the five-root layout (default current: config/settings.toml
# and state/session.json).
#
# Isolation (scripts/lib/storage-isolation.bash): every XDG variable points
# into DIR (a relative WAYLAND_DISPLAY is made absolute first, so the window
# still reaches the desktop), there is no session bus, dbus-send is a fake,
# and the Vulkan driver is named directly. Before the window opens, `f1r3gaze
# paths` must name only DIR (current format); a DIR near a real F1R3Gaze
# folder is refused (exit 3).
# The environment variables F1R3GAZE_COALESCE_RESIZE and
# F1R3GAZE_POLL_ON_RESIZE are passed through and recorded in run.txt.
#
# Output in DIR (default target/resize-live/<label>, the label defaulting to
# coalesce-<value>): log.txt (the frame log), log.txt.tsv (one row per frame),
# summary.txt (resize-bench.sh --analyze) and, unless --no-perf, perf.data,
# perf-report.txt (by self time), perf-children.txt (by inclusive time) and
# flamegraph.svg.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=""
COALESCE=""
OUT=""
LABEL=""
SINGLE_TAB=0
PERF=1
SEED_FORMAT=current

while (($#)); do
    case $1 in
    --bin) BIN=${2:?--bin needs a path}; shift ;;
    --coalesce) COALESCE=${2:?--coalesce needs 0 or 1}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --label) LABEL=${2:?--label needs a name}; shift ;;
    --single-tab) SINGLE_TAB=1 ;;
    --no-perf) PERF=0 ;;
    --seed-format) SEED_FORMAT=${2:?--seed-format needs current or legacy}; shift ;;
    -h | --help) sed -n '2,36p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 64 ;;
    *) printf 'unknown option: %s\n' "$1" >&2; exit 64 ;;
    esac
    shift
done
[[ -x $BIN ]] || { printf 'binary not found or not executable: %s\n' "$BIN" >&2; exit 2; }
[[ -z $COALESCE || $COALESCE == 0 || $COALESCE == 1 ]] || { printf -- '--coalesce takes 0 or 1\n' >&2; exit 64; }
[[ $SEED_FORMAT == current || $SEED_FORMAT == legacy ]] || { printf -- '--seed-format takes current or legacy\n' >&2; exit 64; }
# shellcheck source-path=SCRIPTDIR source=lib/storage-isolation.bash
source "$ROOT/scripts/lib/storage-isolation.bash"
# The real folders, from the real environment, before anything is overridden.
storage_real_roots
LABEL=${LABEL:-coalesce-${COALESCE:-default}}
OUT=${OUT:-$ROOT/target/resize-live/$LABEL}
TOOLS=(gawk jq)
((PERF)) && TOOLS+=(perf inferno-collapse-perf inferno-flamegraph)
for tool in "${TOOLS[@]}"; do
    command -v "$tool" >/dev/null || { printf 'missing tool: %s\n' "$tool" >&2; exit 2; }
done
# Before anything is written there (exit 3 near a real F1R3Gaze folder).
storage_guard "$OUT"
mkdir -p -- "$OUT"
storage_isolate "$OUT"
[[ -n $COALESCE ]] && export F1R3GAZE_COALESCE_RESIZE=$COALESCE

# The same page and tabs as resize-bench.sh.
SITE="$OUT/site"
PROFILE="$OUT/profile"
rm -rf -- "$PROFILE"
mkdir -p -- "$SITE"
{
    printf '<html><head><title>Resize bench</title><style>body{font-family:sans-serif;margin:32px;line-height:1.55;color:#222;background:#fff}</style></head><body><h1>Resize bench</h1>\n'
    for i in $(seq 1 60); do
        printf '<p>Paragraph %d: the window is being resized, so this text wraps again at every width, and the chrome fits its tab titles to the new width too.</p>\n' "$i"
    done
    printf '</body></html>\n'
} >"$SITE/page.html"
# six_tabs SITE_URL: the session of six tabs, the first active.
six_tabs() {
    jq -n --arg site "$1" '{version: 1, sidebar_open: false, panel: "tabs", tree_tabs: false, active: 0,
      tabs: [
        {url: ($site + "/page.html"), title: "Resize bench", parent: null},
        {url: "gaze://about", title: "About F1R3Gaze", parent: null},
        {url: "gaze://newtab", title: "New tab", parent: null},
        {url: ($site + "/page.html?2"), title: "A second copy of the bench page", parent: null},
        {url: ($site + "/page.html?3"), title: "A third copy, with a longer title to fit", parent: null},
        {url: ($site + "/page.html?4"), title: "Fourth", parent: null}
      ]}'
}
PAGE_ARGS=()
if ((SINGLE_TAB)); then
    PAGE_ARGS=("file://$SITE/page.html")
fi
if [[ $SEED_FORMAT == current ]]; then
    mkdir -p -- "$PROFILE/config" "$PROFILE/state"
    printf '[appearance]\ntheme = "default-dark"\n\n[shard]\nobservers = []\n' >"$PROFILE/config/settings.toml"
    ((SINGLE_TAB)) || six_tabs "file://$SITE" >"$PROFILE/state/session.json"
    storage_paths_self_test "$BIN" "$OUT" "$PROFILE"
else
    mkdir -p -- "$PROFILE"
    printf 'observers =\n' >"$PROFILE/settings.conf"
    if ((!SINGLE_TAB)); then
    cat >"$PROFILE/workspace.json" <<JSON
{"theme": "dark", "sidebar_open": false, "panel": "tabs", "tree_tabs": false, "active": 0,
 "tabs": [
  {"url": "file://$SITE/page.html", "title": "Resize bench", "parent": null},
  {"url": "gaze://about", "title": "About F1R3Gaze", "parent": null},
  {"url": "gaze://newtab", "title": "New tab", "parent": null},
  {"url": "file://$SITE/page.html?2", "title": "A second copy of the bench page", "parent": null},
  {"url": "file://$SITE/page.html?3", "title": "A third copy, with a longer title to fit", "parent": null},
  {"url": "file://$SITE/page.html?4", "title": "Fourth", "parent": null}
 ], "visits": []}
JSON
    fi
fi

{
    printf 'date\t%s\n' "$(date -Is)"
    printf 'label\t%s\n' "$LABEL"
    printf 'binary\t%s\n' "$(sha256sum "$BIN" | cut -d' ' -f1)"
    printf 'coalesce_env\t%s\n' "${F1R3GAZE_COALESCE_RESIZE:-unset}"
    printf 'poll_on_resize_env\t%s\n' "${F1R3GAZE_POLL_ON_RESIZE:-unset}"
    printf 'tabs\t%s\n' "$( ((SINGLE_TAB)) && echo 'one (page on the command line)' || echo 'six (seeded session)')"
    printf 'seed_format\t%s\n' "$SEED_FORMAT"
    printf 'perf\t%s\n' "$PERF"
    printf 'session\t%s %s\n' "${XDG_SESSION_TYPE:-?}" "${XDG_CURRENT_DESKTOP:-?}"
} >"$OUT/run.txt"

taskset -c 2-9 "$BIN" --profile "$PROFILE" "${PAGE_ARGS[@]}" >"$OUT/log.txt" 2>"$OUT/stderr.txt" &
APP=$!
PERF_PID=""
if ((PERF)); then
    perf record -F 499 -e cycles:u --call-graph dwarf,16384 -p "$APP" -o "$OUT/perf.data" \
        >"$OUT/perf-record.txt" 2>&1 &
    PERF_PID=$!
fi
printf 'window open (pid %s): drag a corner for about ten seconds, then close it\n' "$APP"
wait "$APP" || true
[[ -n $PERF_PID ]] && { wait "$PERF_PID" || true; }

# The frame log, the perf report and the flamegraph, in parallel.
"$ROOT/scripts/resize-bench.sh" --analyze "$OUT/log.txt" >"$OUT/summary.txt" &
if ((PERF)); then
    perf report -i "$OUT/perf.data" --stdio --no-children --percent-limit 0.5 >"$OUT/perf-report.txt" 2>/dev/null &
    perf report -i "$OUT/perf.data" --stdio --children --percent-limit 2 >"$OUT/perf-children.txt" 2>/dev/null &
    perf script -i "$OUT/perf.data" 2>/dev/null | inferno-collapse-perf | inferno-flamegraph >"$OUT/flamegraph.svg" &
fi
wait
cat "$OUT/summary.txt"
