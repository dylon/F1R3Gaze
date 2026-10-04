#!/usr/bin/env bash
# ui-snapshots.sh — capture every F1R3Gaze chrome dialog headlessly.
#
# The real `f1r3gaze` binary runs under Xvfb (Blitz + Vello), one scene at a
# time, each with a freshly seeded throw-away profile. Snapshots therefore
# show exactly what Blitz renders. Headless Chromium would draw placeholders,
# tooltips and ellipses that Blitz does not, and would hide real defects.
#
#   scripts/ui-snapshots.sh --before            # capture docs/screenshots/ui/before/
#   scripts/ui-snapshots.sh --after             # capture docs/screenshots/ui/after/ and enforce checks
#   scripts/ui-snapshots.sh --after --only 'find'   # a subset (regular expression on scene names)
#   scripts/ui-snapshots.sh --list              # list the scenes
#   scripts/ui-snapshots.sh --compare           # side-by-side before|after sheets (work directory)
#   scripts/ui-snapshots.sh --after --calibrate # crosshairs on the coordinate table
#
# Options: --bin PATH (default target/release/f1r3gaze), --out DIR,
# --work DIR (default target/ui-snapshots), --size WxH (default 1280x800),
# --timeout SECS per scene (default 25), --keep.
#
# Exit codes: 0 ok; 1 a scene or an enforced check failed; 2 a tool, the
# binary, or the display is missing; 3 an unsafe profile path was refused;
# 64 usage error.
#
# Safety: every profile lives under the work directory (held with flock),
# F1R3GAZE_PROFILE and XDG_DATA_HOME point inside it, and the user's real
# profile is refused. The work directory is on disk, not /tmp, which is often
# tmpfs (RAM). Wallets are imported from fixed test keys that are never funded.
#
# Design and the scene catalogue: docs/ui/README.md ("Snapshot harness").

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
MODE=""
BIN="$ROOT/target/release/f1r3gaze"
OUT=""
WORK=""
ONLY=""
SIZE="1280x800"
SCENE_TIMEOUT=25
KEEP=0
LIST=0
CALIBRATE=0
COMPARE=0

usage() {
    sed -n '2,28p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 64
}

while (($#)); do
    case $1 in
    --before) MODE=before ;;
    --after) MODE=after ;;
    --bin) BIN=${2:?--bin needs a path}; shift ;;
    --out) OUT=${2:?--out needs a directory}; shift ;;
    --work) WORK=${2:?--work needs a directory}; shift ;;
    --only) ONLY=${2:?--only needs a regular expression}; shift ;;
    --size) SIZE=${2:?--size needs WxH}; shift ;;
    --timeout) SCENE_TIMEOUT=${2:?--timeout needs seconds}; shift ;;
    --keep) KEEP=1 ;;
    --list) LIST=1 ;;
    --calibrate) CALIBRATE=1 ;;
    --compare) COMPARE=1 ;;
    -h | --help) usage ;;
    *) printf 'unknown option: %s\n' "$1" >&2; usage ;;
    esac
    shift
done

[[ $SIZE =~ ^([0-9]+)x([0-9]+)$ ]] || { printf 'bad --size %s\n' "$SIZE" >&2; exit 64; }
W=${BASH_REMATCH[1]}
H=${BASH_REMATCH[2]}

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() {
    local code=$1
    shift
    log "error: $*"
    exit "$code"
}

# ── Scene catalogue ──────────────────────────────────────────────────────
# Each name maps to a function scene_<name with - replaced by _>. A scene whose
# interaction cannot exist in one mode lists that restriction in ONLY_IN. None
# does at present: scene 60 reaches its state differently in each mode.
SCENES=(
    01-tabs-dark 02-tabs-light 03-history-dark 04-history-light
    05-sites-dark 06-sites-light 07-grants-dark 08-grants-light
    09-wallet-empty-dark 10-wallet-empty-light 11-wallet-one-dark 12-wallet-one-light
    13-console-dark 14-console-light 15-appearance-dark 16-appearance-light
    17-appearance-bad-palette 18-appearance-low-contrast 19-collapsed-dark 20-collapsed-light
    21-tab-menu 22-tab-menu-after-action 23-tabs-search 24-tabs-tree 25-many-tabs 26-history-search
    27-find-open 28-find-matches 29-find-next 30-find-cleared 31-find-tab-switch 32-find-scroll
    33-url-empty 34-url-suggestions 35-url-suggestion-down 36-url-escape 37-url-blur 38-url-switch-to-tab
    39-prompt-net 40-prompt-shard 41-error-page 42-newtab
    43-nav-disabled 44-nav-enabled 45-rail-hover 46-row-hover 47-flash-shown 48-flash-expired
    49-search-reset 50-wallet-review 51-history-clear
    52-theme-appearance-to-light 53-theme-appearance-fresh-light
    54-theme-appearance-to-dark 55-theme-appearance-fresh-dark
    56-theme-prompt-to-light 57-theme-prompt-fresh-light
    58-theme-newtab-to-light 59-theme-newtab-fresh-light
    60-theme-wallet-to-light
    61-cursor-back 62-cursor-rail 63-cursor-tab-row 64-cursor-link
    65-cursor-text 66-cursor-hover-style 67-cursor-resting 68-cursor-css-none
)
# Scene 60 used to be before-only (it needed the removed palette button); it now
# reaches the same state through Appearance in after mode. The cursor scenes
# (L8) came after the before capture and use only the after coordinates.
declare -A ONLY_IN=(
    [61-cursor-back]=after [62-cursor-rail]=after [63-cursor-tab-row]=after
    [64-cursor-link]=after [65-cursor-text]=after [66-cursor-hover-style]=after
    [67-cursor-resting]=after [68-cursor-css-none]=after
)

if ((LIST)); then
    printf '%s\n' "${SCENES[@]}"
    exit 0
fi

# ── Coordinate table ─────────────────────────────────────────────────────
# Clicks are a last resort; keyboard shortcuts and seeded state drive most
# scenes. Every position below is stable by construction (fixed-width rail,
# fixed header heights, first tab at the left edge) for a 1280x800 window,
# and --calibrate draws them on a reference capture for review.
declare -A AT
coordinates_before() {
    AT[TAB1]="48 21"            # first tab title (right-click opens the tab menu)
    AT[RAIL_TABS]="20 109"      # rail icons, top-down, 39 px pitch
    AT[RAIL_HISTORY]="20 148"
    AT[RAIL_SITES]="20 187"
    AT[RAIL_GRANTS]="20 226"
    AT[RAIL_WALLET]="20 265"
    AT[RAIL_CONSOLE]="20 304"
    AT[RAIL_APPEARANCE]="20 343"
    AT[PAGE]="900 600"          # inside the page area, away from overlays
    AT[NEUTRAL]="640 792"       # status bar: hovering it changes nothing
    AT[SIDE_SEARCH]="156 148"   # the search field under the panel header
    AT[FIRST_ROW]="180 197"     # first row of the Tabs panel (the active tab)
    AT[SECOND_ROW]="180 273"    # second row: an inactive tab, for hover
    AT[MENU_DUPLICATE]="88 241" # tab menu items (first tab's menu)
    AT[MENU_COPY]="154 241"
    AT[HISTORY_CLEAR]="297 148"
    AT[APPEARANCE_DARK]="76 231"
    AT[APPEARANCE_LIGHT]="118 231"
    AT[APPEARANCE_DARK_PROMPTED]="76 284"   # same, with the two-line prompt bar shown (+53 px)
    AT[APPEARANCE_LIGHT_PROMPTED]="118 284"
    AT[PALETTE]="1250 61"       # theme cycle button (exists only before)
    AT[WALLET_TO]="108 749"     # the Send row, bottom-anchored under one wallet
    AT[WALLET_AMOUNT]="173 749"
    AT[WALLET_REVIEW]="289 749"
    # Label-legibility crops (WxH+X+Y, inside the button, borders excluded).
    AT[CROP_PROMPT_ALLOW]="44x16+1218+104"
    AT[CROP_LAMP]="52x22+491+341"
    AT[CROP_WALLET_EXPORT]="44x14+162+668"
    # Regions for reflow and effect checks.
    AT[CROP_RAIL]="42x560+0+190"
    AT[CROP_PAGE_LEFT]="640x620+42+130"
    AT[CROP_STATUS]="1280x22+0+778"
    AT[CROP_STRIP]="1280x37+0+0"
    AT[CROP_SIDEBAR]="300x688+42+86"
    # Edge bands for the hairline check (after only: these elements are new).
    AT[CROP_NOTICE_TOP]=""
    AT[CROP_ROW_TOP]=""
    AT[CROP_RAIL_ON_TOP]=""
    AT[LINE_ROW_BAR]=""
    AT[LINE_RAIL_BAR]=""
    AT[LINE_TAB_BAR]=""
    AT[LINE_NOTICE_BAR]=""
    AT[LINE_PROMPT_BAR]=""
    # The cursor scenes run in after mode only.
    AT[BACK]=""
    AT[RELOAD]=""
    AT[URL_FIELD]=""
    AT[RAIL_MID]=""
    AT[PAGE_PLAIN]=""
    AT[PAGE_PLAIN_OPEN]=""
    AT[PAGE_GO]=""
    AT[PAGE_TEXT]=""
    AT[PAGE_NOCURSOR]=""
    AT[CROP_GO]=""
}
coordinates_after() {
    AT[TAB1]="60 20"
    AT[RAIL_TABS]="20 106"      # rail icons, top-down, 36-37 px pitch
    AT[RAIL_HISTORY]="20 143"
    AT[RAIL_SITES]="20 179"
    AT[RAIL_GRANTS]="20 215"
    AT[RAIL_WALLET]="20 251"
    AT[RAIL_CONSOLE]="20 287"
    AT[RAIL_APPEARANCE]="20 753" # pinned to the rail's bottom
    AT[PAGE]="900 600"
    AT[NEUTRAL]="640 792"
    AT[SIDE_SEARCH]="110 142"
    AT[FIRST_ROW]="170 190"
    AT[SECOND_ROW]="170 234"
    AT[MENU_DUPLICATE]="140 233" # inline menu under the first row
    AT[MENU_COPY]="152 262"
    AT[HISTORY_CLEAR]="296 142"
    AT[APPEARANCE_DARK]="103 172"
    AT[APPEARANCE_LIGHT]="195 172"
    AT[APPEARANCE_DARK_PROMPTED]="103 216"   # the prompt bar adds 44 px
    AT[APPEARANCE_LIGHT_PROMPTED]="195 216"
    AT[PALETTE]=""              # the theme cycle button was removed
    AT[WALLET_TO]="190 332"     # labeled send form under one wallet's card
    AT[WALLET_AMOUNT]="120 392"
    AT[WALLET_REVIEW]="259 431"
    AT[CROP_PROMPT_ALLOW]="36x12+1226+100"
    AT[CROP_LAMP]="52x22+481+339"
    AT[CROP_WALLET_EXPORT]=""
    AT[CROP_RAIL]="42x560+0+190"
    AT[CROP_PAGE_LEFT]="640x620+42+130"
    AT[CROP_STATUS]="1280x22+0+778"
    AT[CROP_STRIP]="1280x37+0+0"
    AT[CROP_SIDEBAR]="300x688+42+86"
    # 5 px bands across an edge: first row outside the element, last inside.
    AT[CROP_NOTICE_TOP]="220x5+80+478"    # the palette notice, Appearance (light)
    AT[CROP_ROW_TOP]="150x5+90+168"       # the active row, Tabs (light)
    AT[CROP_RAIL_ON_TOP]="16x5+14+88"     # the active rail button
    # One-pixel lines across the bars, clear of the accent-coloured icons.
    AT[LINE_ROW_BAR]="10x1+44+190"        # active row, Tabs (light)
    AT[LINE_RAIL_BAR]="10x1+0+107"        # active rail button
    AT[LINE_TAB_BAR]="1x10+100+0"         # active tab's top bar
    AT[LINE_NOTICE_BAR]="12x1+56+505"     # palette notice, Appearance (light)
    AT[LINE_PROMPT_BAR]="10x1+0+105"      # prompt bar (light)
    # Cursor scenes (L8). Toolbar buttons and the address field sit on y=60.
    AT[BACK]="55 60"
    AT[RELOAD]="119 60"
    AT[URL_FIELD]="700 60"
    AT[RAIL_MID]="20 520"                 # the rail's empty middle: the arrow
    # site/cursor.html's boxes (320x80 at page x=200), with the page area at
    # (42,84), or at x=342 when the sidebar is open.
    AT[PAGE_PLAIN]="402 184"
    AT[PAGE_PLAIN_OPEN]="702 184"
    AT[PAGE_GO]="402 304"
    AT[PAGE_TEXT]="270 424"               # over the first word of the text
    AT[PAGE_NOCURSOR]="402 544"
    AT[CROP_GO]="300x60+252+274"          # inside the link's box
}

# ── Setup ────────────────────────────────────────────────────────────────
if ((COMPARE)); then
    MODE=${MODE:-after}
fi
[[ -n $MODE ]] || { printf 'one of --before or --after is required\n' >&2; exit 64; }
OUT=${OUT:-$ROOT/docs/screenshots/ui/$MODE}
"coordinates_$MODE"

# A fixed directory, so the demo site's file:// URLs (which the captures show)
# are the same in every run. It is under target/ on disk: /tmp is often tmpfs,
# and profiles, logs and reference captures would then occupy RAM.
# BASE_TMP=${TMPDIR:-/tmp}
# WORK="$BASE_TMP/f1r3gaze-ui-snapshots"
# Disabled 2026-10-04: the default put the work directory on tmpfs.
WORK=${WORK:-$ROOT/target/ui-snapshots}
mkdir -p -- "$WORK"
exec 9>"$WORK/.lock"
flock -n 9 || die 2 "another ui-snapshots run holds $WORK/.lock"

compare_sheets() {
    local before="$ROOT/docs/screenshots/ui/before" after="$ROOT/docs/screenshots/ui/after"
    local dest="$WORK/compare" name made=0
    [[ -d $before && -d $after ]] || die 2 "need both $before and $after"
    command -v montage >/dev/null || die 2 "montage (ImageMagick) is required"
    rm -rf -- "$dest"
    mkdir -p -- "$dest"
    for name in "${SCENES[@]}"; do
        [[ -f $before/$name.png && -f $after/$name.png ]] || continue
        montage -label 'before' "$before/$name.png" -label 'after' "$after/$name.png" \
            -tile 2x1 -geometry 640x400+6+6 -title "$name" "$dest/$name.png"
        made=$((made + 1))
    done
    ((made)) || die 1 "no scene exists in both before/ and after/"
    montage "$dest"/[0-9][0-9]-*.png -tile 2x -geometry 1290x430+4+4 "$dest/index.png"
    log "wrote $made comparisons and $dest/index.png"
}
if ((COMPARE)); then
    compare_sheets
    exit 0
fi

for tool in Xvfb xdotool import magick montage jq gawk xdpyinfo python3 timeout sha256sum flock; do
    command -v "$tool" >/dev/null || die 2 "missing tool: $tool"
done
[[ -x $BIN ]] || die 2 "binary not found or not executable: $BIN (cargo build --release -p gaze-shell)"
# The cursor scenes read the X cursor through XFixes (python-xlib).
if [[ $MODE == after ]]; then
    for name in "${SCENES[@]}"; do
        [[ $name == *-cursor-* && (-z $ONLY || $name =~ $ONLY) ]] || continue
        python3 -c 'import Xlib.ext.xfixes' 2>/dev/null ||
            die 2 "missing tool: python-xlib (the cursor scenes read the X cursor with scripts/x-cursor.py)"
        break
    done
fi

# Refuse anything outside the work directory, and the user's own profile
# (computed from the real environment, before it is overridden below).
USER_PROFILE=${F1R3GAZE_PROFILE:-${XDG_DATA_HOME:-$HOME/.local/share}/f1r3gaze}
guard_profile() {
    local p=$1
    case $p in
    "$WORK"/profiles/*) ;;
    *) die 3 "refusing profile outside the work directory: $p" ;;
    esac
    [[ $p != "$USER_PROFILE" ]] || die 3 "refusing the user's profile: $p"
}

export XDG_DATA_HOME="$WORK/xdg"
export F1R3GAZE_PROFILE="$WORK/profiles/_default"
export WINIT_X11_SCALE_FACTOR=1
unset WAYLAND_DISPLAY
mkdir -p -- "$XDG_DATA_HOME" "$WORK/profiles" "$WORK/logs" "$WORK/refs" "$OUT"
SITE="$WORK/site"
SITE_URL="file://$SITE"

XVFB_PID=""
APP_PID=""
WD_PID=""
EMBERS_PID=""
cleanup() {
    local rc=$?
    stop_app
    [[ -n $EMBERS_PID ]] && kill "$EMBERS_PID" 2>/dev/null
    [[ -n $XVFB_PID ]] && kill "$XVFB_PID" 2>/dev/null
    wait 2>/dev/null || true
    if ((!KEEP)); then
        rm -rf -- "$WORK/profiles" "$WORK/xdg"
    fi
    exit "$rc"
}
stop_app() {
    if [[ -n $APP_PID ]]; then
        kill -TERM "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
        APP_PID=""
    fi
    if [[ -n $WD_PID ]]; then
        pkill -TERM -P "$WD_PID" 2>/dev/null || true
        kill "$WD_PID" 2>/dev/null || true
        wait "$WD_PID" 2>/dev/null || true
        WD_PID=""
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_xvfb() {
    rm -f -- "$WORK/display"
    Xvfb -displayfd 3 -screen 0 "${W}x${H}x24" -nolisten tcp -noreset 3>"$WORK/display" >"$WORK/xvfb.log" 2>&1 &
    XVFB_PID=$!
    local deadline=$((SECONDS + 10))
    until [[ -s $WORK/display ]]; do
        ((SECONDS < deadline)) || die 2 "Xvfb did not start (see $WORK/xvfb.log)"
        sleep 0.1
    done
    export DISPLAY=":$(head -n1 "$WORK/display")"
    xdpyinfo >/dev/null 2>&1 || die 2 "no X server on $DISPLAY"
    log "Xvfb on $DISPLAY (${W}x${H})"
}

# A minimal Embers stand-in: balances only (GET /api/wallets/<addr>/state).
start_embers() {
    rm -f -- "$WORK/embers.port"
    python3 - "$WORK/embers.port" >"$WORK/logs/embers.log" 2>&1 <<'PY' &
import http.server, json, sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/api/wallets/") and self.path.endswith("/state"):
            body = json.dumps({"balance": "1250000", "transfers": []}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.send_header("content-length", "0")
            self.end_headers()
    def log_message(self, *args):
        pass
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(sys.argv[1], "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
    EMBERS_PID=$!
    local deadline=$((SECONDS + 10))
    until [[ -s $WORK/embers.port ]]; do
        ((SECONDS < deadline)) || die 2 "mock Embers did not start (see $WORK/logs/embers.log)"
        sleep 0.1
    done
    EMBERS_URL="http://127.0.0.1:$(cat "$WORK/embers.port")"
}

make_site() {
    rm -rf -- "$SITE"
    mkdir -p -- "$SITE"
    cat >"$SITE/notes.html" <<'HTML'
<html><head><title>Field notes on gaze and capability</title>
<style>body{font-family:sans-serif;margin:32px auto;max-width:720px;padding:0 24px;line-height:1.55;color:#222;background:#fff}</style></head><body>
<h1>Field notes on gaze and capability</h1>
<p>Alpha: a page holds capabilities, not ambient authority. The gaze of the browser rests on what the page was granted, and nothing else.</p>
<p>Beta: every deploy is signed by the active wallet. A page can cost phlo, and the consent prompt quotes it, with the paying wallet and its balance.</p>
<p>Gamma: search the rendered text of this page for the word alpha, or for gaze, and watch the highlights cycle through each match.</p>
<p>Delta: a capability store keeps per-origin data with a quota; Site Data shows its usage and lets you clear it.</p>
<script type="application/f1r3lang" imports="doc log store">
new s in {
  log!("info", "notes page started") |
  log!("warn", "an example warning from the page") |
  store!("put", "visits", 1, *s) |
  for (_ <- s) { log!("info", "store written") }
}
</script>
</body></html>
HTML
    cat >"$SITE/prompt.html" <<'HTML'
<html><head><title>Cross-site fetch</title></head><body><h1>Cross-site fetch</h1><p>This page asks to fetch from another site.</p>
<script type="application/f1r3lang" imports="doc log net">new r in { net!("fetch", {"url": "https://example.org/data.json"}, *r) | for (_ <- r) { log!("info", "fetch answered") } }</script></body></html>
HTML
    cat >"$SITE/shard.html" <<'HTML'
<html><head><title>Shard reader</title></head><body><h1>Shard reader</h1><p>This page asks to read from the shard before it runs.</p>
<script type="application/f1r3lang" imports="shard">new r in { shard!("read", "rho:id:demo", *r) | for (_ <- r) { Nil } }</script></body></html>
HTML
    cat >"$SITE/find.html" <<'HTML'
<html><head><title>Second page</title><style>body{font-family:sans-serif;margin:32px;line-height:1.55;color:#222;background:#fff}</style></head><body>
<h1>Second page</h1><p>beta alpha beta: one more match on another tab.</p></body></html>
HTML
    {
        printf '<html><head><title>Long page</title><style>body{font-family:sans-serif;margin:32px;line-height:1.55;color:#222;background:#fff}</style></head><body><h1>Long page</h1>\n'
        local i
        for i in $(seq 1 160); do
            if ((i % 20 == 0)); then
                printf '<p>Paragraph %d carries the marker word for find.</p>\n' "$i"
            else
                printf '<p>Paragraph %d is filler text that makes the page scroll.</p>\n' "$i"
            fi
        done
        printf '</body></html>\n'
    } >"$SITE/long.html"
    cat >"$SITE/plain.html" <<'HTML'
<html><head><title>Plain page</title><style>body{font-family:sans-serif;margin:32px;color:#222;background:#fff}</style></head><body><h1>Plain page</h1><p>A static document.</p></body></html>
HTML
    # The cursor scenes' pages (L8). The second puts its link where the first
    # has its plain block, for a page that loads under a resting pointer.
    cursor_page_html "Cursor page" 60 180 >"$SITE/cursor.html"
    cursor_page_html "Cursor next" 180 60 >"$SITE/cursor-next.html"
}

# cursor_page_html TITLE PLAIN_TOP LINK_TOP: large boxes at fixed places, so
# the coordinate table's points hit them with room to spare.
cursor_page_html() {
    cat <<HTML
<html><head><title>$1</title><style>
body{margin:0;font:24px sans-serif;color:#222;background:#fff}
.box{position:absolute;left:200px;width:320px;height:80px}
#plain{top:$2px;background:#f4f4f4}
#go{display:block;top:$3px;background:#eeeeee}
#go:hover{background:#cc0000}
#text{position:absolute;left:200px;top:300px;margin:0;line-height:80px}
#nocursor{top:420px;background:#f4f4f4;cursor:none}
</style></head><body>
<div id="plain" class="box"></div>
<a id="go" class="box" href="cursor-next.html"></a>
<p id="text">Words to hover over</p>
<div id="nocursor" class="box"></div>
</body></html>
HTML
}

# Fixed test keys: deterministic addresses; never fund them.
make_keys() {
    printf '%064d\n' 0 | tr 0 1 >"$WORK/test-wallet.hex"
    printf '%064d\n' 0 | tr 0 2 >"$WORK/recipient.hex"
    local p="$WORK/profiles/_recipient"
    guard_profile "$p"
    rm -rf -- "$p"
    mkdir -p -- "$p"
    printf 'observers =\n' >"$p/settings.conf"
    RECIPIENT=$("$BIN" --profile "$p" wallet import "$WORK/recipient.hex" "Recipient" 2>>"$WORK/logs/keys.log" | tail -n1)
    [[ -n $RECIPIENT ]] || die 2 "could not derive the recipient address (see $WORK/logs/keys.log)"
}

std_tabs() {
    jq -cn --arg site "$SITE_URL" '[
      {url: ($site + "/notes.html"), title: "Field notes on gaze and capability", parent: null},
      {url: "gaze://about", title: "About F1R3Gaze", parent: 0},
      {url: ($site + "/prompt.html"), title: "Cross-site fetch", parent: 0},
      {url: ($site + "/shard.html"), title: "Shard reader", parent: null},
      {url: "gaze://newtab", title: "New tab", parent: 3},
      {url: "https://docs.example.invalid/very/long/path/to/a/page/that/overflows/the/sidebar?query=capability",
       title: "A page with a very long title that should be truncated in the tab strip and the sidebar", parent: 4}
    ]'
}
# Thirty tabs: more than fit at 1280 px even at their narrowest (44 px), so
# the strip shows a window around the active tab and a "+N" chip.
many_tabs() {
    jq -cn --arg site "$SITE_URL" '[
      {url: ($site + "/notes.html"), title: "Field notes on gaze and capability", parent: null}
    ] + [range(1; 30) | {url: ($site + "/plain.html?" + tostring), title: ("Plain page " + tostring), parent: null}]'
}
# Visit times are relative to the moment a scene is seeded, so every scene
# shows the same relative times however long the run has been going.
std_visits() {
    jq -cn --argjson now "$(date +%s)" --arg site "$SITE_URL" '[
      {url: ($site + "/notes.html"), title: "Field notes on gaze and capability", at: ($now - 120)},
      {url: "https://f1r3fly.io/", title: "F1R3FLY.io — home", at: ($now - 1500)},
      {url: "https://github.com/F1R3FLY-io/F1R3Gaze", title: "F1R3FLY-io/F1R3Gaze: a browser whose only execution mechanism is f1r3lang", at: ($now - 3 * 3600)},
      {url: "https://docs.rs/blitz-dom/latest/blitz_dom/", title: "blitz_dom - Rust", at: ($now - 20 * 3600)},
      {url: "f1r3://publisher/project@1/index.html", title: "Shard site index", at: ($now - 2 * 86400)},
      {url: "https://example.org/", title: "Example Domain", at: ($now - 3 * 86400)},
      {url: "https://en.wikipedia.org/wiki/Capability-based_security", title: "Capability-based security - Wikipedia", at: ($now - 12 * 86400)},
      {url: "https://www.rust-lang.org/", title: "Rust Programming Language", at: ($now - 45 * 86400)}
    ]'
}

# new_profile NAME → path. Seeds settings.conf (no shard observers, so no
# event thread dials localhost; Embers only when asked). Scenes seed the
# sidebar's state in workspace.json, so the window restores it as seeded
# (`restore_sidebar`; by default every window starts collapsed).
PROFILE=""
new_profile() {
    PROFILE="$WORK/profiles/$1"
    guard_profile "$PROFILE"
    rm -rf -- "$PROFILE"
    mkdir -p -- "$PROFILE"
    printf 'observers =\nrestore_sidebar = true\n' >"$PROFILE/settings.conf"
}
with_embers() { printf 'embers_api = %s\n' "$EMBERS_URL" >>"$PROFILE/settings.conf"; }
with_wallet() {
    "$BIN" --profile "$PROFILE" wallet import "$WORK/test-wallet.hex" "Snapshot wallet" >>"$WORK/logs/$SCENE.log" 2>&1 ||
        fail "wallet import failed"
}
# seed THEME PANEL SIDEBAR_OPEN TREE_TABS ACTIVE [TABS_JSON]
seed() {
    local tabs=${6:-$(std_tabs)}
    jq -n --arg theme "$1" --arg panel "$2" --argjson open "$3" --argjson tree "$4" \
        --argjson active "$5" --argjson tabs "$tabs" --argjson visits "$(std_visits)" \
        '{theme: $theme, sidebar_open: $open, panel: $panel, tree_tabs: $tree,
          active: $active, tabs: $tabs, visits: $visits}' >"$PROFILE/workspace.json"
}

SCENE=""
SCENE_FAILED=0
WID=""
fail() {
    log "  $SCENE: $*"
    SCENE_FAILED=1
    return 1
}

launch() {
    # The window opens under wherever the previous scene left the pointer, and
    # a page that loads under it would be hovered there. Park the pointer on
    # the status bar first, so no scene depends on the one before it (L8).
    pointer NEUTRAL
    "$BIN" --profile "$PROFILE" "$@" >>"$WORK/logs/$SCENE.log" 2>&1 &
    APP_PID=$!
    (sleep "$SCENE_TIMEOUT" && kill -TERM "$APP_PID") 2>/dev/null &
    WD_PID=$!
    WID=$(timeout 20 xdotool search --sync --onlyvisible --pid "$APP_PID" 2>/dev/null | head -n1) || WID=""
    if [[ -z $WID ]]; then
        WID=$(timeout 5 xdotool search --onlyvisible --name 'F1R3Gaze' 2>/dev/null | head -n1) || WID=""
    fi
    [[ -n $WID ]] || { fail "no window appeared"; return 1; }
    xdotool windowmove "$WID" 0 0
    xdotool windowsize --sync "$WID" "$W" "$H"
    xdotool windowfocus --sync "$WID"
    pointer NEUTRAL
}

# wait_title SUBSTRING [SECS]: readiness is the window title "<page> — F1R3Gaze".
wait_title() {
    local want=$1 deadline=$((SECONDS + ${2:-20})) name
    while :; do
        name=$(xdotool getwindowname "$WID" 2>/dev/null || true)
        [[ $name == *"$want"* ]] && return 0
        ((SECONDS < deadline)) || { fail "window title stayed '$name' (wanted '$want')"; return 1; }
        sleep 0.1
    done
}

# ae A B [CROP] → number of differing pixels.
ae() {
    local a=$1 b=$2 crop=${3:-}
    if [[ -n $crop ]]; then
        magick compare -metric AE \( "$a" -crop "$crop" +repage \) \( "$b" -crop "$crop" +repage \) null: 2>&1 | awk '{print $1 + 0}'
    else
        magick compare -metric AE "$a" "$b" null: 2>&1 | awk '{print $1 + 0}'
    fi
}

# Two captures 300 ms apart must match (a blinking caret is tolerated).
wait_stable() {
    local a="$WORK/refs/.stable-a.png" b="$WORK/refs/.stable-b.png" deadline=$((SECONDS + 8)) diff
    import -window root "$a"
    while :; do
        sleep 0.3
        import -window root "$b"
        diff=$(ae "$a" "$b")
        ((diff <= 64)) && return 0
        ((SECONDS < deadline)) || { log "  $SCENE: not stable after 8 s ($diff px still changing)"; return 0; }
        mv -f -- "$b" "$a"
    done
}
shot() {
    sleep "${1:-0.4}"
    wait_stable
    import -window root "$OUT/$SCENE.png"
}
ref() {
    sleep "${2:-0.4}"
    wait_stable
    import -window root "$WORK/refs/$SCENE.$1.png"
}

pointer() { local xy=(${AT[$1]}); xdotool mousemove "${xy[0]}" "${xy[1]}"; }
click() {
    local xy=(${AT[$1]})
    [[ ${#xy[@]} -eq 2 ]] || { fail "no coordinate $1 in $MODE mode"; return 1; }
    xdotool mousemove "${xy[0]}" "${xy[1]}" click "${2:-1}"
    sleep "${3:-0.5}"
}
key() { xdotool key --clearmodifiers "$@"; sleep 0.35; }
typeit() { xdotool type --delay 60 "$1"; sleep 0.5; }

# ── Checks ───────────────────────────────────────────────────────────────
CHECKS="$OUT/checks.tsv"
CHECK_FAILED=0
# check SCENE NAME VALUE OP LIMIT: OP is le (≤), ge (≥), or eq (=). Recorded
# before, enforced after.
check() {
    local scene=$1 name=$2 value=$3 op=$4 limit=$5 verdict
    if [[ $MODE == before ]]; then
        verdict=recorded
    elif awk -v v="$value" -v l="$limit" -v op="$op" 'BEGIN { exit !((op == "le" && v <= l) || (op == "ge" && v >= l) || (op == "eq" && v == l)) }'; then
        verdict=pass
    else
        verdict=FAIL
        CHECK_FAILED=1
    fi
    printf '%s\t%s\t%s\t%s %s\t%s\n' "$scene" "$name" "$value" "$op" "$limit" "$verdict" >>"$CHECKS"
    log "  check $name = $value ($op $limit) $verdict"
}
# Colour-histogram distance: how many pixels would have to change colour to
# turn one image's palette into the other's. Unlike a pixel-exact diff it
# ignores glyphs that merely moved by a pixel, and it still counts every
# label painted in a stale colour.
hist_diff() {
    gawk 'FNR == 1 { f++ }
      match($0, /^ *([0-9]+): .*(#[0-9A-Fa-f]{6})/, m) {
        if (f == 1) a[m[2]] += m[1]; else b[m[2]] += m[1]; seen[m[2]] = 1
      }
      END { for (c in seen) { d = a[c] - b[c]; s += d < 0 ? -d : d }; printf "%d\n", s / 2 }' \
        <(magick "$1" -alpha off -format %c histogram:info:-) <(magick "$2" -alpha off -format %c histogram:info:-)
}
count_color() { magick "$1" -alpha off -format %c histogram:info:- | gawk -v c="$2" 'toupper($0) ~ c { n += $1 } END { print n + 0 }'; }
# count_color_in IMAGE CROP COLOUR: pixels of exactly COLOUR inside CROP.
count_color_in() { magick "$1" -crop "$2" +repage -alpha off -format %c histogram:info:- | gawk -v c="$3" 'toupper($0) ~ c { n += $1 } END { print n + 0 }'; }
# Highest WCAG contrast between the dominant colour of a crop and any colour
# covering at least 4 pixels: ~1.0 when a label is invisible, >3 when legible.
max_contrast() {
    magick "$1" -crop "$2" +repage -alpha off -format %c histogram:info:- | gawk '
      function lin(c) { c /= 255; return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ^ 2.4 }
      function lum(r, g, b) { return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b) }
      match($0, /^ *([0-9]+): *\(([0-9.]+),([0-9.]+),([0-9.]+)/, m) {
        n = m[1] + 0; L = lum(m[2], m[3], m[4]); count[NR] = n; lumi[NR] = L
        if (n > best) { best = n; bg = L }
      }
      END {
        r = 1
        for (i in lumi) if (count[i] >= 4) {
          hi = lumi[i] > bg ? lumi[i] : bg; lo = lumi[i] > bg ? bg : lumi[i]
          x = (hi + 0.05) / (lo + 0.05); if (x > r) r = x
        }
        printf "%.2f\n", r
      }'
}
# Pixels in a band across one edge that are not a blend of the two fills that
# meet there (docs/ui/ledger.md, L5 R1). A is the commonest colour of the
# band's first row (outside the element), B of its last row (inside).
# Anti-aliasing only produces colours on the segment A–B; a hairline of a
# third colour lies off it by more than the 8-step tolerance.
edge_artifact() {
    magick "$1" -crop "$2" +repage -depth 8 txt:- | gawk '
      function hex(s) { return strtonum("0x" s) }
      function mode(arr,    c, best, top) { top = 0; for (c in arr) if (arr[c] > top) { top = arr[c]; best = c }; return best }
      NR > 1 && match($0, /^([0-9]+),([0-9]+):.*#([0-9A-Fa-f]{6})/, m) {
        y = m[2] + 0; c = toupper(m[3]); pixel[++n] = c
        if (y == 0) first[c]++
        if (y > last_y) { last_y = y; delete lastrow }
        if (y == last_y) lastrow[c]++
      }
      END {
        a = mode(first); b = mode(lastrow)
        ar = hex(substr(a, 1, 2)); ag = hex(substr(a, 3, 2)); ab = hex(substr(a, 5, 2))
        dr = hex(substr(b, 1, 2)) - ar; dg = hex(substr(b, 3, 2)) - ag; db = hex(substr(b, 5, 2)) - ab
        len2 = dr * dr + dg * dg + db * db
        for (i = 1; i <= n; i++) {
          pr = hex(substr(pixel[i], 1, 2)); pg = hex(substr(pixel[i], 3, 2)); pb = hex(substr(pixel[i], 5, 2))
          t = len2 > 0 ? ((pr - ar) * dr + (pg - ag) * dg + (pb - ab) * db) / len2 : 0
          t = t < 0 ? 0 : (t > 1 ? 1 : t)
          er = pr - (ar + t * dr); eg = pg - (ag + t * dg); eb = pb - (ab + t * db)
          if (sqrt(er * er + eg * eg + eb * eb) > 8) off++
        }
        print off + 0
      }'
}
# edge_check NAME CROP-KEY: no hairline along an edge (after mode only; the
# elements measured are part of the new design).
edge_check() {
    [[ $MODE == after && -n ${AT[$2]} ]] || return 0
    check "$SCENE" "$1" "$(edge_artifact "$OUT/$SCENE.png" "${AT[$2]}")" le 0
}
# bar_check NAME CROP-KEY COLOUR WIDTH: along a one-pixel line across an edge
# bar, exactly WIDTH pixels have the bar's colour (L5 R1: bars keep their
# width; after mode only).
bar_check() {
    [[ $MODE == after && -n ${AT[$2]} ]] || return 0
    check "$SCENE" "$1" "$(count_color_in "$OUT/$SCENE.png" "${AT[$2]}" "$3")" eq "$4"
}
# effect NAME REF CROP-KEY: the interaction changed the region since the
# reference capture REF. A click that misses changes nothing, so the run
# fails instead of silently capturing the wrong state.
effect() {
    check "$SCENE" "$1" "$(ae "$WORK/refs/$SCENE.$2.png" "$OUT/$SCENE.png" "${AT[$3]}")" ge 1
}

# ── Scenes ───────────────────────────────────────────────────────────────
NOTES_TITLE="Field notes on gaze and capability"

panel_scene() { # PANEL THEME
    new_profile "$SCENE"
    seed "$2" "$1" true false 0
    launch && wait_title "$NOTES_TITLE" && pointer NEUTRAL && shot 0.8
}
scene_01_tabs_dark() { panel_scene tabs dark; }
scene_02_tabs_light() {
    panel_scene tabs light || return 1
    edge_check row_edge_artifact_px CROP_ROW_TOP
    edge_check rail_edge_artifact_px CROP_RAIL_ON_TOP
    bar_check row_bar_px LINE_ROW_BAR '#087F86' 3
    bar_check rail_bar_px LINE_RAIL_BAR '#087F86' 3
    bar_check tab_bar_px LINE_TAB_BAR '#087F86' 2
}
scene_03_history_dark() { panel_scene history dark; }
scene_04_history_light() { panel_scene history light; }
scene_05_sites_dark() { panel_scene sites dark; }
scene_06_sites_light() { panel_scene sites light; }
scene_07_grants_dark() { panel_scene grants dark; }
scene_08_grants_light() { panel_scene grants light; }
scene_09_wallet_empty_dark() { panel_scene wallet dark; }
scene_10_wallet_empty_light() { panel_scene wallet light; }
wallet_one() {
    new_profile "$SCENE"
    with_embers
    with_wallet || return 1
    seed "$1" wallet true false 0
    launch && wait_title "$NOTES_TITLE" && shot 1.5
}
scene_11_wallet_one_dark() { wallet_one dark; }
scene_12_wallet_one_light() { wallet_one light; }
scene_13_console_dark() { panel_scene console dark; }
scene_14_console_light() { panel_scene console light; }
scene_15_appearance_dark() { panel_scene appearance dark; }
scene_16_appearance_light() {
    panel_scene appearance light || return 1
    edge_check notice_edge_artifact_px CROP_NOTICE_TOP
    bar_check notice_bar_px LINE_NOTICE_BAR '#087F86' 3
}
scene_17_appearance_bad_palette() {
    new_profile "$SCENE"
    printf -- '--gaze-bg: #161b26;\nthis line is not a palette entry\n' >"$PROFILE/palette.css"
    seed dark appearance true false 0
    launch && wait_title "$NOTES_TITLE" && shot 0.8
}
scene_18_appearance_low_contrast() {
    new_profile "$SCENE"
    printf -- '--gaze-surface: #202735;\n--gaze-text: #2a3040;\n' >"$PROFILE/palette.css"
    seed dark appearance true false 0
    launch && wait_title "$NOTES_TITLE" && shot 0.8
}
scene_19_collapsed_dark() { new_profile "$SCENE"; seed dark tabs false false 0; launch && wait_title "$NOTES_TITLE" && shot 0.8; }
scene_20_collapsed_light() { new_profile "$SCENE"; seed light tabs false false 0; launch && wait_title "$NOTES_TITLE" && shot 0.8; }

scene_21_tab_menu() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && pointer NEUTRAL && ref closed && click TAB1 3 0.8 && pointer NEUTRAL && shot || return 1
    effect menu_opened_px closed CROP_SIDEBAR
}
scene_22_tab_menu_after_action() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && click TAB1 3 0.8 && pointer NEUTRAL && ref menu &&
        click MENU_DUPLICATE 1 1.5 && pointer NEUTRAL && shot 1 || return 1
    effect duplicated_px menu CROP_STRIP
}
scene_23_tabs_search() {
    new_profile "$SCENE"
    seed dark tabs true false 0
    launch && wait_title "$NOTES_TITLE" && click SIDE_SEARCH && typeit gaze && shot
}
scene_24_tabs_tree() { new_profile "$SCENE"; seed dark tabs true true 0; launch && wait_title "$NOTES_TITLE" && shot 0.8; }
scene_25_many_tabs() { new_profile "$SCENE"; seed dark tabs false false 0 "$(many_tabs)"; launch && wait_title "$NOTES_TITLE" && shot 0.8; }
scene_26_history_search() {
    new_profile "$SCENE"
    seed dark history true false 0
    launch && wait_title "$NOTES_TITLE" && click SIDE_SEARCH && typeit wiki && shot
}

find_base() { new_profile "$SCENE"; seed dark tabs false false 0; launch && wait_title "$NOTES_TITLE"; }
scene_27_find_open() {
    find_base || return 1
    ref closed 0.8
    key ctrl+f
    shot
    check "$SCENE" page_reflow_px "$(ae "$WORK/refs/$SCENE.closed.png" "$OUT/$SCENE.png" "${AT[CROP_PAGE_LEFT]}")" le 0
}
scene_28_find_matches() { find_base && key ctrl+f && typeit alpha && shot; }
scene_29_find_next() { find_base && key ctrl+f && typeit alpha && key Return && shot; }
scene_30_find_cleared() {
    find_base && key ctrl+f && typeit alpha && key Return || return 1
    local i
    for i in 1 2 3 4 5; do key BackSpace; done
    shot 0.8
    check "$SCENE" selection_pixels "$(count_color "$OUT/$SCENE.png" '#B4D5FF')" le 0
}
scene_31_find_tab_switch() {
    new_profile "$SCENE"
    local tabs
    tabs=$(jq -cn --arg site "$SITE_URL" '[
      {url: ($site + "/notes.html"), title: "Field notes on gaze and capability", parent: null},
      {url: ($site + "/find.html"), title: "Second page", parent: null}]')
    seed dark tabs false false 0 "$tabs"
    launch && wait_title "$NOTES_TITLE" && key ctrl+f && typeit alpha && key ctrl+2 && wait_title "Second page" && shot 1
}
scene_32_find_scroll() {
    new_profile "$SCENE"
    local tabs
    tabs=$(jq -cn --arg site "$SITE_URL" '[{url: ($site + "/long.html"), title: "Long page", parent: null}]')
    seed dark tabs false false 0 "$tabs"
    launch && wait_title "Long page" && key ctrl+f && typeit marker && pointer PAGE && xdotool click --repeat 3 --delay 120 5 && shot 1
}

url_base() { new_profile "$SCENE"; seed dark tabs false false 0; launch && wait_title "$NOTES_TITLE" && key ctrl+l && key ctrl+a && key BackSpace; }
scene_33_url_empty() {
    url_base && shot || return 1
    cp -f -- "$OUT/$SCENE.png" "$WORK/refs/url-empty.png"
}
scene_34_url_suggestions() {
    url_base && ref empty && typeit cap && shot || return 1
    check "$SCENE" rail_reflow_px "$(ae "$WORK/refs/$SCENE.empty.png" "$OUT/$SCENE.png" "${AT[CROP_RAIL]}")" le 0
}
scene_35_url_suggestion_down() { url_base && typeit cap && key Down && shot; }
scene_36_url_escape() { url_base && typeit cap && key Escape && shot; }
scene_37_url_blur() { url_base && typeit cap && click PAGE && pointer NEUTRAL && shot; }
scene_38_url_switch_to_tab() {
    url_base && typeit 'Shard reader' && key Down && key Return || return 1
    case $MODE in
    after) wait_title "Shard reader" ;;
    before) wait_title "Could not load" ;; # Enter navigated to the typed text
    esac
    shot 1
}

scene_39_prompt_net() { new_profile "$SCENE"; seed dark tabs false false 2; launch && wait_title "Cross-site fetch" && shot 1.5; }
scene_40_prompt_shard() { new_profile "$SCENE"; seed dark tabs false false 3; launch && wait_title "Shard reader" && shot 1.5; }
scene_41_error_page() {
    new_profile "$SCENE"
    seed dark tabs false false 5
    launch || return 1
    case $MODE in
    after) wait_title "Can't open this page" ;;
    before) wait_title "Could not load" ;;
    esac
    shot 1
}
scene_42_newtab() { new_profile "$SCENE"; seed dark tabs false false 4; launch && wait_title "New tab" && shot 1; }

scene_43_nav_disabled() { new_profile "$SCENE"; seed dark tabs false false 0; launch && wait_title "$NOTES_TITLE" && shot 0.8; }
scene_44_nav_enabled() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && key ctrl+l && key ctrl+a && typeit "$SITE_URL/plain.html" && key Return && wait_title "Plain page" && pointer NEUTRAL && shot 1
}
scene_45_rail_hover() {
    new_profile "$SCENE"
    seed dark tabs true false 0
    launch && wait_title "$NOTES_TITLE" && pointer NEUTRAL && ref idle && pointer RAIL_HISTORY && shot 0.8 || return 1
    effect bubble_px idle CROP_SIDEBAR
}
# An inactive row: the active row shows its actions without hovering.
scene_46_row_hover() {
    new_profile "$SCENE"
    seed dark tabs true false 0
    launch && wait_title "$NOTES_TITLE" && pointer NEUTRAL && ref idle && pointer SECOND_ROW && shot 0.8 || return 1
    effect hover_px idle CROP_SIDEBAR
}
scene_47_flash_shown() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && click TAB1 3 0.8 && pointer NEUTRAL && ref menu &&
        click MENU_COPY 1 0.3 && pointer NEUTRAL && shot 0.2 || return 1
    cp -f -- "$OUT/$SCENE.png" "$WORK/refs/flash-shown.png"
    effect flash_shown_px menu CROP_STATUS
}
scene_48_flash_expired() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && click TAB1 3 0.8 && click MENU_COPY 1 0.3 && pointer NEUTRAL || return 1
    ref shown 0.2
    sleep 5
    shot 0.2
    check "$SCENE" flash_cleared_px "$(ae "$WORK/refs/$SCENE.shown.png" "$OUT/$SCENE.png" "${AT[CROP_STATUS]}")" ge 1
}
scene_49_search_reset() {
    new_profile "$SCENE"
    seed dark tabs true false 0
    launch && wait_title "$NOTES_TITLE" && click SIDE_SEARCH && typeit gaze && click RAIL_HISTORY && pointer NEUTRAL && shot || return 1
    # The History panel opens unfiltered: exactly as when it is open from the
    # start (scene 03), whatever was typed in the Tabs panel's search.
    if [[ -f $OUT/03-history-dark.png ]]; then
        check "$SCENE" sidebar_px_vs_03 "$(ae "$OUT/03-history-dark.png" "$OUT/$SCENE.png" "${AT[CROP_SIDEBAR]}")" le 0
    fi
}
scene_50_wallet_review() {
    new_profile "$SCENE"
    with_embers
    with_wallet || return 1
    seed dark wallet true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 1 && pointer NEUTRAL && ref empty &&
        click WALLET_TO && typeit "$RECIPIENT" && pointer NEUTRAL && ref recipient &&
        click WALLET_AMOUNT && typeit 50 && pointer NEUTRAL && ref amount &&
        click WALLET_REVIEW 1 0.8 && pointer NEUTRAL && shot || return 1
    # Each field took its typing, and Review then showed the transfer.
    local refs="$WORK/refs/$SCENE"
    check "$SCENE" recipient_px "$(ae "$refs.empty.png" "$refs.recipient.png" "${AT[CROP_SIDEBAR]}")" ge 1
    check "$SCENE" amount_px "$(ae "$refs.recipient.png" "$refs.amount.png" "${AT[CROP_SIDEBAR]}")" ge 1
    effect review_px amount CROP_SIDEBAR
}
scene_51_history_clear() {
    new_profile "$SCENE"
    seed dark history true false 0
    launch && wait_title "$NOTES_TITLE" && click HISTORY_CLEAR && pointer NEUTRAL && shot
}

# Theme group: switching in place must look exactly like starting fresh in
# that scheme. A stale label (W1) shows up as a pixel difference.
theme_switch() { # TARGET_COORD ACTIVE_TAB TITLE FROM_THEME
    new_profile "$SCENE"
    seed "$4" appearance true false "$2"
    launch && wait_title "$3" && sleep 1 && click "$1" 1 1 && pointer NEUTRAL && shot 0.8
}
theme_fresh() { # THEME ACTIVE_TAB TITLE SWITCHED_SCENE
    new_profile "$SCENE"
    seed "$1" appearance true false "$2"
    launch && wait_title "$3" && sleep 1 && pointer NEUTRAL && shot 0.8 || return 1
    if [[ -f $OUT/$4.png ]]; then
        check "$SCENE" "stale_colour_px_vs_${4%%-*}" "$(hist_diff "$OUT/$4.png" "$OUT/$SCENE.png")" le 0
    fi
}
scene_52_theme_appearance_to_light() { theme_switch APPEARANCE_LIGHT 0 "$NOTES_TITLE" dark; }
scene_53_theme_appearance_fresh_light() { theme_fresh light 0 "$NOTES_TITLE" 52-theme-appearance-to-light; }
scene_54_theme_appearance_to_dark() { theme_switch APPEARANCE_DARK 0 "$NOTES_TITLE" light; }
scene_55_theme_appearance_fresh_dark() { theme_fresh dark 0 "$NOTES_TITLE" 54-theme-appearance-to-dark; }
scene_56_theme_prompt_to_light() {
    theme_switch APPEARANCE_LIGHT_PROMPTED 2 "Cross-site fetch" dark || return 1
    check "$SCENE" allow_label_contrast "$(max_contrast "$OUT/$SCENE.png" "${AT[CROP_PROMPT_ALLOW]}")" ge 3
}
scene_57_theme_prompt_fresh_light() {
    theme_fresh light 2 "Cross-site fetch" 56-theme-prompt-to-light || return 1
    bar_check prompt_bar_px LINE_PROMPT_BAR '#955300' 3
}
scene_58_theme_newtab_to_light() {
    theme_switch APPEARANCE_LIGHT 4 "New tab" dark || return 1
    check "$SCENE" lamp_label_contrast "$(max_contrast "$OUT/$SCENE.png" "${AT[CROP_LAMP]}")" ge 3
}
scene_59_theme_newtab_fresh_light() { theme_fresh light 4 "New tab" 58-theme-newtab-to-light; }
# W1's first observation: the scheme switches while the Wallet panel is open.
# Before, the toolbar's palette button switched it in place, and the static
# wallet form's labels vanished. The redesign switches schemes in
# Appearance, so after mode goes there and comes back to the Wallet. Its
# sidebar must then match scene 12, which starts in light with the same wallet:
# a stale label, a missed click, or a panel left behind would all differ.
scene_60_theme_wallet_to_light() {
    new_profile "$SCENE"
    case $MODE in
    before)
        with_wallet || return 1
        seed dark wallet true false 0
        launch && wait_title "$NOTES_TITLE" && sleep 1 && click PALETTE 1 1 && pointer NEUTRAL && shot 0.8 || return 1
        check "$SCENE" export_label_contrast "$(max_contrast "$OUT/$SCENE.png" "${AT[CROP_WALLET_EXPORT]}")" ge 3
        ;;
    after)
        with_embers
        with_wallet || return 1
        seed dark wallet true false 0
        launch && wait_title "$NOTES_TITLE" && sleep 1 &&
            click RAIL_APPEARANCE 1 1 && click APPEARANCE_LIGHT 1 1 && click RAIL_WALLET 1 1.5 &&
            pointer NEUTRAL && shot 1 || return 1
        if [[ -f $OUT/12-wallet-one-light.png ]]; then
            check "$SCENE" sidebar_px_vs_12 "$(ae "$OUT/12-wallet-one-light.png" "$OUT/$SCENE.png" "${AT[CROP_SIDEBAR]}")" le 0
        fi
        ;;
    esac
}

# ── Cursor scenes (docs/ui/ledger.md, L8) ────────────────────────────────
# A screen capture does not contain the X cursor, so these scenes read it
# from the server (scripts/x-cursor.py). Each one compares it with reference
# sprites taken in the same run, which keeps the checks independent of the
# cursor theme: a hand over a toolbar button (button{cursor:pointer}), a text
# cursor over the address field, and the arrow over the rail's empty middle.
# Every sprite read is kept in cursors/ next to the captures.
PROBE="$ROOT/scripts/x-cursor.py"
CURSOR_TITLE="Cursor page"
C_OPAQUE=0
C_SHA=""
REF_HAND=""
REF_TEXT=""
REF_ARROW=""

# cursor_probe NAME: read the cursor shown now; sets C_OPAQUE and C_SHA.
cursor_probe() {
    local line
    sleep 0.3
    mkdir -p -- "$OUT/cursors"
    line=$(python3 "$PROBE" --png "$OUT/cursors/$SCENE.$1.png") || { fail "the cursor probe failed"; return 1; }
    read -r _ _ _ _ C_OPAQUE C_SHA <<<"$line"
    log "  cursor $1: $line"
}
same() { [[ $1 == "$2" ]] && echo 1 || echo 0; }
# The reference sprites. Moving over them never touches the page, so a page
# that has not been under the pointer stays that way.
cursor_refs() {
    pointer RELOAD && cursor_probe ref-hand && REF_HAND=$C_SHA &&
        pointer URL_FIELD && cursor_probe ref-text && REF_TEXT=$C_SHA &&
        pointer RAIL_MID && cursor_probe ref-arrow && REF_ARROW=$C_SHA || return 1
    local distinct=0
    [[ $REF_HAND != "$REF_TEXT" && $REF_HAND != "$REF_ARROW" && $REF_TEXT != "$REF_ARROW" ]] && distinct=1
    check "$SCENE" cursor_refs_distinct "$distinct" eq 1
}
# cursor_is NAME REFERENCE CHECK: the cursor shown now has the reference shape.
cursor_is() {
    cursor_probe "$1" || return 1
    check "$SCENE" "$3" "$(same "$C_SHA" "$2")" eq 1
}
# cursor_visible_arrow NAME: the arrow, visible.
cursor_visible_arrow() {
    cursor_probe "$1" || return 1
    check "$SCENE" "${1}_opaque_px" "$C_OPAQUE" ge 1
    check "$SCENE" "${1}_is_arrow" "$(same "$C_SHA" "$REF_ARROW")" eq 1
}
cursor_tabs() {
    jq -cn --arg site "$SITE_URL" '[
      {url: ($site + "/cursor.html"), title: "Cursor page", parent: null},
      {url: ($site + "/notes.html"), title: "Field notes on gaze and capability", parent: null}
    ]'
}

# C1: Back loads a new document, and the pointer goes from Back into it.
scene_61_cursor_back() {
    new_profile "$SCENE"
    seed dark tabs false false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && cursor_refs &&
        key ctrl+l && key ctrl+a && typeit "$SITE_URL/cursor-next.html" && key Return && wait_title "Cursor next" &&
        click BACK 1 0.3 && wait_title "$CURSOR_TITLE" && sleep 0.5 && pointer PAGE_PLAIN && shot 0.5 || return 1
    cursor_visible_arrow page
}
# C2: a rail button, then into a page that has not been under the pointer.
scene_62_cursor_rail() {
    new_profile "$SCENE"
    seed dark tabs true false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && cursor_refs && click RAIL_HISTORY 1 0.5 &&
        pointer PAGE_PLAIN_OPEN && shot 0.5 || return 1
    cursor_visible_arrow page
}
# C3: a Tabs-panel row selects a tab, whose page loads on selection.
scene_63_cursor_tab_row() {
    new_profile "$SCENE"
    seed dark tabs true false 1 "$(cursor_tabs)"
    launch && wait_title "$NOTES_TITLE" && cursor_refs && click FIRST_ROW 1 0.3 &&
        wait_title "$CURSOR_TITLE" && sleep 0.5 && pointer PAGE_PLAIN_OPEN && shot 0.5 || return 1
    cursor_visible_arrow page
}
# C4: a hand over a link inside the page.
scene_64_cursor_link() {
    new_profile "$SCENE"
    seed dark tabs false false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && cursor_refs && pointer PAGE_PLAIN && sleep 0.3 &&
        pointer PAGE_GO && shot 0.5 || return 1
    cursor_is link "$REF_HAND" link_is_hand
}
# C5: a text cursor over the page's text.
scene_65_cursor_text() {
    new_profile "$SCENE"
    seed dark tabs false false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && cursor_refs && pointer PAGE_PLAIN && sleep 0.3 &&
        pointer PAGE_TEXT && shot 0.5 || return 1
    cursor_is text "$REF_TEXT" text_is_text_cursor
}
# C6: the link's :hover style is painted while the pointer is on it, and
# cleared once the pointer leaves the page.
scene_66_cursor_hover_style() {
    new_profile "$SCENE"
    seed dark tabs false false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && pointer PAGE_PLAIN && sleep 0.3 && pointer PAGE_GO &&
        ref hovered 0.5 && pointer RAIL_MID && shot 0.5 || return 1
    check "$SCENE" hover_fill_px "$(count_color_in "$WORK/refs/$SCENE.hovered.png" "${AT[CROP_GO]}" '#CC0000')" ge 15000
    check "$SCENE" left_fill_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_GO]}" '#CC0000')" eq 0
}
# C7: a page loads under a resting pointer; its link lands where the
# previous page's plain block was.
scene_67_cursor_resting() {
    new_profile "$SCENE"
    seed dark tabs false false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && cursor_refs && pointer PAGE_PLAIN && sleep 0.3 &&
        key ctrl+l && key ctrl+a && typeit "$SITE_URL/cursor-next.html" && key Return &&
        wait_title "Cursor next" && sleep 0.5 && shot 0.5 || return 1
    cursor_is page "$REF_HAND" resting_is_hand
}
# C8: `cursor: none` hides the cursor over that element only.
scene_68_cursor_css_none() {
    new_profile "$SCENE"
    seed dark tabs false false 0 "$(cursor_tabs)"
    launch && wait_title "$CURSOR_TITLE" && cursor_refs && pointer PAGE_PLAIN && sleep 0.3 &&
        pointer PAGE_NOCURSOR && cursor_probe hidden || return 1
    check "$SCENE" hidden_opaque_px "$C_OPAQUE" eq 0
    pointer PAGE_PLAIN && shot 0.5 || return 1
    cursor_visible_arrow back
}

# ── Calibration ──────────────────────────────────────────────────────────
calibrate() {
    SCENE=calibrate
    new_profile calibrate
    seed dark tabs true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 1 || die 1 "calibration scene failed"
    local img="$WORK/calibrate-$MODE.png" draw=() name xy
    import -window root "$img"
    for name in "${!AT[@]}"; do
        [[ ${AT[$name]} =~ ^([0-9]+)\ ([0-9]+)$ ]] || continue
        xy=(${AT[$name]})
        draw+=(-draw "line $((xy[0] - 8)),${xy[1]} $((xy[0] + 8)),${xy[1]}" -draw "line ${xy[0]},$((xy[1] - 8)) ${xy[0]},$((xy[1] + 8))"
            -draw "text $((xy[0] + 6)),$((xy[1] - 6)) '$name'")
    done
    magick "$img" -stroke '#ff2d55' -fill '#ff2d55' -pointsize 10 "${draw[@]}" "$img"
    stop_app
    log "calibration image: $img"
}

# ── Run ──────────────────────────────────────────────────────────────────
start_xvfb
make_site
if ((CALIBRATE)); then
    calibrate
    exit 0
fi
start_embers
make_keys
# A partial run (--only) replaces only its own scenes' rows.
if [[ -n $ONLY && -s $CHECKS ]]; then
    gawk -F '\t' -v only="$ONLY" 'NR == 1 || $1 !~ only' "$CHECKS" >"$CHECKS.keep"
    mv -f -- "$CHECKS.keep" "$CHECKS"
else
    printf 'scene\tcheck\tvalue\tlimit\tverdict\n' >"$CHECKS"
fi
RUN_DATE=$(date -Is)
GIT_REV="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)$(git -C "$ROOT" diff --quiet 2>/dev/null || echo ' (dirty)')"
BIN_SHA=$(sha256sum "$BIN" | cut -d' ' -f1)
{
    printf 'mode\t%s\n' "$MODE"
    printf 'date\t%s\n' "$RUN_DATE"
    printf 'git\t%s\n' "$GIT_REV"
    printf 'binary\t%s\n' "$BIN_SHA"
    printf 'imagemagick\t%s\n' "$(magick -version | head -n1)"
    printf 'size\t%sx%s\n' "$W" "$H"
    printf 'work\t%s\n' "$WORK"
    printf 'only\t%s\n' "${ONLY:-all scenes}"
} >"$OUT/run.txt"

FAILED=()
RAN=0
for name in "${SCENES[@]}"; do
    [[ -z $ONLY || $name =~ $ONLY ]] || continue
    if [[ -n ${ONLY_IN[$name]:-} && ${ONLY_IN[$name]} != "$MODE" ]]; then
        log "$name: not reachable in $MODE mode (only ${ONLY_IN[$name]}); skipped by design"
        continue
    fi
    SCENE=$name
    SCENE_FAILED=0
    : >"$WORK/logs/$SCENE.log"
    log "$name"
    if ! "scene_${name//-/_}" || ((SCENE_FAILED)); then
        FAILED+=("$name")
    fi
    stop_app
    RAN=$((RAN + 1))
done

if ((RAN)); then
    shopt -s nullglob
    pngs=("$OUT"/[0-9][0-9]-*.png)
    if ((${#pngs[@]})); then
        montage "${pngs[@]}" -label '%t' -tile 4x -geometry 320x200+6+6 -background '#202020' -fill '#dddddd' \
            -title "F1R3Gaze UI — $MODE" "$OUT/contact-sheet.png"
    fi
fi
log "ran $RAN scenes; ${#FAILED[@]} failed${FAILED:+: ${FAILED[*]}}"
log "checks: $CHECKS"
# run.txt describes only the latest run; runs.tsv keeps one line per run, so
# the provenance of every image survives partial re-runs (--only).
RUNS="$OUT/runs.tsv"
[[ -s $RUNS ]] || printf 'date\tmode\tonly\tbinary\tgit\tscenes\tfailed\tchecks_failed\n' >"$RUNS"
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$RUN_DATE" "$MODE" "${ONLY:-all scenes}" "$BIN_SHA" "$GIT_REV" \
    "$RAN" "${#FAILED[@]}" "$CHECK_FAILED" >>"$RUNS"
((${#FAILED[@]} == 0 && CHECK_FAILED == 0)) || exit 1
