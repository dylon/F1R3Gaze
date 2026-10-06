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
# the build's own default), --keep-window 0|1 (sets F1R3GAZE_KEEP_WINDOW, the
# keeping of state/window.json; default: the build's own, on), --watch-saves
# (count the writes of window.json in each run, during the sweep and in all,
# into watch.tsv: scripts/lib/watch-renames.py), --label NAME
# (default: "coalesce-<value>"),
# --out DIR (default target/resize-bench/<label>), --work DIR (default
# target/resize-bench/work), --step PX (default 4), --rate HZ (default 120),
# --single-tab (open only the bench page, given on the command line, instead
# of restoring six tabs from state/session.json: builds that restore no tabs,
# such as main before the chrome overhaul, are compared this way),
# --seed-format current|legacy (default current: config/settings.toml and
# state/session.json; legacy: settings.conf and workspace.json, for builds
# from before the five-root layout).
#
# Isolation (scripts/lib/storage-isolation.bash): every XDG variable points
# into the work directory, there is no session bus, dbus-send is a fake, and
# the Vulkan driver is named directly. Before Xvfb starts, `f1r3gaze paths`
# must name only the work directory (current format); a work directory near
# a real F1R3Gaze folder is refused (exit 3).
#
# Output: frames-<run>.tsv (one row per frame), runs.tsv (one row per run),
# sweeps.tsv (each sweep's start and end, nanoseconds since the epoch) and
# summary.txt (medians over the runs). A run's row covers its sweep: the
# frames from the first one with a resize to the last one with a resize,
# among those painted from the sweep's start until a second after its end.
# `--analyze LOG` instead writes LOG.tsv and prints the summary of a log
# captured elsewhere, for example while a window edge is dragged by hand;
# such a log has no sweep times, so its row runs from its first frame with
# a resize to its last. Among the columns, resize_frames counts the frames
# that applied a resize, extra_frames those painted during the sweep without
# one (ledger L9, H7), saves the window's readings (storage ledger S13) and
# save_max_ms the longest. Exit codes: 0 ok; 1 a run failed; 2 a tool or the
# binary is missing; 3 an unsafe work directory, or the isolation self-test
# failed; 64 usage.
#
# Xvfb renders with software Vulkan on X11: the numbers measure the CPU side
# and the event pattern, not a GPU and a Wayland compositor.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=""
RUNS=5
COALESCE=""
KEEP_WINDOW=""
LABEL=""
OUT=""
WORK=""
STEP=4
RATE=120
ANALYZE=""
SINGLE_TAB=0
WATCH_SAVES=0
WATCH_PID=""
SEED_FORMAT=current

usage() {
    # Was a fixed range of lines (2-43): the header's comment lines, up to
    # the first empty line.
    # sed -n '2,43p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed -n 's/^# \{0,1\}//p'
    exit 64
}

while (($#)); do
    case $1 in
    --bin) BIN=${2:?--bin needs a path}; shift ;;
    --runs) RUNS=${2:?--runs needs a number}; shift ;;
    --coalesce) COALESCE=${2:?--coalesce needs 0 or 1}; shift ;;
    --keep-window) KEEP_WINDOW=${2:?--keep-window needs 0 or 1}; shift ;;
    --label) LABEL=${2:?--label needs a name}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --work) WORK=${2:?--work needs a directory}; shift ;;
    --step) STEP=${2:?--step needs pixels}; shift ;;
    --rate) RATE=${2:?--rate needs a frequency}; shift ;;
    --analyze) ANALYZE=${2:?--analyze needs a log}; shift ;;
    --single-tab) SINGLE_TAB=1 ;;
    --watch-saves) WATCH_SAVES=1 ;;
    --seed-format) SEED_FORMAT=${2:?--seed-format needs current or legacy}; shift ;;
    -h | --help) usage ;;
    *) printf 'unknown option: %s\n' "$1" >&2; usage ;;
    esac
    shift
done
[[ -n $BIN || -n $ANALYZE ]] || { printf -- '--bin or --analyze is required\n' >&2; exit 64; }
[[ -z $COALESCE || $COALESCE == 0 || $COALESCE == 1 ]] || { printf -- '--coalesce takes 0 or 1\n' >&2; exit 64; }
[[ -z $KEEP_WINDOW || $KEEP_WINDOW == 0 || $KEEP_WINDOW == 1 ]] || { printf -- '--keep-window takes 0 or 1\n' >&2; exit 64; }
[[ $SEED_FORMAT == current || $SEED_FORMAT == legacy ]] || { printf -- '--seed-format takes current or legacy\n' >&2; exit 64; }
# shellcheck source-path=SCRIPTDIR source=lib/storage-isolation.bash
source "$ROOT/scripts/lib/storage-isolation.bash"
# The real folders, from the real environment, before anything is overridden.
storage_real_roots
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
              "page_redraws", "chrome_redraws", "cursor_sets", "epoch", "window_saves", "window_save"
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
              f["page_redraws"], f["chrome_redraws"], f["cursor_sets"], f["epoch"],
              f["window_saves"] + 0, f["window_save"] + 0
        resolve = 0; delete vello
      }' "$1"
}

# fold_saves LOG → one row per reading of the window: its wall-clock time,
# how long it took and whether it wrote window.json (frame_stats::WindowSave;
# storage ledger S13, part 2). Logs of builds that predate the line give the
# header alone.
fold_saves() {
    gawk '
      BEGIN { OFS = "\t"; print "epoch", "took", "changed" }
      /^window_save / {
        delete f
        for (i = 2; i <= NF; i++) { split($i, kv, "="); f[kv[1]] = kv[2] }
        print f["epoch"], f["took"], f["changed"]
      }' "$1"
}

# summarise FRAMES.tsv SAVES.tsv [FROM_MS TO_MS] → one row. The sweep is the
# frames from the first one with a resize to the last one with a resize;
# given the sweep's own start and end (wall-clock milliseconds, which the
# frame line's `epoch` matches), only the frames painted from its start on,
# until a second after its end, count. Was: from the first frame with a
# resize, which is the window's first size, painted before the 1.5 s wait
# that precedes the sweep: span_ms counted that wait, and extra_frames a
# frame painted in it (storage ledger S13, part 2). `--analyze` has no sweep
# times and keeps that definition. The readings of the window counted
# (saves, save_writes, save_max_ms) are those of SAVES.tsv made from the
# first resize's arrival (its frame's epoch less its latency) to the last
# resize frame; the frame line's own window_saves would count a reading made
# in the wait on the sweep's first frame.
summarise() {
    gawk -F '\t' -v from="${3:-}" -v to="${4:-}" '
      function median(a, n,   s) { asort(a, s); return n ? (n % 2 ? s[(n + 1) / 2] : (s[n / 2] + s[n / 2 + 1]) / 2) : 0 }
      function p95(a, n,   s, k) { asort(a, s); k = int(0.95 * n + 0.999); if (k < 1) k = 1; return n ? s[k] : 0 }
      # Was: NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
      # with the frames the only file.
      FNR == 1 && FILENAME == ARGV[1] { for (i = 1; i <= NF; i++) col[$i] = i; next }
      FNR == 1 { for (i = 1; i <= NF; i++) scol[$i] = i; next }
      FILENAME != ARGV[1] { readings++; at[readings] = $(scol["epoch"]); took[readings] = $(scol["took"]); wrote[readings] = $(scol["changed"]); next }
      { row[NR] = $0 }
      from != "" && ($(col["epoch"]) < from || $(col["epoch"]) > to + 1000) { next }
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
          # Was: the counts on the frame line, of the readings since the
          # frame before, which put a reading made in the wait before the
          # sweep on the first frame of the sweep (S13, part 2); the
          # readings are now counted from SAVES.tsv below.
          # if (col["window_saves"]) { saves += v[col["window_saves"]]; if (v[col["window_save"]] > save_max) save_max = v[col["window_save"]] }
          if (n == 1) { t0 = v[col["t"]]; swept_from = v[col["epoch"]] - v[col["latency"]] }
          t1 = v[col["t"]]; swept_to = v[col["epoch"]]
        }
        # The readings made while the window was being resized.
        for (k = 1; k <= readings; k++)
          if (n && at[k] >= swept_from && at[k] <= swept_to) { saves++; save_writes += wrote[k]; if (took[k] > save_max) save_max = took[k] }
        span = t1 - t0
        printf "%d\t%.0f\t%.1f\t%d\t%d\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%d\t%d\t%d\t%d\t%.2f\n",
          n, span, (span > 0 ? n * 1000 / span : 0), resizes, reconf, (n ? reconf / n : 0), (n ? polls / n : 0),
          median(iv, n), p95(iv, n), median(rs, n), median(cm, n), median(gr, n), median(pr, n), median(wt, n),
          median(rc, n), p95(rc, n), median(lt, n), median(pl, n), resize_frames, extra_frames, saves, save_writes, save_max
      }' "$1" "$2"
}

SUMMARY_HEADER='frames\tspan_ms\tfps\tresizes\treconfigures\treconfigures_per_frame\tpolls_per_frame\tinterval_med\tinterval_p95\tresolve_med\tcmd_med\tgpu_render_med\tpresent_med\twait_med\treconfigure_med\treconfigure_p95\tlatency_med\tpoll_med\tresize_frames\textra_frames\tsaves\tsave_writes\tsave_max_ms'
if [[ -n $ANALYZE ]]; then
    fold "$ANALYZE" >"$ANALYZE.tsv"
    fold_saves "$ANALYZE" >"$ANALYZE.saves.tsv"
    printf '%b\n%s\n' "$SUMMARY_HEADER" "$(summarise "$ANALYZE.tsv" "$ANALYZE.saves.tsv")" | column -t -s $'\t'
    exit 0
fi


for tool in Xvfb xdotool xdpyinfo gawk timeout flock jq; do
    command -v "$tool" >/dev/null || die 2 "missing tool: $tool"
done
[[ -x $BIN ]] || die 2 "binary not found or not executable: $BIN"

# Before anything is written there (exit 3 near a real F1R3Gaze folder).
storage_guard "$WORK"
mkdir -p -- "$WORK" "$OUT"
exec 9>"$WORK/.lock"
flock -n 9 || die 2 "another resize-bench run holds $WORK/.lock"

unset WAYLAND_DISPLAY
export WINIT_X11_SCALE_FACTOR=1
# Was `export XDG_DATA_HOME="$WORK/xdg"` alone, until the switch-over (ledger
# S14): every variable F1R3Gaze reads points into the work directory now.
storage_isolate "$WORK"
if [[ $SEED_FORMAT == current ]]; then
    storage_paths_self_test "$BIN" "$WORK" "$WORK/profile-self-test"
fi
[[ -n $COALESCE ]] && export F1R3GAZE_COALESCE_RESIZE=$COALESCE
[[ -n $KEEP_WINDOW ]] && export F1R3GAZE_KEEP_WINDOW=$KEEP_WINDOW

XVFB_PID=""
APP_PID=""
cleanup() {
    local rc=$?
    [[ -n $APP_PID ]] && kill "$APP_PID" 2>/dev/null
    [[ -n $WATCH_PID ]] && kill "$WATCH_PID" 2>/dev/null
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

seed_profile() {
    local profile=$1
    storage_guard "$profile"
    rm -rf -- "$profile"
    if [[ $SEED_FORMAT == current ]]; then
        mkdir -p -- "$profile/config" "$profile/state"
        printf '[appearance]\ntheme = "default-dark"\n\n[shard]\nobservers = []\n' >"$profile/config/settings.toml"
        ((SINGLE_TAB)) && return 0
        six_tabs "file://$SITE" >"$profile/state/session.json"
        return 0
    fi
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
printf 'run\t%b\n' "$SUMMARY_HEADER" >"$RUNS_TSV"
{
    printf 'label\t%s\n' "$LABEL"
    printf 'date\t%s\n' "$(date -Is)"
    printf 'binary\t%s\n' "$(sha256sum "$BIN" | cut -d' ' -f1)"
    printf 'coalesce_env\t%s\n' "${COALESCE:-unset}"
    printf 'keep_window_env\t%s\n' "${KEEP_WINDOW:-unset}"
    printf 'tabs\t%s\n' "$( ((SINGLE_TAB)) && echo 'one (page on the command line)' || echo 'six (seeded session)')"
    printf 'seed_format\t%s\n' "$SEED_FORMAT"
    printf 'step_px\t%s\nrate_hz\t%s\nruns\t%s\n' "$STEP" "$RATE" "$RUNS"
} >"$OUT/run.txt"

# The page opened on the command line in --single-tab mode.
PAGE_ARGS=()
((SINGLE_TAB)) && PAGE_ARGS=("file://$SITE/page.html")

FAILED=0
for run in $(seq 1 "$RUNS"); do
    profile="$WORK/profile"
    seed_profile "$profile"
    if ((WATCH_SAVES)); then
        # Every rename into state/: an atomic write of window.json is one.
        mkdir -p -- "$profile/state"
        : >"$OUT/renames-$run.log"
        python3 "$ROOT/scripts/lib/watch-renames.py" "$profile/state" "$OUT/renames-$run.log" &
        WATCH_PID=$!
    fi
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
    ((run > 1)) || printf 'run\tstarted_ns\tended_ns\n' >"$OUT/sweeps.tsv"
    printf '%s\t%s\t%s\n' "$run" "$started" "$ended" >>"$OUT/sweeps.tsv"
    sleep 1.5
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
    if ((WATCH_SAVES)); then
        kill "$WATCH_PID" 2>/dev/null || true
        wait "$WATCH_PID" 2>/dev/null || true
        WATCH_PID=""
        ((run > 1)) || printf 'run\twindow_json_writes_in_sweep\twindow_json_writes\n' >"$OUT/watch.tsv"
        gawk -F '\t' -v run="$run" -v from="$started" -v to="$ended" '
          $2 == "window.json" { all++; if ($1 * 1e9 >= from && $1 * 1e9 <= to) sweep++ }
          END { printf "%s\t%d\t%d\n", run, sweep, all }' "$OUT/renames-$run.log" >>"$OUT/watch.tsv"
    fi
    fold "$frames_log" >"$OUT/frames-$run.tsv"
    fold_saves "$frames_log" >"$OUT/saves-$run.tsv"
    printf '%s\t%s\n' "$run" "$(summarise "$OUT/frames-$run.tsv" "$OUT/saves-$run.tsv" "$((started / 1000000))" "$((ended / 1000000))")" >>"$RUNS_TSV"
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
