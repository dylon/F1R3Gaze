#!/usr/bin/env bash
# resize-bench.sh — measure the render loop while the window is resized
# (docs/ui/ledger.md, L9).
#
# A `frame-times` build of f1r3gaze runs under Xvfb. One xdotool process
# sweeps the window from 1280x800 down to 900x600 and back in 4 px steps,
# about 120 resizes a second. The build prints, per frame, Blitz's resolve
# phases, Vello's frame phases and the window's own line (crate::renderer);
# this script folds them into one row per frame and summarises the sweep.
#
#   scripts/resize-bench.sh --bin PATH [--runs 5] [--coalesce 0|1] [--label NAME]
#   scripts/resize-bench.sh --analyze LOG   # fold and summarise a log from any session
#
# Options: --bin PATH (a build with `--features frame-times`; required),
# --runs N (default 5), --coalesce 0|1 (sets F1R3GAZE_COALESCE_RESIZE; default:
# the build's own default), --label NAME (default: "coalesce-<value>"),
# --out DIR (default target/resize-bench/<label>), --work DIR (default
# target/resize-bench/work), --step PX (default 4), --rate HZ (default 120),
# --single-tab (open only the bench page, given on the command line, instead
# of restoring six tabs from workspace.json: builds that restore no tabs,
# such as main before the chrome overhaul, are compared this way).
#
# Output: frames-<run>.tsv (one row per frame), runs.tsv (one row per run)
# and summary.txt (medians over the runs). `--analyze LOG` instead writes
# LOG.tsv and prints the summary of a log captured elsewhere, for example
# while a window edge is dragged by hand. Among the columns, resize_frames
# counts the frames that applied a resize and extra_frames those painted
# during the sweep without one (ledger L9, H7). Exit codes: 0 ok; 1 a run
# failed; 2 a tool or the binary is missing; 64 usage.
#
# Xvfb renders with software Vulkan on X11: the numbers measure the CPU side
# and the event pattern, not a GPU and a Wayland compositor.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=""
RUNS=5
COALESCE=""
LABEL=""
OUT=""
WORK=""
STEP=4
RATE=120
ANALYZE=""
SINGLE_TAB=0

usage() {
    sed -n '2,32p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 64
}

while (($#)); do
    case $1 in
    --bin) BIN=${2:?--bin needs a path}; shift ;;
    --runs) RUNS=${2:?--runs needs a number}; shift ;;
    --coalesce) COALESCE=${2:?--coalesce needs 0 or 1}; shift ;;
    --label) LABEL=${2:?--label needs a name}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --work) WORK=${2:?--work needs a directory}; shift ;;
    --step) STEP=${2:?--step needs pixels}; shift ;;
    --rate) RATE=${2:?--rate needs a frequency}; shift ;;
    --analyze) ANALYZE=${2:?--analyze needs a log}; shift ;;
    --single-tab) SINGLE_TAB=1 ;;
    -h | --help) usage ;;
    *) printf 'unknown option: %s\n' "$1" >&2; usage ;;
    esac
    shift
done
[[ -n $BIN || -n $ANALYZE ]] || { printf -- '--bin or --analyze is required\n' >&2; exit 64; }
[[ -z $COALESCE || $COALESCE == 0 || $COALESCE == 1 ]] || { printf -- '--coalesce takes 0 or 1\n' >&2; exit 64; }
LABEL=${LABEL:-coalesce-${COALESCE:-default}}
OUT=${OUT:-$ROOT/target/resize-bench/$LABEL}
WORK=${WORK:-$ROOT/target/resize-bench/work}

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() {
    local code=$1
    shift
    log "error: $*"
    exit "$code"
}

# fold LOG → one tab-separated row per frame. Units are normalised to ms.
fold() {
    gawk '
      function ms(v,   n, u) {
        n = v + 0; u = v; sub(/^[0-9.]+/, "", u)
        if (u == "ns") return n / 1e6; if (u == "us") return n / 1e3
        if (u == "s") return n * 1e3; return n
      }
      BEGIN {
        OFS = "\t"
        print "frame", "t", "interval", "width", "height", "resizes", "reconfigures", "reconfigure",
              "render", "latency", "polls", "poll", "chrome_render", "events", "event",
              "resolve", "cmd", "gpu_render", "present", "wait",
              "page_redraws", "chrome_redraws", "cursor_sets", "epoch"
      }
      /^Resolve\([0-9]+\): / {
        total = $2; sub(/\($/, "", total)
        t = ms(total); if (t > resolve) resolve = t
        next
      }
      /^vello: / {
        line = $0; sub(/^[^(]*\(/, "", line); sub(/\)$/, "", line)
        n = split(line, parts, /, /)
        for (i = 1; i <= n; i++) { split(parts[i], kv, /: /); vello[kv[1]] = ms(kv[2]) }
        next
      }
      /^frame / {
        delete f
        for (i = 2; i <= NF; i++) { split($i, kv, "="); f[kv[1]] = kv[2] }
        split(f["size"], wh, "x")
        # The counts and the wall clock are empty in logs of builds that
        # predate them.
        print f["n"], f["t"], f["interval"], wh[1], wh[2], f["resizes"], f["reconfigures"], f["reconfigure"],
              f["render"], f["latency"], f["polls"], f["poll"], f["chrome_render"], f["events"], f["event"],
              resolve + 0, vello["cmd"] + 0, vello["render"] + 0, vello["present"] + 0, vello["wait"] + 0,
              f["page_redraws"], f["chrome_redraws"], f["cursor_sets"], f["epoch"]
        resolve = 0; delete vello
      }' "$1"
}

# summarise FRAMES.tsv → one row: the sweep is the frames from the first one
# with a resize to the last one with a resize.
summarise() {
    gawk -F '\t' '
      function median(a, n,   s) { asort(a, s); return n ? (n % 2 ? s[(n + 1) / 2] : (s[n / 2] + s[n / 2 + 1]) / 2) : 0 }
      function p95(a, n,   s, k) { asort(a, s); k = int(0.95 * n + 0.999); if (k < 1) k = 1; return n ? s[k] : 0 }
      NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
      { row[NR] = $0 }
      $(col["resizes"]) > 0 { if (!first) first = NR; last = NR }
      END {
        n = 0
        for (r = first; r <= last; r++) {
          split(row[r], v, "\t"); n++
          iv[n] = v[col["interval"]]; rs[n] = v[col["resolve"]]; cm[n] = v[col["cmd"]]
          gr[n] = v[col["gpu_render"]]; pr[n] = v[col["present"]]; wt[n] = v[col["wait"]]
          rc[n] = v[col["reconfigure"]]; lt[n] = v[col["latency"]]; pl[n] = v[col["poll"]]
          resizes += v[col["resizes"]]; reconf += v[col["reconfigures"]]; polls += v[col["polls"]]
          # A frame that applied no resize was painted for something else:
          # during a sweep, the chrome catching up with the new width (H7).
          if (v[col["resizes"]] > 0) resize_frames++; else extra_frames++
          if (n == 1) t0 = v[col["t"]]; t1 = v[col["t"]]
        }
        span = t1 - t0
        printf "%d\t%.0f\t%.1f\t%d\t%d\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%d\t%d\n",
          n, span, (span > 0 ? n * 1000 / span : 0), resizes, reconf, (n ? reconf / n : 0), (n ? polls / n : 0),
          median(iv, n), p95(iv, n), median(rs, n), median(cm, n), median(gr, n), median(pr, n), median(wt, n),
          median(rc, n), p95(rc, n), median(lt, n), median(pl, n), resize_frames, extra_frames
      }' "$1"
}

SUMMARY_HEADER='frames\tspan_ms\tfps\tresizes\treconfigures\treconfigures_per_frame\tpolls_per_frame\tinterval_med\tinterval_p95\tresolve_med\tcmd_med\tgpu_render_med\tpresent_med\twait_med\treconfigure_med\treconfigure_p95\tlatency_med\tpoll_med\tresize_frames\textra_frames'
if [[ -n $ANALYZE ]]; then
    fold "$ANALYZE" >"$ANALYZE.tsv"
    printf "$SUMMARY_HEADER\n%s\n" "$(summarise "$ANALYZE.tsv")" | column -t -s $'\t'
    exit 0
fi


for tool in Xvfb xdotool xdpyinfo gawk timeout flock; do
    command -v "$tool" >/dev/null || die 2 "missing tool: $tool"
done
[[ -x $BIN ]] || die 2 "binary not found or not executable: $BIN"

mkdir -p -- "$WORK" "$OUT"
exec 9>"$WORK/.lock"
flock -n 9 || die 2 "another resize-bench run holds $WORK/.lock"

unset WAYLAND_DISPLAY
export WINIT_X11_SCALE_FACTOR=1
export XDG_DATA_HOME="$WORK/xdg"
[[ -n $COALESCE ]] && export F1R3GAZE_COALESCE_RESIZE=$COALESCE

XVFB_PID=""
APP_PID=""
cleanup() {
    local rc=$?
    [[ -n $APP_PID ]] && kill "$APP_PID" 2>/dev/null
    [[ -n $XVFB_PID ]] && kill "$XVFB_PID" 2>/dev/null
    wait 2>/dev/null || true
    exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

rm -f -- "$WORK/display"
Xvfb -displayfd 3 -screen 0 1600x1000x24 -nolisten tcp -noreset 3>"$WORK/display" >"$WORK/xvfb.log" 2>&1 &
XVFB_PID=$!
deadline=$((SECONDS + 10))
until [[ -s $WORK/display ]]; do
    ((SECONDS < deadline)) || die 2 "Xvfb did not start (see $WORK/xvfb.log)"
    sleep 0.1
done
DISPLAY=":$(head -n1 "$WORK/display")"
export DISPLAY
xdpyinfo >/dev/null 2>&1 || die 2 "no X server on $DISPLAY"

# A text-heavy page, so the page's layout is part of every frame.
SITE="$WORK/site"
mkdir -p -- "$SITE"
{
    printf '<html><head><title>Resize bench</title><style>body{font-family:sans-serif;margin:32px;line-height:1.55;color:#222;background:#fff}</style></head><body><h1>Resize bench</h1>\n'
    for i in $(seq 1 60); do
        printf '<p>Paragraph %d: the window is being resized, so this text wraps again at every width, and the chrome fits its tab titles to the new width too.</p>\n' "$i"
    done
    printf '</body></html>\n'
} >"$SITE/page.html"

seed_profile() {
    local profile=$1
    rm -rf -- "$profile"
    mkdir -p -- "$profile"
    printf 'observers =\n' >"$profile/settings.conf"
    ((SINGLE_TAB)) && return 0
    local site="file://$SITE"
    cat >"$profile/workspace.json" <<JSON
{"theme": "dark", "sidebar_open": false, "panel": "tabs", "tree_tabs": false, "active": 0,
 "tabs": [
  {"url": "$site/page.html", "title": "Resize bench", "parent": null},
  {"url": "gaze://about", "title": "About F1R3Gaze", "parent": null},
  {"url": "gaze://newtab", "title": "New tab", "parent": null},
  {"url": "$site/page.html?2", "title": "A second copy of the bench page", "parent": null},
  {"url": "$site/page.html?3", "title": "A third copy, with a longer title to fit", "parent": null},
  {"url": "$site/page.html?4", "title": "Fourth", "parent": null}
 ], "visits": []}
JSON
}

# The sweep: one xdotool process, so the steps are evenly spaced.
sweep_args() {
    local wid=$1 pause w h
    pause=$(gawk -v r="$RATE" 'BEGIN { printf "%.4f", 1 / r }')
    for ((w = 1280; w >= 900; w -= STEP)); do
        h=$((600 + (w - 900) * 200 / 380))
        printf 'windowsize %s %s %s sleep %s ' "$wid" "$w" "$h" "$pause"
    done
    for ((w = 900; w <= 1280; w += STEP)); do
        h=$((600 + (w - 900) * 200 / 380))
        printf 'windowsize %s %s %s sleep %s ' "$wid" "$w" "$h" "$pause"
    done
}

RUNS_TSV="$OUT/runs.tsv"
printf "run\t$SUMMARY_HEADER\n" >"$RUNS_TSV"
{
    printf 'label\t%s\n' "$LABEL"
    printf 'date\t%s\n' "$(date -Is)"
    printf 'binary\t%s\n' "$(sha256sum "$BIN" | cut -d' ' -f1)"
    printf 'coalesce_env\t%s\n' "${COALESCE:-unset}"
    printf 'tabs\t%s\n' "$( ((SINGLE_TAB)) && echo 'one (page on the command line)' || echo 'six (workspace.json)')"
    printf 'step_px\t%s\nrate_hz\t%s\nruns\t%s\n' "$STEP" "$RATE" "$RUNS"
} >"$OUT/run.txt"

# The page opened on the command line in --single-tab mode.
PAGE_ARGS=()
((SINGLE_TAB)) && PAGE_ARGS=("file://$SITE/page.html")

FAILED=0
for run in $(seq 1 "$RUNS"); do
    profile="$WORK/profile"
    seed_profile "$profile"
    frames_log="$OUT/log-$run.txt"
    taskset -c 2-9 "$BIN" --profile "$profile" "${PAGE_ARGS[@]}" >"$frames_log" 2>"$OUT/stderr-$run.txt" &
    APP_PID=$!
    WID=$(timeout 30 xdotool search --sync --onlyvisible --pid "$APP_PID" 2>/dev/null | head -n1) || WID=""
    if [[ -z $WID ]]; then
        log "run $run: no window appeared"
        FAILED=1
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
        APP_PID=""
        continue
    fi
    xdotool windowmove "$WID" 0 0
    xdotool windowsize --sync "$WID" 1280 800
    deadline=$((SECONDS + 20))
    until [[ $(xdotool getwindowname "$WID" 2>/dev/null || true) == *"Resize bench"* ]]; do
        ((SECONDS < deadline)) || { log "run $run: the page never loaded"; FAILED=1; break; }
        sleep 0.1
    done
    sleep 1.5
    started=$(date +%s%N)
    # shellcheck disable=SC2046 # the sweep is a word list by design
    xdotool $(sweep_args "$WID")
    ended=$(date +%s%N)
    sleep 1.5
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
    fold "$frames_log" >"$OUT/frames-$run.tsv"
    printf '%s\t%s\n' "$run" "$(summarise "$OUT/frames-$run.tsv")" >>"$RUNS_TSV"
    log "run $run: sweep $(((ended - started) / 1000000)) ms; $(tail -n1 "$RUNS_TSV" | cut -f2-8)"
done

# Medians over the runs, column by column.
gawk -F '\t' '
  function median(a, n,   s) { asort(a, s); return n % 2 ? s[(n + 1) / 2] : (s[n / 2] + s[n / 2 + 1]) / 2 }
  NR == 1 { for (i = 2; i <= NF; i++) name[i] = $i; cols = NF; next }
  { runs++; for (i = 2; i <= cols; i++) v[i, runs] = $i }
  END {
    printf "median of %d runs\n", runs
    for (i = 2; i <= cols; i++) { delete a; for (r = 1; r <= runs; r++) a[r] = v[i, r]; printf "%-24s %10.2f\n", name[i], median(a, runs) }
  }' "$RUNS_TSV" | tee "$OUT/summary.txt"
((FAILED == 0)) || exit 1
