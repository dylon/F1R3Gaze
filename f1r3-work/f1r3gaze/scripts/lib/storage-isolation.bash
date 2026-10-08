# storage-isolation.bash — keep F1R3Gaze's test runs away from the user's
# own folders (docs/storage/README.md, "Verifying storage"; ledger S14).
#
# Sourced by scripts/ui-snapshots.sh, scripts/resize-bench.sh,
# scripts/resize-live.sh and scripts/storage-smoke.sh.
#
#   storage_real_roots
#       Sets REAL_ROOTS: every folder a real F1R3Gaze could use on this
#       machine, from the real environment. Call it before anything is
#       overridden.
#   storage_guard PATH
#       Exits 3 if PATH is, holds, or is inside one of REAL_ROOTS.
#   storage_isolate WORK
#       Points everything F1R3Gaze reads at WORK: every XDG variable, no
#       session bus, a fake dbus-send for the colour-scheme portal. What the
#       XDG folders also find for other programs is named directly: the
#       Vulkan driver (VK_DRIVER_FILES) and the X cursor theme
#       (XCURSOR_PATH).
#   storage_paths_self_test BIN WORK PROFILE
#       Exits 3 unless `BIN paths`, with --profile PROFILE and without it
#       (HOME=WORK/home), names only folders inside WORK, and dbus-send is
#       the fake.
#   storage_paths_inside PRINTED DIR
#       Exits 3 unless every folder of `paths` output PRINTED (the legacy
#       line included) is inside DIR and near no real F1R3Gaze folder.
#
# Why each: an old single-folder profile is found through HOME as well as
# $XDG_DATA_HOME, and a start without --profile moves it, so isolating the
# XDG variables alone would move the user's profile (design report,
# finding 2). The Vulkan loader finds its drivers through $XDG_DATA_DIRS
# (finding 3).

# The absolute, normalized form of PATH, which need not exist.
storage_abspath() {
    if realpath -m -- / >/dev/null 2>&1; then
        realpath -m -- "$1"
    else
        python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$1"
    fi
}

storage_real_roots() {
    REAL_ROOTS=()
    local home=${HOME:?HOME is not set}
    # Only an absolute XDG value counts (XDG Base Directory specification).
    local config=$home/.config data=$home/.local/share state=$home/.local/state cache=$home/.cache
    [[ ${XDG_CONFIG_HOME:-} == /* ]] && config=$XDG_CONFIG_HOME
    [[ ${XDG_DATA_HOME:-} == /* ]] && data=$XDG_DATA_HOME
    [[ ${XDG_STATE_HOME:-} == /* ]] && state=$XDG_STATE_HOME
    [[ ${XDG_CACHE_HOME:-} == /* ]] && cache=$XDG_CACHE_HOME
    local ours=f1r3fly-io/f1r3gaze
    REAL_ROOTS+=("$config/$ours" "$data/$ours" "$state/$ours" "$cache/$ours")
    if [[ ${XDG_RUNTIME_DIR:-} == /* ]]; then
        REAL_ROOTS+=("$XDG_RUNTIME_DIR/$ours")
    else
        REAL_ROOTS+=("$state/$ours/runtime")
    fi
    # The single-folder profile of older builds, wherever it could be.
    REAL_ROOTS+=("$data/f1r3gaze" "$home/.local/share/f1r3gaze")
    # macOS: the bundle's folders, and the old profile.
    REAL_ROOTS+=(
        "$home/Library/Application Support/io.f1r3fly.f1r3gaze"
        "$home/Library/Caches/io.f1r3fly.f1r3gaze"
        "$home/Library/Application Support/F1R3Gaze"
    )
    if [[ -n ${F1R3GAZE_PROFILE:-} ]]; then
        REAL_ROOTS+=("$F1R3GAZE_PROFILE")
    fi
    local i
    for i in "${!REAL_ROOTS[@]}"; do
        REAL_ROOTS[i]=$(storage_abspath "${REAL_ROOTS[i]}")
    done
}

storage_guard() {
    local path root
    path=$(storage_abspath "$1")
    for root in "${REAL_ROOTS[@]}"; do
        if [[ $path == "$root" || $path == "$root"/* || $root == "$path"/* ]]; then
            printf 'refusing %s: it is, holds or is inside the real F1R3Gaze folder %s\n' "$path" "$root" >&2
            exit 3
        fi
    done
}

storage_isolate() {
    local work=$1 d f
    # 1. Vulkan. Its loader searches $XDG_CONFIG_DIRS, /etc and
    #    $XDG_DATA_DIRS for vulkan/icd.d: name the drivers found there now.
    if [[ -z ${VK_DRIVER_FILES:-} && -z ${VK_ICD_FILENAMES:-} ]]; then
        local dirs=() files=()
        IFS=: read -r -a dirs <<<"${XDG_CONFIG_HOME:-$HOME/.config}:${XDG_CONFIG_DIRS:-/etc/xdg}:/etc:${XDG_DATA_HOME:-$HOME/.local/share}:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
        for d in "${dirs[@]}"; do
            for f in "$d"/vulkan/icd.d/*.json; do
                [[ -f $f ]] && files+=("$f")
            done
        done
        if ((${#files[@]})); then
            VK_DRIVER_FILES=$(IFS=:; printf '%s' "${files[*]}")
            export VK_DRIVER_FILES VK_ICD_FILENAMES=$VK_DRIVER_FILES
        fi
    fi
    # 1b. The X cursor theme. libXcursor's search path (the `xcursor` crate
    #    under winit's X11 backend) runs through $XDG_DATA_HOME and each
    #    $XDG_DATA_DIRS/icons, which are about to point into WORK: every
    #    cursor would fall back to one shape (the cursor scenes' check
    #    `cursor_refs_distinct`; ledger S14, H0). Name the path now, in the
    #    crate's order, with WORK's data folder first, as the harness had it
    #    when only $XDG_DATA_HOME was isolated.
    if [[ -z ${XCURSOR_PATH:-} ]]; then
        local data_dirs=() cursor_paths=("$work/xdg/data" "$HOME/.icons")
        IFS=: read -r -a data_dirs <<<"${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
        for d in "${data_dirs[@]}"; do
            [[ -n $d ]] && cursor_paths+=("$d/icons")
        done
        cursor_paths+=(/usr/share/pixmaps "$HOME/.cursors" /usr/share/cursors/xorg-x11)
        XCURSOR_PATH=$(IFS=:; printf '%s' "${cursor_paths[*]}")
        export XCURSOR_PATH
    fi
    # 2. A relative WAYLAND_DISPLAY names a socket in $XDG_RUNTIME_DIR,
    #    which is about to move.
    if [[ -n ${WAYLAND_DISPLAY:-} && $WAYLAND_DISPLAY != /* && ${XDG_RUNTIME_DIR:-} == /* ]]; then
        export WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY
    fi
    # 3. Every XDG variable F1R3Gaze reads.
    local x=$work/xdg
    mkdir -p -- "$x/config" "$x/data" "$x/state" "$x/cache" "$x/config-dirs" "$x/data-dirs" "$x/runtime" "$work/home"
    chmod 700 "$x/runtime"
    export XDG_CONFIG_HOME=$x/config XDG_DATA_HOME=$x/data XDG_STATE_HOME=$x/state XDG_CACHE_HOME=$x/cache
    export XDG_RUNTIME_DIR=$x/runtime XDG_CONFIG_DIRS=$x/config-dirs XDG_DATA_DIRS=$x/data-dirs
    # 4. No session bus: the colour-scheme portal is the fake below.
    unset DBUS_SESSION_BUS_ADDRESS
    # 5. The fake dbus-send. It logs its arguments and answers from
    #    $F1R3GAZE_FAKE_PORTAL/scheme: 0, 1 or 2 (no preference, dark,
    #    light), `hang` (killed at the 1 s deadline) or `fail`.
    mkdir -p -- "$work/fake-bin" "$work/portal"
    export F1R3GAZE_FAKE_PORTAL=$work/portal
    [[ -f $work/portal/scheme ]] || printf '0\n' >"$work/portal/scheme"
    cat >"$work/fake-bin/dbus-send" <<'SH'
#!/usr/bin/env bash
# A stand-in for the XDG desktop portal's colour scheme
# (scripts/lib/storage-isolation.bash).
printf '%s\n' "$*" >>"${F1R3GAZE_FAKE_PORTAL:?}/argv.log"
answer=$(head -n1 "$F1R3GAZE_FAKE_PORTAL/scheme" 2>/dev/null || printf 0)
case $answer in
0 | 1 | 2) printf '   variant       uint32 %s\n' "$answer" ;;
hang) exec sleep 30 ;;
fail)
    printf 'Error org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.portal.Desktop was not provided by any .service files\n' >&2
    exit 1
    ;;
*)
    printf 'Error org.freedesktop.DBus.Error.Failed: the fake portal has no answer %s\n' "$answer" >&2
    exit 1
    ;;
esac
SH
    chmod +x "$work/fake-bin/dbus-send"
    export PATH=$work/fake-bin:$PATH
}

storage_paths_inside() {
    local printed=$1 inside class path
    inside=$(storage_abspath "$2")
    while IFS=$'\t' read -r class path; do
        [[ -n $class ]] || continue
        path=$(storage_abspath "$path")
        if [[ $path != "$inside"/* ]]; then
            printf 'the self-test failed: %s %s is outside %s\n' "$class" "$path" "$inside" >&2
            exit 3
        fi
        storage_guard "$path"
    done <<<"$printed"
}

storage_paths_self_test() {
    local bin=$1 work=$2 profile=$3 printed
    printed=$("$bin" --profile "$profile" paths) || { printf 'the self-test failed: %s paths\n' "$bin" >&2; exit 3; }
    printed+=$'\n'$(env -u F1R3GAZE_PROFILE HOME="$work/home" "$bin" paths) ||
        { printf 'the self-test failed: %s paths (no --profile)\n' "$bin" >&2; exit 3; }
    # Was the loop below, before scenes with roots of their own needed it too:
    # while IFS=$'\t' read -r class path; do … storage_guard "$path"; done <<<"$printed"
    storage_paths_inside "$printed" "$work"
    if [[ $(command -v dbus-send) != "$work/fake-bin/dbus-send" ]]; then
        printf 'the self-test failed: dbus-send is %s, not the fake\n' "$(command -v dbus-send)" >&2
        exit 3
    fi
}
