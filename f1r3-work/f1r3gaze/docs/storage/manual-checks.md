# Manual checks on macOS and Windows

The automatic checks run on Linux: the tests on every system in CI, the
snapshot harness under X11 (Xvfb). What only a person at a Mac or a Windows
machine can see is listed here, one check at a time, with what each must
show and the ledger entry it verifies. The results go in the tables at the
end of each section, and from there into the
[storage ledger](ledger.md) (entry S18, part 2).

## Before you start

**Build** the branch's release binary on the machine:

```text
git switch feature/state-management
cd f1r3-work/f1r3gaze
cargo build --release --locked -p gaze-shell
```

The binary is `target/release/f1r3gaze` (`f1r3gaze.exe` on Windows). The
commands below call it `f1r3gaze`.

**A throwaway profile.** Every check but the first runs with
`--profile DIR`, a folder of its own that holds all five roots, so nothing
touches another F1R3Gaze's files. Use a new, empty folder for the first
start of each check that says so, for example `~/f1r3gaze-check` on macOS
and `%USERPROFILE%\f1r3gaze-check` on Windows; remove it when you are done.

**Recording.** For each check write *pass* or *fail* and what you saw. For a
failure, a screenshot and the folder's `state/window.json` (if the check
names it) help most.

## macOS

| # | Check | Steps | Expected | Verifies |
|---|---|---|---|---|
| M1 | Where the files go | `f1r3gaze paths` (no `--profile`; it touches no file) | `config`, `data` and `state` under `~/Library/Application Support/io.f1r3fly.f1r3gaze/`; `cache` at `~/Library/Caches/io.f1r3fly.f1r3gaze`; `runtime` under `$TMPDIR` | README §4.1 |
| M2 | Cmd shortcuts | `f1r3gaze --profile DIR`, then Cmd+T, Cmd+L, Cmd+R, Cmd+F, Cmd+B, Cmd+W, Cmd+Shift+T, Cmd+1 to Cmd+9, Ctrl+Tab | a new tab; the address field focused; a reload; the find bar; the sidebar toggled; the tab closed; the tab reopened; that tab selected; the next tab | chrome ledger L12 |
| M3 | History and Hide | Cmd+Y, then Cmd+H | Cmd+Y opens History in the sidebar; Cmd+H hides F1R3Gaze (macOS's menu) and opens nothing | L16 |
| M4 | The keys the tips name | hover the new-tab button, the sidebar's buttons, the reload button; open a new tab and read its hints | every tip and hint names Cmd (Cmd+T, Cmd+Y, …), never Ctrl | L17 |
| M5 | The system's scheme | Appearance → System; switch System Settings → Appearance between Light and Dark | the chrome and the title bar follow within a second, both ways | S12 part 3 |
| M6 | A choice over the system | macOS Light; Appearance → Dark; then Appearance → System | Dark: the chrome and the title bar dark; System: both light again | S12 part 3 |
| M7 | Size and place, in points | on a Retina display: move the window to about (200, 150), resize it to about 1000 × 700, quit with Cmd+Q, start again with the same `--profile` | the same size and place; in `state/window.json`, `normal` has `"units": "points"` | S13 part 2 (Quit closes no window first: the save comes from `Drop`) |
| M8 | A second display | if there is one: move the window onto it, quit, start again; then quit with it there, disconnect the display, start again | it opens on that display; with the display gone, it opens on the main display, wholly visible | S13 part 1 and 2 |
| M9 | Full screen | Ctrl+Cmd+F; read the status bar; Ctrl+Cmd+F; then enter full screen and quit, start again, leave full screen | the window goes full screen and the status bar says "Press Ctrl+Cmd+F to leave full screen"; it comes back to the size before; after the restart it opens full screen, and leaving restores the size before | S13 part 2 |
| M10 | The zoom | Cmd+= three times, quit, start again; then Cmd+− twelve times | the same zoom after the restart; the zoom stops at 25 % | S13 part 2, L15 |
| M11 | A second instance | with the window open on `DIR`: `f1r3gaze --profile DIR --headless gaze://newtab`, then `f1r3gaze --profile DIR wallet list` | the first exits with status 1 and names the holder (process, `window`, its start time); the second runs, reading only | README §8 |

| # | Result | What you saw |
|---|---|---|
| M1 | | |
| M2 | | |
| M3 | | |
| M4 | | |
| M5 | | |
| M6 | | |
| M7 | | |
| M8 | | |
| M9 | | |
| M10 | | |
| M11 | | |

## Windows

| # | Check | Steps | Expected | Verifies |
|---|---|---|---|---|
| W1 | Where the files go | `f1r3gaze paths` (no `--profile`; it touches no file) | `config` at `%APPDATA%\f1r3fly-io\f1r3gaze`; `data`, `state`, `cache` and `runtime` under `%LOCALAPPDATA%\f1r3fly-io\f1r3gaze\` | README §4.1 |
| W2 | Ctrl shortcuts | `f1r3gaze --profile DIR`, then Ctrl+T, Ctrl+L, Ctrl+R, Ctrl+F, Ctrl+B, Ctrl+H, Ctrl+W, Ctrl+Shift+T, Ctrl+1 to Ctrl+9, Ctrl+Tab | as M2, with Ctrl+H opening History; every tip names Ctrl | L16, L17 |
| W3 | The title bar from the first frame | Windows in dark mode (Settings → Personalisation → Colours); Appearance → System; close and start again | the title bar is dark from the first frame, with no light flash | S13 part 2 (a window made with no theme) |
| W4 | The system's scheme while open | Appearance → System; switch Windows between Light and Dark | the chrome and the title bar follow within a second, both ways | S12 part 3 (`WM_SETTINGCHANGE`) |
| W5 | A choice over the system | Windows Light; Appearance → Dark; then Appearance → System | Dark: the chrome and the title bar dark; System: both light again | S12 part 3 |
| W6 | Size and place at a display scale | on a display at 150 %: move the window, resize it to about 1000 × 700, close it, start again | the same size and place | S13 part 2 (the scale at creation) |
| W7 | Maximized | maximize, close, start again; then restore down | it opens maximized; restoring down gives the size before maximizing | S13 part 2 (maximize after the window is shown) |
| W8 | Minimized | minimize, then close it from the taskbar (right-click → Close window), start again | it opens at its last normal place, visible (not at −32 000, −32 000) | S13 part 2 |
| W9 | Full screen | F11; read the status bar; F11; then enter full screen, close, start again, F11 | full screen, with "Press F11 to leave full screen"; back to the size before; after the restart it opens full screen, and F11 restores the size before | S13 part 2 |
| W10 | The zoom | Ctrl+= three times, close, start again; then Ctrl+− twelve times | the same zoom after the restart; it stops at 25 % | S13 part 2, L15 |
| W11 | A second instance | with the window open on `DIR`: `f1r3gaze --profile DIR --headless gaze://newtab`, then `f1r3gaze --profile DIR wallet list` | the first exits with status 1 and names the holder, read from `instance.pid` (Windows forbids reading the locked file); the second runs, reading only | README §8 |

| # | Result | What you saw |
|---|---|---|
| W1 | | |
| W2 | | |
| W3 | | |
| W4 | | |
| W5 | | |
| W6 | | |
| W7 | | |
| W8 | | |
| W9 | | |
| W10 | | |
| W11 | | |
