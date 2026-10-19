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
# --timeout SECS per scene (default 25), --keep,
# --seed-format current|legacy (default current: config/settings.toml and
# state/session.json, state/history.json; legacy: the single-folder files,
# for binaries from before the five-root layout, such as the S1 baseline),
# --check-isolation (check the isolation and the work directory, then exit).
#
# Exit codes: 0 ok; 1 a scene or an enforced check failed; 2 a tool, the
# binary, or the display is missing; 3 an unsafe path was refused, or the
# isolation self-test failed; 64 usage error.
#
# Safety (scripts/lib/storage-isolation.bash; docs/storage/README.md,
# "Verifying storage"): every profile is a portable root under the work
# directory (held with flock). Every XDG variable points into the work
# directory, F1R3GAZE_PROFILE too, and so does HOME for the one run without
# --profile (the self-test): an old profile is found through HOME, and a
# start would move it. There is no session bus: a fake dbus-send answers the
# colour-scheme question from $WORK/portal/scheme. Every new profile resets
# it to 0 (no preference), and the system-scheme scenes set the answer they
# need (`portal`: 1 dark, 2 light, hang, fail). Scenes 94 and 95 run with
# no --profile, each in a HOME and XDG folders of its own under the work
# directory, checked with `paths` before they start. Scenes 98 and 99 run a
# window manager (openbox), and need it installed. The Vulkan
# driver is named directly (VK_DRIVER_FILES), as its folder leaves
# $XDG_DATA_DIRS. A guard refuses a work directory or profile that is, holds
# or is inside a real F1R3Gaze folder, and before Xvfb starts, `f1r3gaze
# paths` must name only the work directory (current format). The work
# directory is on disk, not /tmp, which is often tmpfs (RAM). Wallets are
# imported from fixed test keys that are never funded.
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
SEED_FORMAT=current
CHECK_ISOLATION=0

usage() {
    # Was a fixed range of lines (2-42), which the header outgrew in step 10:
    # the header's comment lines, up to the first empty line.
    # sed -n '2,42p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed -n 's/^# \{0,1\}//p'
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
    --seed-format) SEED_FORMAT=${2:?--seed-format needs current or legacy}; shift ;;
    --check-isolation) CHECK_ISOLATION=1 ;;
    -h | --help) usage ;;
    *) printf 'unknown option: %s\n' "$1" >&2; usage ;;
    esac
    shift
done

[[ $SIZE =~ ^([0-9]+)x([0-9]+)$ ]] || { printf 'bad --size %s\n' "$SIZE" >&2; exit 64; }
[[ $SEED_FORMAT == current || $SEED_FORMAT == legacy ]] || { printf 'bad --seed-format %s\n' "$SEED_FORMAT" >&2; exit 64; }
# shellcheck source-path=SCRIPTDIR source=lib/storage-isolation.bash
source "$ROOT/scripts/lib/storage-isolation.bash"
# The real folders, from the real environment, before anything is overridden.
storage_real_roots
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
    69-lamp-lit-dark 70-lamp-lit-light 71-lamp-out-dark
    72-system-dark 73-system-light 74-system-none 75-system-fail 76-system-refocus 77-system-hang
    78-theme-list 79-theme-file-switch 80-theme-file-fresh 81-theme-new-reload 82-theme-chosen-broken
    83-page-scheme-dark 84-page-scheme-light
    85-window-first-start 86-window-restored 87-window-off-screen 88-window-saved-while-open
    89-window-move-resize-close 90-window-zoom 91-window-zoom-restored 92-window-zoom-floor 93-fullscreen-f11
    94-legacy-migration 95-migration-conflict 96-corrupt-state 97-second-instance
    98-fullscreen-wm 99-maximize-wm
)
# Scene 60 used to be before-only (it needed the removed palette button); it now
# reaches the same state through Appearance in after mode. The cursor scenes
# (L8) and the lamp scenes (L11) came after the before capture and use only the
# after coordinates.
declare -A ONLY_IN=(
    [61-cursor-back]=after [62-cursor-rail]=after [63-cursor-tab-row]=after
    [64-cursor-link]=after [65-cursor-text]=after [66-cursor-hover-style]=after
    [67-cursor-resting]=after [68-cursor-css-none]=after
    [69-lamp-lit-dark]=after [70-lamp-lit-light]=after [71-lamp-out-dark]=after
    [72-system-dark]=after [73-system-light]=after [74-system-none]=after [75-system-fail]=after
    [76-system-refocus]=after [77-system-hang]=after [78-theme-list]=after [79-theme-file-switch]=after
    [80-theme-file-fresh]=after [81-theme-new-reload]=after [82-theme-chosen-broken]=after
    [83-page-scheme-dark]=after [84-page-scheme-light]=after
    [85-window-first-start]=after [86-window-restored]=after [87-window-off-screen]=after
    [88-window-saved-while-open]=after [89-window-move-resize-close]=after [90-window-zoom]=after
    [91-window-zoom-restored]=after [92-window-zoom-floor]=after [93-fullscreen-f11]=after
    [94-legacy-migration]=after [95-migration-conflict]=after [96-corrupt-state]=after
    [97-second-instance]=after [98-fullscreen-wm]=after [99-maximize-wm]=after
)
# The scenes of the system's scheme, theme files and the pages' scheme
# (ledger S12, part 3; docs/ui/ledger.md, L13) seed what only builds since
# the five-root layout read: they need the current seed format.
declare -A FORMAT_IN=(
    [72-system-dark]=current [73-system-light]=current [74-system-none]=current [75-system-fail]=current
    [76-system-refocus]=current [77-system-hang]=current [78-theme-list]=current [79-theme-file-switch]=current
    [80-theme-file-fresh]=current [81-theme-new-reload]=current [82-theme-chosen-broken]=current
    [83-page-scheme-dark]=current [84-page-scheme-light]=current
    [85-window-first-start]=current [86-window-restored]=current [87-window-off-screen]=current
    [88-window-saved-while-open]=current [89-window-move-resize-close]=current [90-window-zoom]=current
    [91-window-zoom-restored]=current [92-window-zoom-floor]=current [93-fullscreen-f11]=current
    [94-legacy-migration]=current [95-migration-conflict]=current [96-corrupt-state]=current
    [97-second-instance]=current [98-fullscreen-wm]=current [99-maximize-wm]=current
)
# Scenes that need a window manager, which carries out maximize and full
# screen and frames the window (ledger S13, part 2). They run last, so no
# scene without one runs after one.
declare -A NEEDS_WM=([98-fullscreen-wm]=1 [99-maximize-wm]=1)

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
    # The lamp scenes run in after mode only.
    AT[LAMP]=""
    # So do the system-scheme and theme-file scenes (S12, part 3).
    AT[APPEARANCE_SYSTEM]=""
    AT[THEME_ROW_3]=""
    AT[THEME_NEW]=""
    AT[THEME_RELOAD]=""
    AT[PANEL_MID]=""
    AT[CROP_SEGMENT_SYSTEM]=""
    AT[CROP_SCHEME_NOTE]=""
    AT[CROP_THEME_ROWS]=""
    AT[CROP_PANEL_TOP]=""
    # And the window scenes (S13, part 2).
    AT[CROP_ALL]=""
    AT[CROP_ABOVE_STATUS]=""
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
    # System, Dark and Light: three 87 px segments from x=59, 2 px apart
    # (S12, part 3; Dark and Light were the first two of Dark, Light and
    # Custom).
    AT[APPEARANCE_SYSTEM]="103 172"
    AT[APPEARANCE_DARK]="192 172"
    AT[APPEARANCE_LIGHT]="281 172"
    AT[APPEARANCE_DARK_PROMPTED]="192 216"   # the prompt bar adds 44 px
    AT[APPEARANCE_LIGHT_PROMPTED]="281 216"
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
    AT[CROP_NOTICE_TOP]="220x5+80+627"    # the Theme files card's notice, Appearance (light)
    AT[CROP_ROW_TOP]="150x5+90+168"       # the active row, Tabs (light)
    AT[CROP_RAIL_ON_TOP]="16x5+14+88"     # the active rail button
    # One-pixel lines across the bars, clear of the accent-coloured icons.
    AT[LINE_ROW_BAR]="10x1+44+190"        # active row, Tabs (light)
    AT[LINE_RAIL_BAR]="10x1+0+107"        # active rail button
    AT[LINE_TAB_BAR]="1x10+100+0"         # active tab's top bar
    AT[LINE_NOTICE_BAR]="12x1+56+670"     # the Theme files card's notice, Appearance (light)
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
    # Lamp scenes (L11): the centre of CROP_LAMP, with the sidebar open.
    AT[LAMP]="507 350"
    # System-scheme and theme-file scenes (S12, part 3), Appearance open
    # with three theme files (three_themes): the third row (solar), the
    # Theme files card's buttons after scroll_panel_bottom, and a point
    # inside the panel to scroll it from.
    AT[THEME_ROW_3]="190 420"
    AT[THEME_NEW]="110 736"
    AT[THEME_RELOAD]="205 736"
    AT[PANEL_MID]="190 450"
    AT[CROP_SEGMENT_SYSTEM]="85x28+60+159"  # inside the System segment
    AT[CROP_SCHEME_NOTE]="271x47+56+192"    # the sentence under the control
    AT[CROP_THEME_ROWS]="287x190+48+274"    # the Themes list's rows
    AT[CROP_PANEL_TOP]="287x60+48+130"      # the top of the panel: a notice
    # The window scenes (S13, part 2): the whole screen, and all of it but
    # the status bar, where full screen shows how to leave it.
    AT[CROP_ALL]="1280x800+0+0"
    AT[CROP_ABOVE_STATUS]="1280x778+0+0"
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
# Before anything is written there: a work directory that is, holds or is
# inside a real F1R3Gaze folder is refused (exit 3).
storage_guard "$WORK"
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
# The cursor scenes read the X cursor through XFixes (python-xlib); scene 76
# moves the focus with it, and the window scenes close the window with it.
if [[ $MODE == after ]]; then
    for name in "${SCENES[@]}"; do
        [[ ($name == *-cursor-* || $name =~ ^(76|89|90|92|98|99)- ) && (-z $ONLY || $name =~ $ONLY) ]] || continue
        python3 -c 'import Xlib.ext.xfixes' 2>/dev/null ||
            die 2 "missing tool: python-xlib (the cursor scenes read the X cursor with scripts/x-cursor.py; scene 76 moves the focus, and the window scenes close the window)"
        break
    done
    # The window scenes name the X server's monitor (xrandr); 98 and 99 run
    # a window manager.
    for name in "${SCENES[@]}"; do
        [[ $name =~ ^(8[5-9]|9[0-3]|9[89])- && (-z $ONLY || $name =~ $ONLY) ]] || continue
        command -v xrandr >/dev/null || die 2 "missing tool: xrandr (the window scenes name the monitor)"
        break
    done
    for name in "${!NEEDS_WM[@]}"; do
        [[ -z $ONLY || $name =~ $ONLY ]] || continue
        for tool in openbox wmctrl xprop; do
            command -v "$tool" >/dev/null ||
                die 2 "missing tool: $tool (scenes 98 and 99 need a window manager: install the openbox package, or leave them out with --only '^([0-8][0-9]|9[0-7])-')"
        done
        break
    done
fi

# Was, until the switch-over (ledger S14): only XDG_DATA_HOME and
# F1R3GAZE_PROFILE pointed into the work directory, and only the one place
# of the old single-folder profile was refused:
#     USER_PROFILE=${F1R3GAZE_PROFILE:-${XDG_DATA_HOME:-$HOME/.local/share}/f1r3gaze}
#     export XDG_DATA_HOME="$WORK/xdg"
# Refuse a profile outside the work directory, and anything near a real
# F1R3Gaze folder.
guard_profile() {
    local p=$1
    case $p in
    "$WORK"/profiles/*) ;;
    *) die 3 "refusing profile outside the work directory: $p" ;;
    esac
    storage_guard "$p"
}

storage_isolate "$WORK"
export F1R3GAZE_PROFILE="$WORK/profiles/_default"
export WINIT_X11_SCALE_FACTOR=1
unset WAYLAND_DISPLAY
mkdir -p -- "$WORK/profiles" "$WORK/logs" "$WORK/refs" "$OUT"
SITE="$WORK/site"
SITE_URL="file://$SITE"

XVFB_PID=""
APP_PID=""
WD_PID=""
WM_PID=""
EXIT_STATUS=""
EMBERS_PID=""
cleanup() {
    local rc=$?
    stop_app
    stop_wm
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
    # L13's page: its own light and dark styles, chosen by
    # prefers-color-scheme (scenes 83 and 84).
    cat >"$SITE/scheme.html" <<'HTML'
<html><head><title>Scheme page</title><style>body{margin:0;background:#ffffff;color:#3c4043}@media (prefers-color-scheme: dark){body{background:#202124;color:#e8eaed}}</style></head><body><h1>Scheme page</h1><p>The page's own light and dark styles, chosen by prefers-color-scheme.</p></body></html>
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
    case $SEED_FORMAT in
    current)
        mkdir -p -- "$p/config"
        printf '[shard]\nobservers = []\n' >"$p/config/settings.toml"
        ;;
    legacy)
        mkdir -p -- "$p"
        printf 'observers =\n' >"$p/settings.conf"
        ;;
    esac
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

# new_profile NAME → path. Seeds the settings (no shard observers, so no
# event thread dials localhost; Embers only when asked). Scenes seed the
# sidebar's state too, so the window restores it as seeded
# (`restore_sidebar`; by default every window starts collapsed).
#
# The current format is a portable root: config/settings.toml, with the
# scene's theme (S_THEME) and Embers service (S_EMBERS), and
# state/session.json and state/history.json. The legacy format is the
# single folder of builds before the five-root layout: settings.conf and
# workspace.json (control run H0, with the S1 baseline; ledger S14).
PROFILE=""
S_THEME=""
S_EMBERS=""
new_profile() {
    PROFILE="$WORK/profiles/$1"
    guard_profile "$PROFILE"
    rm -rf -- "$PROFILE"
    # No scene inherits another's answer from the fake portal, nor its log.
    portal 0
    : >"$WORK/portal/argv.log"
    case $SEED_FORMAT in
    current)
        mkdir -p -- "$PROFILE/config" "$PROFILE/state"
        S_THEME=""
        S_EMBERS=""
        write_settings
        ;;
    legacy)
        mkdir -p -- "$PROFILE"
        printf 'observers =\nrestore_sidebar = true\n' >"$PROFILE/settings.conf"
        ;;
    esac
}
# write_settings: the current format's config/settings.toml.
write_settings() {
    {
        printf '[appearance]\nrestore_sidebar = true\n'
        if [[ -n $S_THEME ]]; then
            printf 'theme = "%s"\n' "$S_THEME"
        fi
        printf '\n[shard]\nobservers = []\n'
        if [[ -n $S_EMBERS ]]; then
            printf '\n[wallet]\nembers_api = "%s"\n' "$S_EMBERS"
        fi
    } >"$PROFILE/config/settings.toml"
}
with_embers() {
    case $SEED_FORMAT in
    current)
        S_EMBERS=$EMBERS_URL
        write_settings
        ;;
    legacy) printf 'embers_api = %s\n' "$EMBERS_URL" >>"$PROFILE/settings.conf" ;;
    esac
}
# write_palette TEXT: the custom palette, where the format keeps it.
write_palette() {
    case $SEED_FORMAT in
    current)
        mkdir -p -- "$PROFILE/config/themes"
        printf '%s' "$1" >"$PROFILE/config/themes/custom.css"
        ;;
    legacy) printf '%s' "$1" >"$PROFILE/palette.css" ;;
    esac
}
# portal ANSWER: what the fake portal answers from now on: 0, 1 or 2 (no
# preference, dark, light), hang (killed at the query's 1 s deadline) or
# fail (no portal on the bus).
portal() { printf '%s\n' "$1" >"$WORK/portal/scheme"; }
# portal_queries: how many times the fake portal was asked since the
# profile was made.
portal_queries() {
    local n
    n=$(grep -c 'Settings.ReadOne' "$WORK/portal/argv.log" 2>/dev/null) || true
    printf '%s\n' "${n:-0}"
}
# The query's arguments, as the fake logs them (system_theme::portal_args).
PORTAL_ARGV='--session --print-reply=literal --reply-timeout=250 --dest=org.freedesktop.portal.Desktop /org/freedesktop/portal/desktop org.freedesktop.portal.Settings.ReadOne string:org.freedesktop.appearance string:color-scheme'
# write_theme NAME TEXT: the theme file config/themes/NAME.css (current
# format only).
write_theme() {
    mkdir -p -- "$PROFILE/config/themes"
    printf '%s' "$2" >"$PROFILE/config/themes/$1.css"
}
# three_themes: one theme file that cannot be used and two that can, a dark
# one and a light one; the list shows them by name: broken, nord, solar.
three_themes() {
    write_theme nord $'--gaze-accent: #88c0d0;\n'
    write_theme solar $'--gaze-bg: #fdf6e3;\n--gaze-text: #073642;\n'
    write_theme broken $'--gaze-bg: #000000;\nbody { }\n'
}
# scroll_panel_bottom: the sidebar's panel scrolled to its end, where the
# Theme files card is when theme files are listed.
scroll_panel_bottom() { pointer PANEL_MID && xdotool click --repeat 12 --delay 60 5 && sleep 0.5; }
# blur_and_refocus: the window loses the focus, then gains it again
# (Focused(false), then Focused(true)), through python-xlib as the cursor
# scenes use it.
blur_and_refocus() {
    python3 -c 'from Xlib import X, display
d = display.Display(); d.set_input_focus(X.NONE, X.RevertToNone, X.CurrentTime); d.sync()' || return 1
    sleep 0.4
    xdotool windowfocus --sync "$WID"
    sleep 0.4
}
with_wallet() {
    "$BIN" --profile "$PROFILE" wallet import "$WORK/test-wallet.hex" "Snapshot wallet" >>"$WORK/logs/$SCENE.log" 2>&1 ||
        fail "wallet import failed"
}
# seed THEME PANEL SIDEBAR_OPEN TREE_TABS ACTIVE [TABS_JSON]: THEME is dark
# or light (the built-in schemes), or a theme file's name.
seed() {
    local tabs=${6:-$(std_tabs)}
    case $SEED_FORMAT in
    current)
        case $1 in
        dark) S_THEME=default-dark ;;
        light) S_THEME=default-light ;;
        *) S_THEME=$1 ;;
        esac
        write_settings
        jq -n --arg panel "$2" --argjson open "$3" --argjson tree "$4" --argjson active "$5" --argjson tabs "$tabs" \
            '{version: 1, sidebar_open: $open, panel: $panel, tree_tabs: $tree, active: $active, tabs: $tabs}' \
            >"$PROFILE/state/session.json"
        jq -n --argjson visits "$(std_visits)" '{version: 1, visits: $visits}' >"$PROFILE/state/history.json"
        ;;
    legacy)
        jq -n --arg theme "$1" --arg panel "$2" --argjson open "$3" --argjson tree "$4" \
            --argjson active "$5" --argjson tabs "$tabs" --argjson visits "$(std_visits)" \
            '{theme: $theme, sidebar_open: $open, panel: $panel, tree_tabs: $tree,
              active: $active, tabs: $tabs, visits: $visits}' >"$PROFILE/workspace.json"
        ;;
    esac
}

SCENE=""
SCENE_FAILED=0
WID=""
fail() {
    log "  $SCENE: $*"
    SCENE_FAILED=1
    return 1
}

# launch [ARGS…]: the binary on the scene's profile, its window then put at
# (0, 0) and sized to the screen, so captures compare pixel for pixel.
launch() { launch_cmd 1 "$BIN" --profile "$PROFILE" "$@"; }
# launch_restored [ARGS…]: the same, leaving the window where window.json
# put it (S13, part 2).
launch_restored() { launch_cmd 0 "$BIN" --profile "$PROFILE" "$@"; }
# launch_env [ARGS…]: the binary with no --profile, in the scene's own HOME
# and XDG folders (new_machine). env execs the binary, so APP_PID is its.
launch_env() { launch_cmd 0 "${MACHINE_ENV[@]}" "$BIN" "$@"; }
# launch_cmd PLACE COMMAND…: Was the body of launch, which always moved and
# sized the window; PLACE 0 leaves it where it was made.
launch_cmd() {
    local place=$1
    shift
    # The window opens under wherever the previous scene left the pointer, and
    # a page that loads under it would be hovered there. Park the pointer on
    # the status bar first, so no scene depends on the one before it (L8).
    pointer NEUTRAL
    "$@" >>"$WORK/logs/$SCENE.log" 2>&1 &
    APP_PID=$!
    (sleep "$SCENE_TIMEOUT" && kill -TERM "$APP_PID") 2>/dev/null &
    WD_PID=$!
    WID=$(timeout 20 xdotool search --sync --onlyvisible --pid "$APP_PID" 2>/dev/null | head -n1) || WID=""
    if [[ -z $WID ]]; then
        WID=$(timeout 5 xdotool search --onlyvisible --name 'F1R3Gaze' 2>/dev/null | head -n1) || WID=""
    fi
    [[ -n $WID ]] || { fail "no window appeared"; return 1; }
    if ((place)); then
        xdotool windowmove "$WID" 0 0
        xdotool windowsize --sync "$WID" "$W" "$H"
    fi
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

# ae A B [CROP] → ImageMagick's AE metric. Under ImageMagick 7.1.2 it is not a
# count of differing pixels: on these RGB captures it is the sum, over pixels,
# of the mean absolute channel difference (0 to 1 each). It is 0 only for
# identical images, and a pixel that changes completely counts 1 (ledger L11).
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
# vs SCENE NAME CROP-KEY: the region is exactly as in SCENE's capture of this
# run (skipped when SCENE did not run, as with --only).
vs() {
    [[ -f $OUT/$1.png ]] || return 0
    check "$SCENE" "$2" "$(ae "$OUT/$1.png" "$OUT/$SCENE.png" "${AT[$3]}")" le 0
}
# differs SCENE NAME CROP-KEY: the region differs from SCENE's capture.
differs() {
    [[ -f $OUT/$1.png ]] || return 0
    check "$SCENE" "$2" "$(ae "$OUT/$1.png" "$OUT/$SCENE.png" "${AT[$3]}")" ge 1
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
    write_palette $'--gaze-bg: #161b26;\nthis line is not a palette entry\n'
    seed dark appearance true false 0
    launch && wait_title "$NOTES_TITLE" && shot 0.8
}
scene_18_appearance_low_contrast() {
    new_profile "$SCENE"
    write_palette $'--gaze-surface: #202735;\n--gaze-text: #2a3040;\n'
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
    # The pair shares one profile path, so the custom palette's path, which the
    # Appearance panel shows, is the same text in both captures. With the
    # scene's own name it differed by a letter whenever the cut kept one
    # (ledger L10).
    new_profile "$4"
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
# window_manager_version: openbox's first line of --version, or "none
# installed"; read without a pipe, which pipefail would fail when head
# closes it early.
window_manager_version() {
    local version
    command -v openbox >/dev/null || { printf 'none installed'; return 0; }
    version=$(openbox --version 2>/dev/null) || version="openbox (no version)"
    printf '%s' "${version%%$'\n'*}"
}
# later A B: true if the time A (seconds, with a fraction) is after B.
later() { gawk -v a="$1" -v b="$2" 'BEGIN { exit !(a > b) }'; }
# modified FILE: its modification time, in seconds with nanoseconds, or
# nothing if it is missing.
modified() { stat -c %.9Y -- "$1" 2>/dev/null || true; }
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

# ── Lamp scenes (docs/ui/ledger.md, L11) ─────────────────────────────────
# The new-tab page's f1r3lang program toggles the lamp's class `lit` on each
# click. The host theme overrode the page's `button.lit` rule, so the class
# changed and the lamp did not. These scenes click it in the layout of scene
# 58 (sidebar open), whose CROP_LAMP is calibrated. Each waits a second after
# the page loads, so its program is listening, and parks the pointer before
# every capture.
LAMP_YELLOW='#FFCF3F'
# lamp_scene THEME CLICKS: capture the unlit lamp as the reference `unlit`,
# then click it CLICKS times.
lamp_scene() {
    new_profile "$SCENE"
    seed "$1" appearance true false 4
    launch && wait_title "New tab" && sleep 1 && pointer NEUTRAL && ref unlit || return 1
    local n
    for ((n = 0; n < $2; n++)); do
        click LAMP 1 0.8 || return 1
    done
    pointer NEUTRAL && shot 0.8
}
# lamp_lit_checks: the click changed the lamp, it shows the page's yellow,
# and its label reads on it.
lamp_lit_checks() {
    effect lamp_lit_px unlit CROP_LAMP
    check "$SCENE" lit_fill_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_LAMP]}" "$LAMP_YELLOW")" ge 500
    check "$SCENE" lit_label_contrast "$(max_contrast "$OUT/$SCENE.png" "${AT[CROP_LAMP]}")" ge 4.5
}
scene_69_lamp_lit_dark() { lamp_scene dark 1 && lamp_lit_checks; }
scene_70_lamp_lit_light() { lamp_scene light 1 && lamp_lit_checks; }
# A second click puts the lamp out: no yellow is left, and the lamp looks
# exactly as it did before the first click. Blitz outlines only focused
# inputs and text areas, so the clicked button has no focus ring.
scene_71_lamp_out_dark() {
    lamp_scene dark 2 || return 1
    check "$SCENE" out_fill_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_LAMP]}" "$LAMP_YELLOW")" eq 0
    check "$SCENE" out_px_vs_unlit "$(ae "$WORK/refs/$SCENE.unlit.png" "$OUT/$SCENE.png" "${AT[CROP_LAMP]}")" le 0
}

# ── The system's scheme, theme files and the pages' scheme ───────────────
# Ledger S12 (part 3) and docs/ui/ledger.md, L13. Each scene compares the
# regions it shares with an earlier scene of the run: the theme System must
# look exactly like the built-in scheme it follows.

# system_scene ANSWER: theme System, Appearance open, the portal answering
# ANSWER.
system_scene() {
    new_profile "$SCENE"
    portal "$1"
    seed system appearance true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 0.5 && pointer NEUTRAL && shot 0.8
}
scene_72_system_dark() {
    system_scene 1 || return 1
    check "$SCENE" portal_argv_ok "$(same "$(head -n1 "$WORK/portal/argv.log" 2>/dev/null || true)" "$PORTAL_ARGV")" eq 1
    vs 15-appearance-dark strip_px_vs_15 CROP_STRIP
    differs 15-appearance-dark system_segment_vs_15 CROP_SEGMENT_SYSTEM
}
scene_73_system_light() {
    system_scene 2 || return 1
    vs 16-appearance-light strip_px_vs_16 CROP_STRIP
    differs 16-appearance-light system_segment_vs_16 CROP_SEGMENT_SYSTEM
}
scene_74_system_none() {
    system_scene 0 || return 1
    vs 15-appearance-dark strip_px_vs_15 CROP_STRIP
}
# No portal on the bus: the preference cannot be read, so dark is used and
# the sentence says so (not "states no preference", as in scene 74).
scene_75_system_fail() {
    system_scene fail || return 1
    vs 15-appearance-dark strip_px_vs_15 CROP_STRIP
    differs 74-system-none note_px_vs_74 CROP_SCHEME_NOTE
}
# The system turns light while the window is open. Gaining the focus asks
# the portal again (at most every 2 s, so the scene waits past start-up's
# query), and the answer is shown without a restart: the sidebar and the
# strip are then exactly scene 73's. The two share one profile path, so the
# themes folder the card shows is the same text in both (ledger L10).
scene_76_system_refocus() {
    new_profile 73-system-light
    portal 1
    seed system appearance true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 2.2 || return 1
    portal 2
    blur_and_refocus || { fail "the focus could not be moved (python-xlib)"; return 1; }
    sleep 1
    pointer NEUTRAL && shot 0.8 || return 1
    check "$SCENE" portal_queries "$(portal_queries)" ge 2
    vs 73-system-light sidebar_px_vs_73 CROP_SIDEBAR
    vs 73-system-light strip_px_vs_73 CROP_STRIP
}
# A portal that never answers: the window shows all the same, dark (start-up
# waits at most 100 ms for it), and the query is killed at its 1 s deadline.
scene_77_system_hang() {
    new_profile "$SCENE"
    portal hang
    seed system appearance true false 0
    local t0 ms
    t0=$(date +%s%N)
    launch && wait_title "$NOTES_TITLE" 3 || return 1
    ms=$((($(date +%s%N) - t0) / 1000000))
    sleep 1.3
    pointer NEUTRAL && shot 0.8 || return 1
    check "$SCENE" title_ms "$ms" le 3000
    vs 15-appearance-dark strip_px_vs_15 CROP_STRIP
    check "$SCENE" hung_children "$( (pgrep -P "$APP_PID" -x sleep || true) | wc -l)" eq 0
}
# theme_list_scene THEME: three theme files (three_themes), THEME chosen,
# Appearance open.
theme_list_scene() {
    three_themes
    seed "$1" appearance true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 0.5
}
# The list: the file that cannot be used shows the danger colour's icon.
scene_78_theme_list() {
    new_profile "$SCENE"
    theme_list_scene dark && pointer NEUTRAL && shot 0.8 || return 1
    # 81 such pixels in the first run (S12, part 3); the limit is half of that.
    check "$SCENE" broken_icon_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_THEME_ROWS]}" '#FF8A80')" ge 40
}
# Clicking a row chooses its theme: solar's background fills the strip.
scene_79_theme_file_switch() {
    new_profile "$SCENE"
    theme_list_scene dark && click THEME_ROW_3 1 1 && pointer NEUTRAL && shot 0.8 || return 1
    check "$SCENE" solar_bg_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_STRIP]}" '#FDF6E3')" ge 20000
}
# A switch to a theme file must look exactly like starting with it. The
# pair shares one profile path, so the themes folder the card shows is the
# same text in both (theme_fresh's rule, ledger L10).
scene_80_theme_file_fresh() {
    new_profile 79-theme-file-switch
    theme_list_scene solar && pointer NEUTRAL && shot 0.8 || return 1
    if [[ -f $OUT/79-theme-file-switch.png ]]; then
        check "$SCENE" stale_colour_px_vs_79 "$(hist_diff "$OUT/79-theme-file-switch.png" "$OUT/$SCENE.png")" le 0
    fi
}
# New theme writes the colours shown as my-theme.css and chooses it; the
# file is then edited, and Reload shows the edit. Three theme files make the
# panel longer than the sidebar, so scrolled to its end the card's buttons
# are where the coordinate table has them, before New theme and after.
scene_81_theme_new_reload() {
    new_profile "$SCENE"
    three_themes
    seed nord appearance true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 0.5 && scroll_panel_bottom && click THEME_NEW 1 1 || return 1
    local made="$PROFILE/config/themes/my-theme.css"
    check "$SCENE" new_theme_file "$( (grep -c -- '--gaze-accent: #88c0d0;' "$made" 2>/dev/null || true) | head -n1)" eq 1
    check "$SCENE" new_theme_chosen "$( (grep -cx 'theme = "my-theme"' "$PROFILE/config/settings.toml" || true) | head -n1)" eq 1
    write_theme my-theme $'--gaze-bg: #2e3440;\n--gaze-accent: #88c0d0;\n'
    scroll_panel_bottom && click THEME_RELOAD 1 1 && pointer NEUTRAL && shot 0.8 || return 1
    check "$SCENE" reloaded_bg_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_STRIP]}" '#2E3440')" ge 20000
}
# A chosen theme file that cannot be used: the system's scheme is shown
# (dark: the portal states no preference), and the panel says why first.
scene_82_theme_chosen_broken() {
    new_profile "$SCENE"
    write_theme broken $'--gaze-bg: #000000;\nbody { }\n'
    seed broken appearance true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 0.5 && pointer NEUTRAL && shot 0.8 || return 1
    vs 15-appearance-dark strip_px_vs_15 CROP_STRIP
    check "$SCENE" error_notice_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_PANEL_TOP]}" '#FF8A80')" ge 100
}
# L13: a page's prefers-color-scheme is the scheme the browser shows. The
# page is alone in its window, the sidebar closed.
scheme_page_scene() { # THEME
    new_profile "$SCENE"
    local tabs
    tabs=$(jq -cn --arg site "$SITE_URL" '[{url: ($site + "/scheme.html"), title: "Scheme page", parent: null}]')
    seed "$1" tabs false false 0 "$tabs"
    launch && wait_title "Scheme page" && sleep 0.5 && pointer NEUTRAL && shot 0.8
}
scene_83_page_scheme_dark() {
    scheme_page_scene dark || return 1
    check "$SCENE" dark_page_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_PAGE_LEFT]}" '#202124')" ge 300000
}
scene_84_page_scheme_light() {
    scheme_page_scene light || return 1
    check "$SCENE" dark_page_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_PAGE_LEFT]}" '#202124')" eq 0
    check "$SCENE" white_page_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_PAGE_LEFT]}" '#FFFFFF')" ge 300000
}

# ── The window kept in window.json, and the storage scenes ──────────────
# Ledger S13 (part 2). Scenes 85–93 read the window as the X server has it
# (xdotool, xprop) and window.json as F1R3Gaze wrote it. With no window
# manager, nothing reports a window that only moved, so F1R3Gaze reads the
# window when a save is due and before it closes; scenes 88 and 89 check
# both. Scenes 94–97 are storage scenes: an old profile found through HOME
# and moved, a move that meets a file already there, damaged state files,
# and a second F1R3Gaze on one profile. Scenes 98 and 99 run openbox.

# geometry: the window's client area, in root coordinates: "X Y W H".
geometry() {
    # shellcheck disable=SC2034 # set by xdotool's --shell output
    local WINDOW='' X='' Y='' WIDTH='' HEIGHT='' SCREEN=''
    eval "$(xdotool getwindowgeometry --shell "$WID")"
    printf '%s %s %s %s\n' "$X" "$Y" "$WIDTH" "$HEIGHT"
}
# frame_extents: the frame a window manager gives the window, "L R T B";
# "0 0 0 0" with none.
frame_extents() {
    local line
    line=$(xprop -id "$WID" _NET_FRAME_EXTENTS 2>/dev/null || true)
    if [[ $line =~ =\ *([0-9]+),\ *([0-9]+),\ *([0-9]+),\ *([0-9]+) ]]; then
        printf '%s %s %s %s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}" "${BASH_REMATCH[4]}"
    else
        printf '0 0 0 0\n'
    fi
}
# wm_state: the window's _NET_WM_STATE, as xprop prints it.
wm_state() { xprop -id "$WID" _NET_WM_STATE 2>/dev/null || true; }
# has_state ATOM: 1 if the window's _NET_WM_STATE holds ATOM, else 0.
has_state() { [[ $(wm_state) == *"$1"* ]] && echo 1 || echo 0; }
# xvfb_monitor: the X server's monitor as window.json keeps it.
xvfb_monitor() { jq -cn --arg name "$XVFB_OUTPUT" --argjson w "$W" --argjson h "$H" '{name: $name, uuid: null, origin: [0, 0], size: [$w, $h], scale: 1}'; }
# seed_window X Y W H [MAXIMIZED FULLSCREEN ZOOM MONITOR_JSON]: state/window.json
# as a window closed there would have left it (physical pixels, scale 1).
seed_window() {
    local monitor=${8:-}
    [[ -n $monitor ]] || monitor=$(xvfb_monitor)
    jq -n --argjson x "$1" --argjson y "$2" --argjson w "$3" --argjson h "$4" --argjson max "${5:-false}" \
        --argjson full "${6:-false}" --argjson zoom "${7:-1}" --argjson monitor "$monitor" \
        '{version: 1, normal: {width: $w, height: $h, position: [$x, $y], surface_offset: [0, 0], units: "physical", scale: 1},
          maximized: $max, fullscreen: $full, monitor: $monitor, zoom: $zoom}' >"$PROFILE/state/window.json"
}
# wjson FILTER: the scene's window.json through jq (nothing if unreadable).
# Numbers go through floor, as jq keeps a literal like 1280.0 as written.
wjson() { jq -r "$1" "$PROFILE/state/window.json" 2>/dev/null || true; }
# normal_place: "W H X Y" of window.json's normal size and place.
normal_place() { wjson '[(.normal.width|floor), (.normal.height|floor), (.normal.position[]|floor)]|map(tostring)|join(" ")'; }
# close_window: closes the window as a window manager would
# (WM_DELETE_WINDOW), then waits for the binary to end; EXIT_STATUS is its
# exit status.
close_window() {
    python3 - "$WID" <<'PY' || { fail "WM_DELETE_WINDOW could not be sent"; return 1; }
import sys
from Xlib import X, display, protocol
d = display.Display()
window = d.create_resource_object("window", int(sys.argv[1]))
message = protocol.event.ClientMessage(
    window=window,
    client_type=d.intern_atom("WM_PROTOCOLS"),
    data=(32, [d.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]),
)
window.send_event(message, event_mask=X.NoEventMask)
d.flush()
PY
    wait_exit 5
}
# wait_exit SECS: waits for the binary to end by itself.
wait_exit() {
    local deadline=$((SECONDS + $1))
    while kill -0 "$APP_PID" 2>/dev/null; do
        ((SECONDS < deadline)) || { fail "the window did not close within $1 s"; return 1; }
        sleep 0.1
    done
    EXIT_STATUS=0
    wait "$APP_PID" 2>/dev/null || EXIT_STATUS=$?
    APP_PID=""
    if [[ -n $WD_PID ]]; then
        pkill -TERM -P "$WD_PID" 2>/dev/null || true
        kill "$WD_PID" 2>/dev/null || true
        wait "$WD_PID" 2>/dev/null || true
        WD_PID=""
    fi
}
# new_machine NAME: the scene's own HOME and XDG folders under the work
# directory, for a start with no --profile (launch_env), which looks for an
# old profile through HOME too. S is their root.
S=""
MACHINE_ENV=()
new_machine() {
    S="$WORK/profiles/$1"
    guard_profile "$S"
    rm -rf -- "$S"
    mkdir -p -- "$S/home" "$S/xdg/config" "$S/xdg/data" "$S/xdg/state" "$S/xdg/cache" "$S/xdg/config-dirs" \
        "$S/xdg/data-dirs" "$S/xdg/runtime"
    chmod 700 "$S/xdg/runtime"
    portal 0
    : >"$WORK/portal/argv.log"
    MACHINE_ENV=(env -u F1R3GAZE_PROFILE HOME="$S/home" XDG_CONFIG_HOME="$S/xdg/config" XDG_DATA_HOME="$S/xdg/data"
        XDG_STATE_HOME="$S/xdg/state" XDG_CACHE_HOME="$S/xdg/cache" XDG_RUNTIME_DIR="$S/xdg/runtime"
        XDG_CONFIG_DIRS="$S/xdg/config-dirs" XDG_DATA_DIRS="$S/xdg/data-dirs")
}
# machine_self_test: before such a start, `paths` names only the scene's
# folders, the old profile's places included; exit 3 otherwise.
machine_self_test() {
    local printed
    printed=$("${MACHINE_ENV[@]}" "$BIN" paths) || { fail "paths failed in $S"; return 1; }
    printf '%s\n' "$printed" >"$WORK/logs/$SCENE.paths"
    storage_paths_inside "$printed" "$S"
}
# The ids an old profile and a new one hold in the storage scenes.
LEGACY_ID=0123456789abcdef0123456789abcdef
NEW_ID=fedcba9876543210fedcba9876543210
# seed_legacy DIR THEME PANEL OPEN TREE ACTIVE: the single folder of a build
# from before the five-root layout: its settings, a user id, and its
# workspace with the standard tabs and visits.
seed_legacy() {
    local dir=$1
    mkdir -p -- "$dir"
    printf 'observers =\nrestore_sidebar = true\n' >"$dir/settings.conf"
    printf '%s' "$LEGACY_ID" >"$dir/user-id"
    jq -n --arg theme "$2" --arg panel "$3" --argjson open "$4" --argjson tree "$5" --argjson active "$6" \
        --argjson tabs "$(std_tabs)" --argjson visits "$(std_visits)" \
        '{theme: $theme, sidebar_open: $open, panel: $panel, tree_tabs: $tree, active: $active, tabs: $tabs, visits: $visits}' \
        >"$dir/workspace.json"
}
# originals_kept CONFIG STATE: how many of the old settings.conf and
# workspace.json the backups keep byte for byte.
originals_kept() {
    local n=0 f
    for f in "$1"/backups/*/legacy-profile/settings.conf; do
        cmp -s -- "$f" "$WORK/refs/$SCENE.settings.conf" && n=$((n + 1))
    done
    for f in "$2"/backups/*/legacy-profile/workspace.json; do
        cmp -s -- "$f" "$WORK/refs/$SCENE.workspace.json" && n=$((n + 1))
    done
    printf '%s\n' "$n"
}
# conflict_listed NOTE: 1 if MIGRATED.txt lists user-id under "Not moved".
conflict_listed() {
    gawk '/^Not moved/ { section = 1; next } /^[^ ]/ { section = 0 } section && /user-id/ { found = 1 } END { print found + 0 }' "$1" 2>/dev/null ||
        echo 0
}
# backup_kept NAME: how many backups of state/NAME hold the scene's seed.
backup_kept() {
    local n=0 f
    for f in "$PROFILE"/state/backups/*/"$1"; do
        cmp -s -- "$f" "$WORK/refs/$SCENE.$1" && n=$((n + 1))
    done
    printf '%s\n' "$n"
}
# start_wm: openbox on the X server, with a configuration of the work
# directory's (no key bindings, no mouse bindings); stop_wm ends it.
start_wm() {
    mkdir -p -- "$WORK/openbox"
    cat >"$WORK/openbox/rc.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <theme><name>Clearlooks</name><animateIconify>no</animateIconify></theme>
  <placement><policy>Smart</policy></placement>
  <desktops><number>1</number></desktops>
  <keyboard/>
  <mouse/>
</openbox_config>
XML
    XDG_DATA_DIRS=/usr/local/share:/usr/share XDG_CONFIG_DIRS=/etc/xdg \
        openbox --sm-disable --config-file "$WORK/openbox/rc.xml" >"$WORK/logs/$SCENE.openbox.log" 2>&1 &
    WM_PID=$!
    local deadline=$((SECONDS + 5))
    until xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q 'window id'; do
        ((SECONDS < deadline)) || { fail "openbox did not start (see $WORK/logs/$SCENE.openbox.log)"; return 1; }
        sleep 0.1
    done
}
stop_wm() {
    [[ -n $WM_PID ]] || return 0
    kill "$WM_PID" 2>/dev/null || true
    wait "$WM_PID" 2>/dev/null || true
    WM_PID=""
    # Its check window goes with it; the root's pointer to it is removed so
    # that no later scene sees a window manager.
    xprop -root -remove _NET_SUPPORTING_WM_CHECK 2>/dev/null || true
}

# The first start: the default size at the screen's corner, and the first
# save, though no event reports the window (it is read 500 ms after it is
# made, the loop woken for it by ControlFlow::WaitUntil). It must look
# exactly like scene 19, made at 800×600 and resized. Without that wake-up
# the first thing to wake the loop is the history's save, 2 s after the
# page's visit, and window.json is written just after history.json, in time
# for first_save all the same (storage ledger S13 part 2, E7): so the
# window's state must be written before history.json. (Start-up writes a
# default window.json before either, when the file is missing; only the
# window's own state counts.)
scene_85_window_first_start() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    local launched history_at="" window_at saved deadline
    launched=$(date +%s.%N)
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1.2 && shot 0.8 || return 1
    check "$SCENE" default_size_and_place "$(same "$(geometry)" "0 0 $W $H")" eq 1
    saved=$(same "$(normal_place) $(wjson '.monitor.name')" "$W $H 0 0 $XVFB_OUTPUT")
    check "$SCENE" first_save "$saved" eq 1
    # The history's own save in this start; the seeded file is older.
    deadline=$((SECONDS + 5))
    until history_at=$(modified "$PROFILE/state/history.json") && [[ -n $history_at ]] && later "$history_at" "$launched"; do
        ((SECONDS < deadline)) || { history_at=""; break; }
        sleep 0.1
    done
    window_at=$(modified "$PROFILE/state/window.json")
    check "$SCENE" first_save_before_history "$( ((saved)) && [[ -n $history_at && -n $window_at ]] && later "$history_at" "$window_at" && echo 1 || echo 0)" eq 1
    vs 19-collapsed-dark first_start_px_vs_19 CROP_ALL
}
# Made where it was left, and a window opened where it was writes nothing.
scene_86_window_restored() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 120 80 900 600
    cp -- "$PROFILE/state/window.json" "$WORK/refs/$SCENE.window.json"
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1.2 && shot || return 1
    check "$SCENE" restored_size_and_place "$(same "$(geometry)" "120 80 900 600")" eq 1
    check "$SCENE" restored_writes_nothing "$(cmp -s -- "$WORK/refs/$SCENE.window.json" "$PROFILE/state/window.json" && echo 1 || echo 0)" eq 1
}
# Left on a monitor that is gone, off this one: centred on the primary.
scene_87_window_off_screen() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 1500 100 900 600 false false 1 '{"name":"DP-9","uuid":null,"origin":[1280,0],"size":[1920,1080],"scale":1}'
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1.2 && shot || return 1
    check "$SCENE" centred_on_primary "$(same "$(geometry)" "190 100 900 600")" eq 1
    check "$SCENE" saved_where_shown "$(same "$(normal_place) $(wjson '.monitor.name')" "900 600 190 100 $XVFB_OUTPUT")" eq 1
}
# A drag longer than 3 s is saved during it (SaveClock::LONGEST, woken by
# ControlFlow::WaitUntil), and the last size once it ends.
scene_88_window_saved_while_open() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 120 80 900 600
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1 || return 1
    local i during=""
    for ((i = 0; i < 40; i++)); do
        xdotool windowsize "$WID" $((900 + 5 * i)) 600
        sleep 0.1
        if ((i == 33)); then
            during=$(wjson '.normal.width|floor')
        fi
    done
    sleep 1
    shot || return 1
    check "$SCENE" saved_during_drag "$( ((${during:-0} > 900)) && echo 1 || echo 0)" eq 1
    check "$SCENE" saved_after_drag "$(wjson '.normal.width|floor')" eq 1095
}
# Moved and resized, then closed within 500 ms: what is saved is read
# before the window goes (no event reports the move without a window
# manager).
scene_89_window_move_resize_close() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 120 80 900 600
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1 && shot || return 1
    xdotool windowmove "$WID" 200 150
    xdotool windowsize --sync "$WID" 1000 640
    sleep 0.2
    close_window || return 1
    check "$SCENE" closed_cleanly "$EXIT_STATUS" eq 0
    check "$SCENE" saved_at_close "$(same "$(normal_place)" "1000 640 200 150")" eq 1
    check "$SCENE" session_kept "$(jq '.tabs|length' "$PROFILE/state/session.json" 2>/dev/null || echo 0)" eq 6
}
# Blitz's zoom keys: the zoom is kept.
scene_90_window_zoom() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && key ctrl+equal && key ctrl+equal && shot 0.8 || return 1
    close_window || return 1
    check "$SCENE" zoom_saved "$(wjson '.zoom')" eq 1.2
    differs 19-collapsed-dark zoomed_px_vs_19 CROP_ALL
}
# The zoom kept is restored. Blitz lays a window out at a zoom a little
# differently depending on the zoom it started at (ledger S13 part 2, E3), so
# the restored window is compared with itself after Ctrl+0 and Ctrl+= twice:
# they match only if the zoom restored is exactly the keys' 1.2.
# Was: compared with scene 90's window, zoomed from 100 %.
# vs 90-window-zoom zoom_restored_px_vs_90 CROP_ALL
scene_91_window_zoom_restored() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 0 0 "$W" "$H" false false 1.2
    launch && wait_title "$NOTES_TITLE" && shot 0.8 || return 1
    differs 19-collapsed-dark zoomed_px_vs_19 CROP_ALL
    key ctrl+0 && sleep 0.5 && key ctrl+equal && key ctrl+equal && ref keys 0.8 || return 1
    check "$SCENE" zoom_restored_px_vs_keys "$(ae "$OUT/$SCENE.png" "$WORK/refs/$SCENE.keys.png" "${AT[CROP_ALL]}")" le 0
}
# Chrome ledger L15: Ctrl+− pressed past the floor keeps the zoom at 25 %,
# and a window restored at it looks the same.
scene_92_window_zoom_floor() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" || return 1
    local i
    for ((i = 0; i < 12; i++)); do
        key ctrl+minus
    done
    sleep 0.5
    check "$SCENE" alive_after_zoom_out "$(kill -0 "$APP_PID" 2>/dev/null && echo 1 || echo 0)" eq 1
    shot || return 1
    close_window || return 1
    check "$SCENE" zoom_floor_saved "$(wjson '.zoom')" eq 0.25
    # Restored at 25 %, then compared with itself after Ctrl+0 and the same
    # twelve presses (as in scene 91; ledger S13 part 2, E3).
    # Was: compared with the window zoomed out from 100 % above.
    # check "$SCENE" zoom_floor_px_vs_restored "$(ae "$WORK/refs/$SCENE.restored.png" "$OUT/$SCENE.png" "${AT[CROP_ALL]}")" le 0
    launch && wait_title "$NOTES_TITLE" && ref restored 0.8 || return 1
    key ctrl+0 && sleep 0.5 || return 1
    for ((i = 0; i < 12; i++)); do
        key ctrl+minus
    done
    ref keys 0.8 || return 1
    check "$SCENE" zoom_floor_px_vs_keys "$(ae "$WORK/refs/$SCENE.restored.png" "$WORK/refs/$SCENE.keys.png" "${AT[CROP_ALL]}")" le 0
}
# F11 into window.json and back, with the hint on how to leave.
scene_93_fullscreen_f11() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" && pointer NEUTRAL && ref before || return 1
    key F11
    sleep 1
    shot 0.3 || return 1
    check "$SCENE" fullscreen_saved "$(same "$(wjson '.fullscreen')" true)" eq 1
    effect fullscreen_hint_px before CROP_STATUS
    key F11
    sleep 1
    check "$SCENE" fullscreen_left_saved "$(same "$(wjson '.fullscreen') $(normal_place)" "false $W $H 0 0")" eq 1
}
# An old profile found through HOME (the scene's), moved by a start with no
# --profile: the note, the id, the originals in the backups, the converted
# settings and state; and the window is scene 02's, with the notice.
scene_94_legacy_migration() {
    new_machine "$SCENE"
    local legacy="$S/home/.local/share/f1r3gaze" config="$S/xdg/config/f1r3fly-io/f1r3gaze"
    local data="$S/xdg/data/f1r3fly-io/f1r3gaze" state="$S/xdg/state/f1r3fly-io/f1r3gaze"
    seed_legacy "$legacy" light tabs true false 0
    cp -- "$legacy/settings.conf" "$WORK/refs/$SCENE.settings.conf"
    cp -- "$legacy/workspace.json" "$WORK/refs/$SCENE.workspace.json"
    machine_self_test || return 1
    local started
    started=$(date +%s)
    launch_env && wait_title "$NOTES_TITLE" && sleep 0.5 && shot 0.8 || return 1
    check "$SCENE" migrated_note "$([[ -f $legacy/MIGRATED.txt ]] && echo 1 || echo 0)" eq 1
    check "$SCENE" user_id_moved "$(same "$(cat -- "$data/user-id" 2>/dev/null)" "$LEGACY_ID")" eq 1
    check "$SCENE" originals_kept "$(originals_kept "$config" "$state")" eq 2
    check "$SCENE" old_files_left "$(find "$legacy" -type f ! -name MIGRATED.txt | wc -l)" eq 0
    check "$SCENE" settings_converted "$(grep -cxE 'restore_sidebar = true|theme = "default-light"' "$config/settings.toml" || true)" eq 2
    check "$SCENE" session_tabs "$(jq '.tabs|length' "$state/session.json" 2>/dev/null || echo 0)" eq 6
    # The visits from before the start: the window adds one when its page
    # loads, saved within 2 s or at close.
    check "$SCENE" history_visits "$(jq --argjson t "$started" '[.visits[]|select(.at < $t)]|length' "$state/history.json" 2>/dev/null || echo 0)" eq 8
    vs 02-tabs-light strip_px_vs_02 CROP_STRIP
    vs 02-tabs-light sidebar_px_vs_02 CROP_SIDEBAR
    differs 02-tabs-light notice_px_vs_02 CROP_STATUS
}
# A file already where the old profile's goes: both are kept, the note
# lists it, the move is finished (the marker), and the window warns.
scene_95_migration_conflict() {
    new_machine "$SCENE"
    local legacy="$S/home/.local/share/f1r3gaze" data="$S/xdg/data/f1r3fly-io/f1r3gaze"
    seed_legacy "$legacy" dark tabs true false 0
    mkdir -p -- "$data"
    printf '%s' "$NEW_ID" >"$data/user-id"
    machine_self_test || return 1
    launch_env && wait_title "$NOTES_TITLE" && sleep 0.5 && shot 0.8 || return 1
    local kept=0
    [[ $(cat -- "$data/user-id" 2>/dev/null) == "$NEW_ID" ]] && kept=$((kept + 1))
    [[ $(cat -- "$legacy/user-id" 2>/dev/null) == "$LEGACY_ID" ]] && kept=$((kept + 1))
    check "$SCENE" both_ids_kept "$kept" eq 2
    check "$SCENE" conflict_listed "$(conflict_listed "$legacy/MIGRATED.txt")" eq 1
    check "$SCENE" marker_written "$(jq -e '.migrated_from' "$data/layout.json" >/dev/null 2>&1 && echo 1 || echo 0)" eq 1
    # 43 such pixels in the first run (S13, part 2); the limit is half of that.
    check "$SCENE" warning_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_STATUS]}" '#EFBF75')" ge 21
}
# Damaged state files: each is backed up before start-up makes it anew, the
# window opens at its default, and the session starts over.
scene_96_corrupt_state() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    printf '{"version": 1, "tabs": [' >"$PROFILE/state/session.json"
    printf '{"normal": {"width": "wide"}}' >"$PROFILE/state/window.json"
    cp -- "$PROFILE/state/session.json" "$WORK/refs/$SCENE.session.json"
    cp -- "$PROFILE/state/window.json" "$WORK/refs/$SCENE.window.json"
    launch_restored && wait_title "New tab" && sleep 1.2 && shot || return 1
    check "$SCENE" session_backup_kept "$(backup_kept session.json)" eq 1
    check "$SCENE" window_backup_kept "$(backup_kept window.json)" eq 1
    check "$SCENE" default_window "$(same "$(geometry)" "0 0 $W $H")" eq 1
    # 43 such pixels in the first run (S13, part 2); the limit is half of that.
    check "$SCENE" warning_px "$(count_color_in "$OUT/$SCENE.png" "${AT[CROP_STATUS]}" '#EFBF75')" ge 21
    close_window || return 1
    check "$SCENE" session_restarted "$(same "$(jq -c '[(.tabs|length), .tabs[0].url]' "$PROFILE/state/session.json" 2>/dev/null)" '[1,"gaze://newtab"]')" eq 1
}
# A second F1R3Gaze on the profile: a window and a headless run are refused,
# naming the holder; a command that only reads goes on; the first window is
# untouched.
scene_97_second_instance() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    launch && wait_title "$NOTES_TITLE" || return 1
    local status=0 logs="$WORK/logs/$SCENE"
    timeout 15 "$BIN" --profile "$PROFILE" >"$logs.second.out" 2>"$logs.second.err" || status=$?
    check "$SCENE" second_window_refused "$status" eq 1
    check "$SCENE" holder_named "$(grep -cF "another F1R3Gaze is using this profile: process $APP_PID (window" "$logs.second.err" || true)" ge 1
    status=0
    timeout 15 "$BIN" --profile "$PROFILE" --headless gaze://newtab >"$logs.headless.out" 2>&1 || status=$?
    check "$SCENE" headless_refused "$status" eq 1
    status=0
    timeout 15 "$BIN" --profile "$PROFILE" wallet list >"$logs.reader.out" 2>&1 || status=$?
    check "$SCENE" reader_goes_on "$( ((status == 0)) && grep -qF 'this session only reads' "$logs.reader.out" && echo 1 || echo 0)" eq 1
    shot || return 1
    vs 19-collapsed-dark first_unchanged_px_vs_19 CROP_ALL
}
# With a window manager (openbox): placed by its frame's corner, full screen
# and back, restored full screen, and no creep by the frame.
scene_98_fullscreen_wm() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 120 80 900 600
    start_wm || return 1
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1 || return 1
    local l r t b framed
    read -r l r t b <<<"$(frame_extents)"
    framed="$((120 + l)) $((80 + t)) 900 600"
    check "$SCENE" framed_place "$(same "$(geometry)" "$framed")" eq 1
    key F11
    sleep 1
    shot 0.5 || return 1
    check "$SCENE" fullscreen_state "$(has_state _NET_WM_STATE_FULLSCREEN)" eq 1
    check "$SCENE" fullscreen_geometry "$(same "$(geometry)" "0 0 $W $H")" eq 1
    vs 19-collapsed-dark fullscreen_px_vs_19 CROP_ABOVE_STATUS
    key F11
    sleep 1
    check "$SCENE" left_fullscreen_place "$(same "$(geometry)" "$framed")" eq 1
    close_window || return 1
    check "$SCENE" no_creep "$(same "$(normal_place) $(wjson '.fullscreen')" "900 600 120 80 false")" eq 1
    jq '.fullscreen = true' "$PROFILE/state/window.json" >"$PROFILE/state/window.json.next" &&
        mv -f -- "$PROFILE/state/window.json.next" "$PROFILE/state/window.json"
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1 || return 1
    check "$SCENE" restored_fullscreen_state "$(has_state _NET_WM_STATE_FULLSCREEN)" eq 1
    check "$SCENE" restored_fullscreen_geometry "$(same "$(geometry)" "0 0 $W $H")" eq 1
    key F11
    sleep 1
    check "$SCENE" restored_then_left_place "$(same "$(geometry)" "$framed")" eq 1
}
# With a window manager: maximized, kept so with the size before it, made
# maximized again (after the window is shown), and back.
scene_99_maximize_wm() {
    new_profile "$SCENE"
    seed dark tabs false false 0
    seed_window 120 80 900 600
    start_wm || return 1
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1 || return 1
    local l r t b framed
    read -r l r t b <<<"$(frame_extents)"
    framed="$((120 + l)) $((80 + t)) 900 600"
    wmctrl -i -r "$WID" -b add,maximized_vert,maximized_horz
    sleep 1.2
    check "$SCENE" maximized_state "$(has_state _NET_WM_STATE_MAXIMIZED_HORZ)" eq 1
    read -r l r t b <<<"$(frame_extents)"
    check "$SCENE" maximized_geometry "$(same "$(geometry)" "$l $t $((W - l - r)) $((H - t - b))")" eq 1
    close_window || return 1
    check "$SCENE" normal_kept_while_maximized "$(same "$(wjson '.maximized') $(normal_place)" "true 900 600 120 80")" eq 1
    launch_restored && wait_title "$NOTES_TITLE" && sleep 1 && shot 0.5 || return 1
    check "$SCENE" restored_maximized_state "$(has_state _NET_WM_STATE_MAXIMIZED_HORZ)" eq 1
    read -r l r t b <<<"$(frame_extents)"
    check "$SCENE" restored_maximized_geometry "$(same "$(geometry)" "$l $t $((W - l - r)) $((H - t - b))")" eq 1
    wmctrl -i -r "$WID" -b remove,maximized_vert,maximized_horz
    sleep 1
    check "$SCENE" unmaximized_place "$(same "$(geometry)" "$framed")" eq 1
}

# ── Calibration ──────────────────────────────────────────────────────────
# calibrate_draw IMAGE: a capture now, with every point of the coordinate
# table drawn on it as a cross, and every crop as a box.
calibrate_draw() {
    local img=$1 points=() boxes=() name value x y w h
    import -window root "$img"
    for name in "${!AT[@]}"; do
        value=${AT[$name]}
        if [[ $value =~ ^([0-9]+)\ ([0-9]+)$ ]]; then
            x=${BASH_REMATCH[1]}
            y=${BASH_REMATCH[2]}
            points+=(-draw "line $((x - 8)),$y $((x + 8)),$y" -draw "line $x,$((y - 8)) $x,$((y + 8))"
                -draw "text $((x + 6)),$((y - 6)) '$name'")
        elif [[ $value =~ ^([0-9]+)x([0-9]+)\+([0-9]+)\+([0-9]+)$ ]]; then
            w=${BASH_REMATCH[1]}
            h=${BASH_REMATCH[2]}
            x=${BASH_REMATCH[3]}
            y=${BASH_REMATCH[4]}
            boxes+=(-draw "rectangle $x,$y $((x + w - 1)),$((y + h - 1))")
        fi
    done
    magick "$img" -fill none -stroke '#00c8ff' "${boxes[@]}" -stroke '#ff2d55' -fill '#ff2d55' -pointsize 10 "${points[@]}" "$img"
    log "calibration image: $img"
}
# Was one capture of the Tabs panel, with the points only. In after mode the
# Appearance panel is drawn too, with three theme files, at its top and
# scrolled to its end (S12, part 3).
calibrate() {
    SCENE=calibrate
    new_profile calibrate
    seed dark tabs true false 0
    launch && wait_title "$NOTES_TITLE" && sleep 1 || die 1 "calibration scene failed"
    calibrate_draw "$WORK/calibrate-$MODE.png"
    stop_app
    [[ $MODE == after ]] || return 0
    new_profile calibrate
    theme_list_scene dark && pointer NEUTRAL && sleep 0.5 || die 1 "calibration scene failed"
    calibrate_draw "$WORK/calibrate-after-appearance.png"
    scroll_panel_bottom && pointer NEUTRAL && sleep 0.5
    calibrate_draw "$WORK/calibrate-after-appearance-bottom.png"
    stop_app
}

# ── Run ──────────────────────────────────────────────────────────────────
# Before anything starts: `f1r3gaze paths` names only the work directory,
# with --profile and without (a binary from before the five-root layout has
# no `paths`, and runs only with --profile).
if [[ $SEED_FORMAT == current ]]; then
    storage_paths_self_test "$BIN" "$WORK" "$WORK/profiles/_self-test"
    log "isolation: every folder in $WORK; dbus-send is the fake; VK_DRIVER_FILES=${VK_DRIVER_FILES:-none}"
fi
if ((CHECK_ISOLATION)); then
    exit 0
fi
start_xvfb
# The X server's monitor, as RandR names it: the name winit gives the
# monitor, which window.json keeps (S13, part 2).
XVFB_OUTPUT=$(xrandr --query 2>/dev/null | gawk '$2 == "connected" { print $1; exit }' || true)
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
    printf 'seed_format\t%s\n' "$SEED_FORMAT"
    printf 'xvfb_output\t%s\n' "${XVFB_OUTPUT:-none}"
    # Was `openbox --version | head -n1 || printf 'none installed'`: under
    # pipefail, head closing the pipe fails the pipeline, so both were
    # printed (run.txt of 2026-10-06, corrected by hand).
    # printf 'window_manager\t%s\n' "$(command -v openbox >/dev/null && openbox --version 2>/dev/null | head -n1 || printf 'none installed')"
    printf 'window_manager\t%s\n' "$(window_manager_version)"
    printf 'isolation\txdg+home+no-bus+fake-portal; VK_DRIVER_FILES=%s\n' "${VK_DRIVER_FILES:-none}"
} >"$OUT/run.txt"

FAILED=()
RAN=0
for name in "${SCENES[@]}"; do
    [[ -z $ONLY || $name =~ $ONLY ]] || continue
    if [[ -n ${ONLY_IN[$name]:-} && ${ONLY_IN[$name]} != "$MODE" ]]; then
        log "$name: not reachable in $MODE mode (only ${ONLY_IN[$name]}); skipped by design"
        continue
    fi
    if [[ -n ${FORMAT_IN[$name]:-} && ${FORMAT_IN[$name]} != "$SEED_FORMAT" ]]; then
        log "$name: needs the ${FORMAT_IN[$name]} seed format; skipped by design"
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
    stop_wm
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
