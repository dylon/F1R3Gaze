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

while (($#)); do
    case $1 in
    --bin) BIN=${2:?--bin needs a path}; shift ;;
    --coalesce) COALESCE=${2:?--coalesce needs 0 or 1}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --label) LABEL=${2:?--label needs a name}; shift ;;
    --single-tab) SINGLE_TAB=1 ;;
    --no-perf) PERF=0 ;;
    -h | --help) sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 64 ;;
    *) printf 'unknown option: %s\n' "$1" >&2; exit 64 ;;
    esac
    shift
done
[[ -x $BIN ]] || { printf 'binary not found or not executable: %s\n' "$BIN" >&2; exit 2; }
[[ -z $COALESCE || $COALESCE == 0 || $COALESCE == 1 ]] || { printf -- '--coalesce takes 0 or 1\n' >&2; exit 64; }
LABEL=${LABEL:-coalesce-${COALESCE:-default}}
OUT=${OUT:-$ROOT/target/resize-live/$LABEL}
TOOLS=(gawk)
((PERF)) && TOOLS+=(perf inferno-collapse-perf inferno-flamegraph)
for tool in "${TOOLS[@]}"; do
    command -v "$tool" >/dev/null || { printf 'missing tool: %s\n' "$tool" >&2; exit 2; }
done
mkdir -p -- "$OUT"
[[ -n $COALESCE ]] && export F1R3GAZE_COALESCE_RESIZE=$COALESCE

# The same page and tabs as resize-bench.sh.
SITE="$OUT/site"
PROFILE="$OUT/profile"
rm -rf -- "$PROFILE"
mkdir -p -- "$SITE" "$PROFILE"
{
    printf '<html><head><title>Resize bench</title><style>body{font-family:sans-serif;margin:32px;line-height:1.55;color:#222;background:#fff}</style></head><body><h1>Resize bench</h1>\n'
    for i in $(seq 1 60); do
        printf '<p>Paragraph %d: the window is being resized, so this text wraps again at every width, and the chrome fits its tab titles to the new width too.</p>\n' "$i"
    done
    printf '</body></html>\n'
} >"$SITE/page.html"
printf 'observers =\n' >"$PROFILE/settings.conf"
PAGE_ARGS=()
if ((SINGLE_TAB)); then
    PAGE_ARGS=("file://$SITE/page.html")
else
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

{
    printf 'date\t%s\n' "$(date -Is)"
    printf 'label\t%s\n' "$LABEL"
    printf 'binary\t%s\n' "$(sha256sum "$BIN" | cut -d' ' -f1)"
    printf 'coalesce_env\t%s\n' "${F1R3GAZE_COALESCE_RESIZE:-unset}"
    printf 'poll_on_resize_env\t%s\n' "${F1R3GAZE_POLL_ON_RESIZE:-unset}"
    printf 'tabs\t%s\n' "$( ((SINGLE_TAB)) && echo 'one (page on the command line)' || echo 'six (workspace.json)')"
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
