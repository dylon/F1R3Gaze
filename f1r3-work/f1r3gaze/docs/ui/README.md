# The F1R3Gaze chrome: design

The **chrome** is everything in the F1R3Gaze window except the page itself:
- the tab strip;
- the toolbar with the address box;
- the icon rail and the sidebar panels;
- the prompt bar;
- the find box;
- the status bar.

This document describes what each part does, how it is built, and why it is
built that way, in enough detail to rebuild it. The history behind the
design is in two places:
- [the defect ledger](ledger.md): every bug, its hypotheses, experiments, and
  verdicts.
- [`../screenshots/ui/`](../screenshots/ui/): before and after captures of
  every dialog.

**Terms used throughout.**

| Term | Meaning |
|---|---|
| **px** | A CSS pixel. The snapshots use a 1280 × 800 window at scale 1, so one CSS pixel is one device pixel. |
| **Blitz** | The HTML/CSS engine the chrome and pages render with ([DioxusLabs/blitz](https://github.com/DioxusLabs/blitz), pinned at rev `674d7d2`). It uses Stylo for styles, Taffy for layout, **parley** for text, and **Vello** for painting. |
| **Region** | An element of the chrome document whose content the chrome code rebuilds as a unit. |
| **Token** | A named colour variable (`--gaze-*`) of the colour scheme. |
| **WCAG** | The W3C Web Content Accessibility Guidelines 2.1 [W3C-WCAG21]. |
| **Contrast ratio** | The WCAG measure of how distinct two colours are, from 1:1 (identical) to 21:1 (black on white). Defined in §4.2. |
| **winit** | The Rust windowing library under blitz-shell. It turns the platform's input and window changes into events such as `SurfaceResized`, `PointerLeft`, and `RedrawRequested`. |
| **Compositor** | The part of the system that places windows on the screen and tells each application its size: KWin on KDE Plasma, the window server and AppKit on macOS, the X server and its window manager on X11. |
| **Configure** | A Wayland compositor's message that gives a window its next size. The application answers with a frame of that size. |
| **Frame** | One pass that lays the window out, paints it, and presents the image (§3.5). |

## Contents

1. [Principles](#1-principles)
2. [How the chrome is built](#2-how-the-chrome-is-built)
3. [From an event to the screen](#3-from-an-event-to-the-screen)
4. [Colour tokens and contrast](#4-colour-tokens-and-contrast)
5. [Type, sizes, and spacing](#5-type-sizes-and-spacing)
6. [Conventions R1–R8](#6-conventions-r1r8)
7. [Components](#7-components)
8. [The parts of the chrome, one by one](#8-the-parts-of-the-chrome-one-by-one)
9. [Keyboard](#9-keyboard)
10. [Algorithms](#10-algorithms)
11. [Engine facts the design relies on](#11-engine-facts-the-design-relies-on)
12. [Verifying the chrome](#12-verifying-the-chrome)
13. [References](#13-references)

---

## 1. Principles

1. **One look.** Every control comes from one small vocabulary of components
   (§7), styled from one set of colour tokens (§4) and one spacing grid (§5).
   A new panel reuses these instead of inventing styles.
2. **Nothing jumps.** Interface that appears on demand never moves the page or
   the panels. This covers the find box, the address suggestions, hover labels,
   and row actions. They float above the content (§2.3).
3. **Exact text.** A label too long for its box is shortened to the longest
   prefix (or window) that fits, measured with the same text engine Blitz uses,
   and ends in "…". Blitz implements no `text-overflow`, so the chrome does
   this itself (§10.1).
4. **Keyboard first.** Every frequent action has a key. Fields behave as in
   other browsers: Ctrl+L selects the address, and Escape backs out one step
   at a time (§9).
5. **Show why.** A search result shows the text it matched (§10.2). A disabled
   control looks disabled. An empty panel says what would appear there and how
   to get it.
6. **Legible.** Text reaches a WCAG contrast of at least 4.5:1, and bars, rings,
   and icons at least 3:1, in both built-in schemes and in any accepted theme
   file (§4).
7. **Engine-aware.** The conventions in §6 encode facts about Blitz, Vello, and
   parley that would otherwise cause defects. Tests enforce them (§12).

---

## 2. How the chrome is built

### 2.1 One document, rebuilt by region

The chrome is a single HTML document. `crates/gaze-shell/src/chrome.rs`
generates it, and Blitz renders it. Each tab's page is a separate document
(a `RhoDocument`), attached as a *sub-document* inside the chrome's `#main`
element.

The chrome's markup is updated in three ways:

| Strategy | Used for | Mechanism |
|---|---|---|
| **static** | the toolbar, the rail, the find box, the wallet form, the new-tab button | Built once by `shell_html` and never replaced, so text fields keep their content, caret, and focus. |
| **`set_html`** | regions whose content is data: the tab list, a sidebar panel, suggestions, the prompt bar, the status bar… | A pure builder function returns the region's HTML from plain data. `set_html` replaces the region's children only when the string differs from the last one set (the *render cache*). |
| **`sync_attr`** | the state of static controls: `disabled`, `aria-pressed`, `class`, the ghost hints | `sync_attr(id, name, value)` compares the live attribute and writes only on a difference. |

The builders (`tab_panel_html`, `history_panel_html`, `strip_layout`, …) take
plain data and return strings. That is why most of the chrome can be tested
without a window or an engine (§12).

![The chrome's regions, their ids, and how each is updated](diagrams/chrome-regions.svg)

### 2.2 Regions

| Region | Contents | Built by | Strategy |
|---|---|---|---|
| `#tabs` | `#tablist` (the tabs and the "+N" chip), `#newtab` | `tab_strip_html` | `set_html` (list), static (button) |
| `#toolbar` | `#sidetoggle`, `#back`, `#fwd`, `#reload`, `#urlwrap`, `#findbtn` | `TOOLBAR_HTML` | static + `sync_attr` |
| `#scheme` | the address badge | `scheme_badge_html` | `set_html` |
| `#suggestions` | the address dropdown | `suggestions_html` | `set_html` |
| `#prompt` | the question a page is asking | `prompt_bar_html` | `set_html` |
| `#rail` | seven panel buttons | `RAIL_HTML` | static + `sync_attr` (`.on`) |
| `#sidehead` | panel title, count, Hide | `side_head_html` | `set_html` |
| `#sidecontrols` | search field and chips, per panel | `*_CONTROLS_HTML` | constants: byte-identical while typing |
| `#panel` | the open panel's body | one builder per panel | `set_html` |
| `#wallet` | the wallet form + `#walletmsg`, `#walletcard`, `#walletlist` | `WALLET_FORM_HTML` + builders | static + `set_html` |
| `#main` | page views, `#findoverlay`, `#findbar` | `FIND_BOX_HTML`, `render_find_overlay` | static + `set_html` |
| `#status` | stage, reason or notice, message | `status_html` | `set_html` |

### 2.3 Layers

Blitz takes an element out of normal paint order ("hoists" it) only when it is
positioned **and** has a non-zero `z-index`. A hoisted element:
- paints above the page sub-documents;
- is hit-tested first;
- ignores clips from its ancestors.

The chrome therefore gives its overlays explicit z-indices and never gives one
to anything else (convention R2).

| z-index | Element | Why it floats |
|---|---|---|
| 40 | `.tip`, the hover labels | Blitz shows no `title` tooltips |
| 30 | `#suggestions` | must cover the page and the panels, without moving them |
| 6 | `#findbar` | floats at the top right of the page; the page does not reflow |
| 2 | `#findoverlay` | highlights over the page's text |

![Layers: what paints over what](diagrams/layers.svg)

---

## 3. From an event to the screen

### 3.1 The pipeline

![Event → Action → render](diagrams/event-pipeline.svg)

The window hands every input to `ChromeDocument::handle_ui_event`. Key presses
are handled in three stages, and the first stage that recognises a key stops
it from going further:

1. **`global_shortcut`** handles browser shortcuts (§9). They work whatever has
   the focus, including a page.
2. **`chrome_key`** handles the keys of the chrome's own fields, by which field
   has the focus (`KeyFocus`): Enter, ↑, ↓, and Esc in the address box; Enter
   and Esc in the find box; Esc in the sidebar search. Esc anywhere else gives
   `Dismiss`, *and* the key still reaches the page, which may handle Esc
   itself.
3. Everything else goes to Blitz's **`EventDriver`**. It hit-tests, moves the
   focus, edits text fields, and dispatches DOM events to the
   **`ChromeHandler`**:

| DOM event | The handler emits |
|---|---|
| Click | Walks up from the target to the nearest `[data-action]` and parses it (`Action::parse`). The walk stops at a `[disabled]` element, because Blitz still dispatches clicks to disabled buttons. |
| Blur of `#url` | `AddressDismiss` |
| ContextMenu on a tab | `tab:menu-open:<id>` (the tab's menu, in the Tabs panel) |
| Input | `Input(field, text)` for `#url`, `#find`, `#side-search` |
| Pointer down/up on tabs | drag to reorder, middle-click to close |

Each `Action` is then applied by `act`, which changes state only. Nothing is
drawn until the next frame. There, `poll` attaches newly loaded pages and
`render` brings every region up to date.

**Click before Blur.** Clicking a suggestion delivers Click first; the default
action then clears the focus, which delivers Blur. So `Select(id)` switches
the tab *before* `AddressDismiss` closes the dropdown. Dismissing on Blur is
therefore safe.

### 3.2 The action protocol

Controls carry `data-action="verb[:argument]"`. `Action::parse` accepts:

| Verb | Argument | Action |
|---|---|---|
| `back`, `fwd`, `reload`, `newtab` | — | navigation, new tab |
| `select:<id>`, `close:<id>` | tab id | switch to or close a tab |
| `panel:<name>` | panel | open it, or hide the sidebar if it is already open |
| `show:<name>` | panel | open it, never toggling (the "+N" chip, the badge) |
| `sidebar:toggle` | — | show or hide the sidebar (Ctrl+B) |
| `tab:<verb>:<id>` | `menu`, `menu-open`, `duplicate`, `copy`, `left`, `right`, `others`, `close-right`, `branch`, `restore`, `restore-at`, `move-to` (drag and drop) | tab actions |
| `visit:open\|forget:<url>` | url | History rows |
| `history:clear-ask\|clear-cancel\|clear\|more:0` | — | History controls |
| `site:<verb>:<arg>` | `store`, `grants`, `cache`, `session:<tab>:<label>` | Site data actions |
| `revoke:<urn>`, `allow:<tab>:<prompt>`, `deny:…`, `remember` | — | permissions and prompts |
| `wallet:<verb>[:<arg>]` | `new`, `import`, `import-file`, `send`, `confirm`, `cancel`, `use`, `copy`, `export`, `remove`, `dismiss` | Wallet; `import-file` uses a native open panel and `export` chooses a destination folder |
| `theme:<op>` | `system`, `dark`, `light`; `use:<name>`, a theme file's name as `theme::check_theme_name` allows it; `new`; `reload`. Anything else is refused: the step-9 `next`, `template` and `custom` no longer parse | Appearance: System follows the operating system's preference, Dark and Light are the built-in schemes, and `use` chooses `themes/<name>.css` (a file that cannot be used is not chosen, and the status bar says why). New theme saves the colours shown as `themes/my-theme.css` (`my-theme-2`, … when a name is taken; never over anything) and chooses it; Reload reads the theme files and the chosen theme again. A choice is saved as `theme` under `[appearance]` in `settings.toml`, and the window is asked to show its scheme (§3.6) |
| `find:show\|next\|prev\|hide` | — | find |
| `go`, `suggest:<url>` | url | open the typed address, or a history suggestion |
| `tree:toggle`, `side:pages\|clear`, `console:clear`, `savelog`, `letitrun`, `dismiss` | — | the rest |

The test `every_emitted_action_parses` walks the shell and a sample of every
builder's output, and checks that each `data-action` parses.

### 3.3 One render pass

![render(): the order of one pass](diagrams/render-order.svg)

Every step compares before writing, so an idle frame writes nothing. The order
matters in three places:
- The panel is hidden for the Wallet **before** any panel HTML is set, so no
  gap opens above the wallet form.
- A field's text is selected **after** `render_address` has restored the page
  address, so the selection covers the restored text.
- The badge is computed from the field's live text, after the address has been
  restored.

### 3.4 The pointer over pages: cursor and hover

The window has one mouse cursor, but two documents have a say in it: the
chrome, and the page under the pointer. Each page is a Blitz sub-document of
the chrome, hosted by a `.view` element. With pages as Blitz builds them, the
cursor disappeared over a page after Back, a rail click or a tab-row click,
and links never showed the hand ([ledger L8](ledger.md)). Three engine facts
combine to cause this:

| # | Fact (Blitz `674d7d2`) | Effect on pages | Source |
|---|---|---|---|
| 1 | The chrome's hit test stops at a page's host. A document asks its shell for a new cursor only when its hovered element changes. | Moving inside a page never changes the chrome's hover, so the chrome never asks for a new cursor. | `node/node.rs` `hit_inner`; `document.rs` `set_hover_to` |
| 2 | When the hover moves onto a host, the chrome reads the page's cursor *before* it forwards the event to the page. A page with nothing hovered answers `None`, and blitz-shell hides the cursor for `None`. | Entering a page that has not seen the pointer yet hides the cursor (every page after Back, a navigation, or a lazy tab's first selection). Entering any other page shows the cursor of wherever the pointer last was in it. | `events/driver.rs` `handle_ui_event`; `document.rs` `get_cursor`; `blitz-shell/src/lib.rs` `set_cursor` |
| 3 | A document built without a shell provider gets `DummyShellProvider`, and sub-documents receive no enter or leave events. | A page's own cursor and redraw requests are lost. Its `:hover` state outlives the pointer's visit. | `document.rs` `BaseDocument::new`; `events/mod.rs` `map_dom_event_to_ui_event` |

**Why pages do not get the window's provider.** Blitz's own iframes share
their parent's provider (`iframe.rs`). That would fix fact 3, but the provider
is also the window's title, clipboard, file dialogs and window controls, and a
page could then retitle the window (`mutator.rs` sets it from `<title>`). Pages
hold capabilities, not ambient authority, so they get a provider that can
only *report*.

![The cursor arbiter](diagrams/cursor-arbiter-components.svg)

| Part (`crate::cursor`) | What it is | What it does |
|---|---|---|
| `CursorRouter` | Two `AtomicBool`s, `dirty` and `repaint`, and the chrome's `WakeHandle` | Raising a flag wakes the window's event loop only when the flag was clear, so a burst of reports costs one poll. |
| `WindowShell` | The chrome's provider: blitz-shell's, wrapped | Forwards the 13 other `ShellProvider` methods unchanged. `set_cursor` only raises `dirty`. `apply` is the one path by which a cursor reaches the window. |
| `PageShell` | Every page's provider, through `Tab::config` (so iframes inside a page inherit it) | `set_cursor` raises `dirty`, `request_redraw` raises `repaint`, and every other method keeps the dummy's no-op. |
| `CursorArbiter` | A field of `ChromeDocument` | Decides the cursor, and moves the pointer's hover between pages. |

blitz-shell sets its provider in `View::init`, after the document is built,
and offers no hook. So `install` runs first in every `handle_ui_event` and
`poll`, and wraps the provider whenever it is not already a `WindowShell`.

![The pointer enters a page](diagrams/pointer-into-page-sequence.svg)

**The sync.** In literate form [Knuth 1984]:

```text
⟨sync⟩ ≡
    host ← the chrome's hovered element, if it hosts a page
    if host ≠ hovered:
        ⟨end the hover of the page the pointer left⟩
        ⟨tell the page the pointer entered where the pointer is⟩
        hovered ← host
    want ← window_cursor(chrome.get_cursor(), hidden_by_css(chrome))
    if shown ≠ want:
        window.apply(want);  shown ← want
    dirty ← false
    return whether a page's hover changed      — its :hover styles need painting

⟨end the hover of the page the pointer left⟩ ≡
    if the old host still hosts a page:  page.clear_hover()

⟨tell the page the pointer entered where the pointer is⟩ ≡
    if host ≠ None and the pointer's position is known:
        page.set_hover_to(page_point(pointer, host's position, page's scroll))
```

`chrome.get_cursor()` reads through the host into the page, so the answer is
the page's own cursor. `sync` runs only after the page has seen the event, so
that answer is current. Seeding and clearing change hover state only. No DOM
event is dispatched, just as with Blitz's own `refresh_hover`.

`page_point` maps a point in the chrome into the page's coordinates, as
Blitz maps the events it forwards (`adjust_coords_for_subdocument`):

```math
\mathbf{p}_{\mathrm{page}} = \mathbf{p}_{\mathrm{chrome}} - \mathbf{o}_{\mathrm{host}} + \mathbf{s}_{\mathrm{page}}
```

Here $`\mathbf{p}_{\mathrm{chrome}}`$ is the pointer's last position in the
chrome and $`\mathbf{o}_{\mathrm{host}}`$ is the host's absolute position
(`absolute_position(0, 0)`). $`\mathbf{s}_{\mathrm{page}}`$ is how far the
page has scrolled (`viewport_scroll()`).

**Reading Blitz's `None`.** `get_cursor` answers `None` in two cases, and only
one of them should hide the cursor:

| Blitz's answer | Hovered element's computed `cursor` | The window shows |
|---|---|---|
| `Some(icon)` | any | `icon` |
| `None` | `none` | nothing: the page hid the cursor, as on the web |
| `None` | anything else, or nothing hovered | the arrow: a page that has not seen the pointer yet, or is not laid out yet |

The computed value comes from `resolved_style_value(id, "cursor")` on the
deepest hovered document.

![Pointer hover across the chrome and its pages](diagrams/hovered-page-states.svg)

**When the sync runs.**

| Trigger | Who reports it | Synced at |
|---|---|---|
| A pointer event | `WindowShell` (the chrome's hover changed) or `PageShell` (a page's hover changed) | the end of `handle_ui_event` |
| A layout change under a still pointer: a tab switch, the sidebar toggled, a page reflowed or scrolled. Blitz re-resolves hover in `resolve` (`refresh_hover`). | either shell: raises `dirty` and wakes the loop | the next `poll` |
| A page loads under the pointer | `page_attached` seeds it; its first layout hovers the right element | the same `poll`, then the one after that layout |
| A page asks to be painted (a hover restyle, a scroll, a loaded image) | `PageShell` raises `repaint` | `handle_ui_event` asks the window to redraw; `poll` reports a change, which blitz-shell turns into a redraw |
| A tab is closed | `view_removed` forgets its host, because Blitz reuses node ids | — |

The window is asked for a cursor only when it changes, and moving within one
element asks nothing of it. `shown` is exact because nothing but `apply`
writes the cursor.

Built-in pages give their buttons `cursor: pointer`, as the chrome does.
Blitz's default style sheet sets no `cursor`, which would leave a text cursor
over a button's label (§8.14).

**Maintenance.** `WindowShell` must forward every `ShellProvider` method, or
the trait's default no-op would silently swallow it. The test
`window_shell_forwards_everything_but_the_cursor` lists them all. A Blitz
upgrade that adds a method must add it in both places.

The pointer leaving the *window*, rather than a page, is handled one level up,
by the window's application handler (§3.5).

### 3.5 Resizing: one frame per size

![A window resized by its frame](diagrams/resize-frame-sequence.svg)

When the user drags a window's frame, the compositor turns each pointer
position into a new size, and the application paints the window at that size.
If a size takes $`t`$ seconds to reach the screen, and the pointer moves at
$`v`$ pixels per second, the border trails the pointer by about $`v \cdot t`$
pixels. KWin 6.7.5 never waits for the application: each pointer motion
computes the frame from the pointer's absolute position
(`isWaitingForInteractiveResizeSync` is `false` for Wayland windows). AppKit
paints each step of a live resize inside its own notification. Either way,
everything rests on how quickly the application's frames come out.

By the end of the chrome overhaul, the window painted most sizes twice. The
second frame had no layout to do, yet cost a full render and present
([ledger L9](ledger.md)). `crate::application::ChromeApplication` wraps
Blitz's application handler, forwards every method to it, and adds three
things:

| | The cause (Blitz `674d7d2`, winit 0.31.0-beta.3) | What the chrome does |
|---|---|---|
| H8 | blitz-shell forwards nothing when the mouse leaves the window. Every layout re-resolves hover at the pointer's last position (`refresh_hover`). Grabbing the frame takes the pointer out of the window, the page reflows under the old spot, and each change of the hovered node asks for a redraw. | On a mouse `PointerLeft`, `ChromeDocument::pointer_left` ends the hover of the chrome and of the page under the pointer, with `clear_hover`: what Blitz does when a finger lifts. This also ends a stale `:hover` after the pointer leaves the window. |
| H9 | The chrome's layout pass gives each page its viewport and lays the page out right away. Setting the viewport makes the page ask for a redraw (`queue_device_changes`), which the frame being painted already answers. Since L8, a page's request reaches the window (§3.4). | `begin_paint` and `end_paint` bracket every `RedrawRequested`. A page's request made in between, on the painting thread, is set aside. A request from another thread, such as a finished resource load, wakes the window as before. One more frame is granted only if a page's hovered node changed during the frame, because that restyle is the one Blitz defers to the next pass. |
| H7 | blitz-shell polls the document through the event-loop proxy after each window event. winit-wayland delivers that wake-up at the start of the next loop iteration, after the iteration's redraw. AppKit paints a live-resize step before the run loop reaches it. The frame for a new size showed the tab strip fitted to the old width, and the poll then painted again. | After `SurfaceResized` or `ScaleFactorChanged`, the window is polled at once (`View::poll`), so the frame for a size is fitted to it. If the poll changed the chrome, the window is also laid out at once: Blitz hit-tests with the last layout, and an input event before the frame would meet the replaced nodes (§11). |

**Why the hovered node, not dirty flags.** Blitz leaves a restyle for the next
pass by setting `dirty_descendants` flags. Those flags cannot tell that a
restyle is pending: `clear_damage_and_dirty_flags` returns early for a node
without damage and leaves them set (`layout/damage.rs:193-203`). So a restyle
that changes nothing visible, such as a hover over an element with no `:hover`
rule, leaves them set for good. Comparing each page's hovered node before and
after the frame asks the precise question.

**Coalescing.** `crate::renderer::CoalescingRenderer` wraps Vello's window
renderer. `set_size` records the size, and `render` applies the last one,
once, just before rendering. Under X11, several resizes arrive between two
frames. In the Xvfb sweep, coalescing cut surface reconfigurations from 16 to
0.95 per frame, and the frame rate rose from 3 to 18 a second. Wayland and
macOS deliver one resize per frame, so there it changes nothing. While the
renderer is not yet active (Vello until its resume completes), sizes pass
straight through and none counts as applied: blitz-shell sets the size again
once the renderer is active, and that request must not be mistaken for a
repeat (ledger L9, the regression the snapshot harness caught).

**Measured** (ledger L9). This counts the frames painted without a new size,
per frame that applied one:

| Run | `main` | Before L9 | After L9 |
|---|---|---|---|
| Xvfb, 40 resizes at 10 a second (3 runs) | 1 per run | 39–40 per run | 1 per run |
| Live drag on KWin/Wayland, share of sizes followed by an extra frame | 2.1–2.6 % | 40–88 % | 0.0 % |

**Every platform.** Nothing here depends on a compositor. The three additions
change when the chrome does its own work (H7–H9), and coalescing changes when
the renderer reconfigures. The orderings they rely on were read in winit's
sources for Wayland (`event_loop/mod.rs:354-546`) and for macOS (winit-appkit
`view.rs:170-183`). X11 was measured under Xvfb. Windows was not tested.

**What stays outside the chrome.** Vello waits for the GPU after every frame
(`device.poll(wait_indefinitely)`). Anything else that keeps the GPU busy
therefore slows every frame. In L9, another process's compute work held a
resize to about 85 frames a second instead of 120 (H11). That is not worked
around.

**Maintenance.** `ChromeApplication` must forward every `ApplicationHandler`
method. `#[deny(clippy::missing_trait_methods)]` fails the build if a winit
upgrade adds one. Its `winit` dependency is pinned to the version blitz-shell
pins, so both name the same crate.

### 3.6 The scheme beyond the chrome: pages and the window

The scheme shown is the resolved theme's (`theme::Resolved::scheme`): a
built-in scheme, or a theme file's, classified by its background (§4.3).
The chrome draws itself in its colours (§4), and gives every built-in page
its host sheet. Two more things show the scheme, and the chrome draws
neither of them: the pages, and the window's decorations.

![A theme change: the chrome, its pages, the window's decorations and the settings file](../storage/diagrams/theme-change-sequence.svg)

**Pages (ledger L13).** A page's `prefers-color-scheme` is the scheme
shown, as in Chrome and Firefox. Blitz evaluates the media query against a
document's viewport (`Viewport::color_scheme`). The chrome's layout pass
copies its own viewport's scheme into the viewport of every page it holds,
hidden tabs included, and then lays the page out (blitz-dom
`resolve.rs:150`). So `paint_theme` sets the scheme on the chrome's own
viewport, and every page follows at the chrome's next layout, before the
page is painted; nothing is set on a page itself. A change restyles each
page once, because Stylo recascades a whole document when its device's
colour scheme changes.

Stylo, as Blitz uses it, also gives an element whose `color-scheme` is
`normal` the system colours of the preferred scheme (`Mark`, `Canvas`, …),
which Blitz's own style sheet uses for `mark`, `dialog` and popovers. Chrome
and Firefox draw such an element light: a page that declares no colour
scheme is light. Every page therefore gets one more user-agent style sheet,
`:root{color-scheme:light}` (`tab::PAGE_SCHEME_CSS`). A page's own
`color-scheme` overrides it, since an author's rule beats a user agent's
(§11), so a page that declares `light dark` follows the scheme shown in its
system colours too.

**The window.** Only `ChromeApplication` holds the winit window, so the
chrome asks it through a queue of `WindowRequest`s. The application drains
the queue (`take_window_requests`) after Blitz creates the window, after
each of the window's events, and once per turn of the event loop:
- **`Scheme { effective, follows_system }`**, queued by every `paint_theme`;
  only the latest is kept. It sets Blitz's theme override
  (`View::set_theme_override`), which keeps the window's own theme from
  replacing the chrome's scheme. Without it, `View::init` would set the
  chrome's scheme from `Window::theme()`, which X11 never knows, so every
  page would be light again once the window exists, and so would each
  `ThemeChanged` (blitz-shell `window.rs:181, 633`; winit-x11
  `window.rs:2288-2290`). It also gives the window's decorations
  `decoration_theme(platform, effective, follows_system)`:

  | Platform | Following the system | An explicit choice |
  |---|---|---|
  | macOS, Windows | no theme: the window follows the system itself, and reports its changes (`ThemeChanged`) | the scheme shown |
  | X11, Wayland | the scheme shown: nothing is reported, and a window with no theme is dark on X11 (`_GTK_THEME_VARIANT`) and the desktop's on Wayland | the scheme shown |

  Each of the two is set only when it changes (`scheme_change`): Windows
  redraws its title bar at every `set_theme`.
- **`FollowSystem`**, when System is chosen again. macOS reports no change
  while a window has an appearance of its own (winit-appkit
  `window_delegate.rs:656`), so the application reads `system_theme()` and
  reports it to the chrome.
- **After every `ThemeChanged`** the chrome asks for the scheme again, and
  the application forgets the decorations it gave. Windows re-applies the
  system's theme to a window created without one whenever the system's
  settings change (winit-win32 `event_loop.rs:2509-2520`), even while an
  explicit choice is shown; the next request sets the decorations back.
- **`ToggleFullScreen`**, from F11 (Ctrl+Cmd+F on macOS; §9): one request
  per press. The application reads the window first, so that leaving full
  screen restores what was there, then enters full screen on the window's
  monitor (`Fullscreen::Borderless(None)`) or leaves it; the status bar says
  how to leave (§8.5).

### 3.7 The window: made where it was, and kept

The window's size and place, whether it was maximized or full screen, and
its zoom are kept in `state/window.json` (storage ledger S13; the rules, per
platform, are in [`docs/storage/README.md`](../storage/README.md),
section 13).

![The window: made where it was left, followed, and kept in window.json](../storage/diagrams/window-geometry-sequence.svg)

- **Made once the monitors can be listed.** `launch()` hands
  `ChromeApplication` a `PendingWindow`: the chrome, the renderer, the
  saved state and a saver (the chrome, which holds the profile, goes with
  the window at `CloseRequested`). In `can_create_surfaces`,
  `create_window` asks `plan_restore` where to put it, among the monitors
  winit lists, and has Blitz make it with those attributes
  (`WindowConfig::with_attributes`), with its decorations' theme, except on
  Windows (`creation_theme`; §3.6).
- **The order after `View::init`.** Blitz makes the window hidden, shows
  it, and gives the chrome a viewport at zoom 1 with the window's theme.
  Then come the saved zoom, the scheme shown (the window requests of §3.6),
  maximized, and full screen on the saved monitor, in that order: a window
  manager ignores a maximize asked of a hidden window.
- **Followed, not read, on the resize path.** `SurfaceResized`, `Moved` and
  `ScaleFactorChanged` only mark the window changed (`Keeper::changed`).
  `about_to_wait` sets `ControlFlow::WaitUntil` for the save that is due;
  Blitz never sets the control flow itself. The window is read, and
  `window.json` written if what it holds changed, when a save is due (500 ms
  after the last change, at least every 3 s, and 500 ms after the window is
  made), before it closes, when it loses a focus it had, and around full
  screen; `Drop` writes the last reading when the loop ends.
- **The zoom.** Blitz's keys (Ctrl+=, Ctrl+−, Ctrl+0) are kept within
  $`[0.25, 5]`$ after every key, and kept to hundredths (ledger L15).
- **A frame-times A/B.** `F1R3GAZE_KEEP_WINDOW=0` turns the keeping off
  (§12.3); each save is counted and timed in the frame line.

---

## 4. Colour tokens and contrast

### 4.1 Tokens

`crates/gaze-shell/src/theme.rs` defines twelve tokens. Their values in the two
built-in schemes:

| Token | Role | Dark | Light |
|---|---|---|---|
| `--gaze-bg` | window background, fields, cards | `#161b26` | `#f5f7fa` |
| `--gaze-surface` | toolbar, sidebar, dropdowns, find box | `#202735` | `#ffffff` |
| `--gaze-raised` | buttons, tags, prompt bar | `#293244` | `#eaf0f4` |
| `--gaze-text` | text | `#eaf0f7` | `#182734` |
| `--gaze-muted` | secondary text, idle icons | `#a8b7c9` | `#526879` |
| `--gaze-border` | borders, disabled text | `#3a4659` | `#c9d7df` |
| `--gaze-accent` | bars, rings, active icons, primary buttons | `#71d4cf` | `#087f86` |
| `--gaze-accent-text` | text on an accent background | `#10272c` | `#ffffff` |
| `--gaze-hover` | hover and active-row background | `#344156` | `#dde9ed` |
| `--gaze-warning` | prompts, warnings | `#efbf75` | `#955300` |
| `--gaze-success` | "Allowed", success messages | `#7ddc9a` | `#146c43` |
| `--gaze-danger` | errors, destructive actions | `#ff8a80` | `#b42318` |

Built-in pages (`gaze://newtab`, `gaze://about`, error pages) use the same
tokens through `theme::builtin_css`. The chrome installs that sheet as a
user-agent style sheet whose rules are `!important`, so it overrides the
pages' own rules whatever their specificity (§11). A page state that changes a
colour the sheet sets must therefore be styled in the sheet too. The one such
state is the new-tab page's lit lamp (`button.lit`, §8.14). Its colours are
the same in both schemes: the page's yellow `#ffcf3f`, a `#1d2330` label, and
a `#a37f00` edge (`theme::LAMP_LIT_FILL`, `LAMP_LIT_LABEL`, `LAMP_LIT_EDGE`;
ledger L11).

### 4.2 Contrast

WCAG 2.1 defines the **relative luminance** $`L`$ of an sRGB colour with 8-bit
channels $`R_8, G_8, B_8`$ [W3C-WCAG21, "relative luminance"]. Each channel
$`C_8`$ is linearised and the three are weighted:

```math
c = \frac{C_8}{255}, \qquad
C_{\mathrm{lin}} =
\begin{cases}
\dfrac{c}{12.92} & c \le 0.04045\\[6pt]
\left(\dfrac{c + 0.055}{1.055}\right)^{2.4} & c > 0.04045
\end{cases}
\qquad
L = 0.2126\,R_{\mathrm{lin}} + 0.7152\,G_{\mathrm{lin}} + 0.0722\,B_{\mathrm{lin}}
```

The **contrast ratio** of two colours, where $`L_1`$ is the lighter one's
luminance, is

```math
\mathrm{CR} = \frac{L_1 + 0.05}{L_2 + 0.05} \in [1, 21].
```

The chrome's rules:
- Text, including coloured text such as danger labels, needs
  $`\mathrm{CR} \ge 4.5`$ (WCAG 1.4.3, level AA).
- Bars, focus rings, and icons that carry meaning need $`\mathrm{CR} \ge 3`$
  against their background (WCAG 1.4.11).
- Disabled controls are exempt (WCAG 1.4.3). They use `--gaze-border` on
  purpose, so they recede.

Measured ratios against each background a token is drawn on:

| Foreground | Dark: bg / surface / raised / hover | Light: bg / surface / raised / hover |
|---|---|---|
| text | 15.02 / 13.05 / 11.20 / 8.99 | 14.19 / 15.23 / 13.25 / 12.30 |
| muted | 8.44 / 7.33 / 6.29 / 5.05 | 5.41 / 5.81 / 5.05 / 4.69 |
| accent | 9.86 / 8.57 / 7.36 / 5.90 | **4.46** / 4.78 / **4.16** / **3.86** |
| warning | 10.16 / 8.82 / 7.58 / 6.08 | 5.57 / 5.97 / 5.20 / 4.82 |
| success | 10.32 / 8.96 / 7.70 / 6.18 | 6.01 / 6.45 / 5.61 / 5.21 |
| danger | 7.55 / 6.56 / 5.63 / 4.52 | 6.13 / 6.57 / 5.72 / 5.31 |

The light accent falls below 4.5:1 on three of the four backgrounds. That is
the reason for convention **R6**: accent *text* appears only on
`--gaze-surface`, and elsewhere the accent colours bars, rings, and icons,
where 3:1 suffices. Two pairings used on accent or danger backgrounds:
- `--gaze-accent-text` on accent: 8.91 (dark), 4.78 (light).
- `--gaze-bg` on danger, for the primary danger button: 7.55 (dark), 6.13
  (light).

The lit lamp (§8.14) keeps its own colours in both schemes:
- Its label on its yellow: 10.67. The scheme's dark-mode text would give 1.28
  there, so the lit rule sets the label too.
- Its edge on the page background: 4.59 (dark), 3.50 (light). The yellow
  itself is only 1.37:1 against the light background, so on the light page
  the edge outlines the lit lamp (WCAG 1.4.11). The page's own amber edge,
  `#c79a00`, gives 2.43 there.

The test `the_lit_lamp_is_legible_in_both_schemes` asserts the label at
4.5:1 or more, and the edge at 3:1 or more against both backgrounds.

The test `palettes_are_legible` asserts, for both schemes:
- text and muted text on the surface, at least 4.5:1;
- accent-text on the accent, at least 4.5:1;
- success and danger on all four backgrounds, at least 4.5:1;
- the accent on all four backgrounds, at least 3:1;
- the accent on the surface, at least 4.5:1;
- bg on danger, at least 4.5:1.

### 4.3 Themes

The theme is a setting, `theme` under `[appearance]` in `settings.toml`:
- **`system`**, the default, follows the operating system's light or dark
  preference, and is dark without one ([`docs/storage/README.md`](../storage/README.md),
  section 12);
- **`default-dark`** and **`default-light`** are the built-in schemes;
- **any other name** is a theme file, `themes/<name>.css`, in the settings
  folder or one installed for everyone; a user's file hides an installed one
  of the same name. Appearance lists them all (§8.13). **New theme** saves
  the colours shown as a new file (`my-theme.css`, then `my-theme-2.css`, …;
  never over anything) and chooses it, and **Reload** reads the files again
  after editing.

A theme file is a restricted CSS subset: an optional `:root { … }` holding
`--gaze-*: #RRGGBB` declarations, and comments (the grammar is in
`docs/storage/README.md`, section 6.2). `parse_palette` accepts it only if:
1. every declaration is a known token with a `#RRGGBB` value;
2. text and muted text reach 4.5:1 on the surface;
3. accent-text reaches 4.5:1 on the accent;
4. any of the two newest tokens (`--gaze-success`, `--gaze-danger`) present
   reaches 4.5:1 on the surface.

A theme that leaves tokens out takes them from the built-in scheme it
resembles: dark or light by its background's luminance (`scheme_of`,
`palette_with`). A theme that cannot be used is reported, and the scheme
the system prefers (or dark) is shown instead: Appearance says so at the
top of the panel and in the file's row, and the status bar at start-up.

A built-in scheme is drawn in its own colours whatever theme files exist.
Before the switch to the five roots, a valid old `palette.css` was laid
over Dark and Light too (ledger L14).

Pages see the scheme shown in `prefers-color-scheme` (§3.6, ledger L13). A
theme file's scheme, for pages and for the window's decorations, is the one
its background classifies it as (`scheme_of`).

---

## 5. Type, sizes, and spacing

**Fonts.** The fonts are bundled, so the chrome looks the same everywhere:
- Noto Sans for text.
- Fira Code for addresses, keys, and console text.
- Font Awesome Free 7 for icons.

Each face is registered under the family name the stylesheet asks for
(`theme::BUNDLED_FONTS`), not the one in its own name table. The vendored
Fira Code is a variable font that names itself "Fira Code Light". Until ledger
L10, a lookup of `'Fira Code'` never found it, and each platform drew
monospace text in a face of its own.

| Use | Face | Size |
|---|---|---|
| Labels, row titles, tab titles | Noto Sans 400 | 13 px |
| Card titles | Noto Sans 600 | 13 px |
| Panel title | Noto Sans 600 | 14 px |
| Status bar, small buttons | Noto Sans 400 | 11.5 px |
| Row meta (times, sizes) | Noto Sans 400 | 11 px |
| Tags and counts | Noto Sans 600 | 10.5–11 px |
| Row details (addresses) | Fira Code | 11 px |
| Wallet addresses, suggestion URLs | Fira Code | 11.5 px |

**Sizes.**

| Element | Size |
|---|---|
| Tab strip / toolbar / status bar | 38 / 46 / 24 px high |
| Rail | 42 px wide, 34 px buttons |
| Sidebar | 300 px wide |
| Inputs | 30 px high |
| `.btn` / `-sm` / `-xs` | 28 / 24 / 20 px high |
| Radii (`--radius-sm/md/lg`) | 5 / 7 / 10 px |

**The sidebar grid (R7).** The panel has one gutter of 6 px. **Boxes** span it
edge to edge: rows, cards, notices, the controls bar. **Text** that is not
inside a box starts 8 px further in: headings, field labels, footnotes, and
in-flow controls such as the segmented scheme control and the wallet form
(`.form`). With the rail 42 px wide, box edges fall at x = 48 and 335, and
text at x = 56, in every panel.

The constants in `chrome.rs`'s `geometry` module mirror these CSS lengths, so
labels can be fitted to their boxes' exact widths. The test
`text_budgets_match_laid_out_boxes` lays the chrome out with Blitz at 1280 px
and at 900 px. It checks every derived width to within 0.6 px: the row text
column, card contents, the address box, suggestion rows, and active and
inactive tab titles.

---

## 6. Conventions R1–R8

Each convention is commented at the top of the stylesheet (`CSS` in
`chrome.rs`). The table gives the rule, the engine fact behind it, and what
enforces it.

| | Rule | Why | Enforced by |
|---|---|---|---|
| **R1** | A label is an element (`<span>`), an icon an `<i>`. No bare text directly inside a flex or grid box. | Blitz wraps bare text in a flex box in an *anonymous block*. Its style is computed once and never restyled, so after a scheme switch the label keeps the old colour (ledger L2). | `labels_are_never_bare_text_in_flex_boxes`, `theme_switch_leaves_no_stale_label_colours`, `host_theme_swap_leaves_no_stale_label_colours` |
| **R2** | Every overlay has a positive z-index. Nothing persistent inside `#panel` or `#tabs` gets one. Ancestors of overlays get no `opacity`, `transform`, or `filter`. | Only z-index ≠ 0 hoists an element above the page sub-documents. A hoisted element ignores its ancestors' clips. | review; the layer table in §2.3 |
| **R3** | Frequent show/hide uses `visibility` (`.off`, `:hover`). `display:none` is only for `#sidebar`, `#panel`, `#wallet`, and the Send form when there is no wallet (`.hidden`), for regions with nothing in them (`:empty`), and for a narrow tab's title and close button, which are set when the strip is rebuilt. | `visibility:hidden` skips paint and hit-testing without relayout, and the element keeps its box. | review |
| **R4** | Inputs live only in static markup. Their state is set with `sync_attr` on stable ids. | Replacing an input loses its text, caret, and focus. | `side_controls_are_constant` |
| **R5** | Labels are fitted to their exact width in Rust; `overflow:hidden` is only a backstop. | Blitz has no `text-overflow: ellipsis`. | `text_budgets_match_laid_out_boxes`, `measured_width_matches_blitz_layout` |
| **R6** | Accent colours icons, bars, and rings. Accent *text* appears only on `--gaze-surface`. | The light accent is below 4.5:1 on bg, raised, and hover (§4.2). | `palettes_are_legible` |
| **R7** | One 6 px sidebar gutter for boxes; text starts 8 px further in (§5). | One left edge for boxes and one for text reads as order. | snapshots |
| **R8** | An edge bar is a solid image the size of the bar, `linear-gradient(c, c) 0 0 / 3px 100% no-repeat`, over the background colour. Bordered elements put it in the border box. | Blitz draws an inset box-shadow with a hairline along the other edges. Vello quantizes a hard gradient stop across the element to $`L/511`$ px (ledger L5 R1; §10.7). | snapshot checks `*_edge_artifact_px` and `*_bar_px` |

---

## 7. Components

| Component | Classes | Notes |
|---|---|---|
| Buttons | `.btn`, `-sm`, `-xs`, `-ghost`, `-primary`, `-danger` | A disabled button uses border-coloured text; a disabled ghost button stays borderless. |
| Icon buttons | `.icon-btn`, `-sm`, `.on` | Every icon button has a `.tip`. |
| Chips | `.chip`, `.chip.on` | Toggles such as Tree and Page text (`aria-pressed`). |
| Segmented control | `.segmented`, `.segment.on` | System, Dark, Light; the lit one has `aria-pressed="true"`. |
| Checkbox | `.check`, `.check-box` | Drawn in CSS ("Remember for this site"). |
| Badges | `.tag` (`ok`, `warn`, `err`), `.count` | Never flex boxes, because they hold bare text (R1). |
| Hover labels | `.tip`, `-right`, `-below`, `-below-end` | `:hover > .tip { visibility: visible }`; hidden under `:disabled`. |
| Fields | `.field`, `.field-icon`, `.ghost-hint` (`.off`), `.field-label`, `.form`, `.form-cols`, `.form-row`, `.form-actions` | A ghost hint stands in for `placeholder`, which Blitz does not draw. |
| Lists | `.section-head`, `.row` (`.on`, `.menu-open`, `.static`), `.row-icon` (`.warn`, `.err`), `.row-text`, `.row-label`, `.row-detail`, `.row-desc` (`.why`), `.row-meta`, `.row-actions` | Row actions float over the row's end; they show on hover, on the active row, and when its menu is open. A reason (`.row-desc.why`) may hold a long path, so it wraps anywhere. |
| Inline menu | `.row-menu`, `.menu-item` (`.danger`), `.menu-sep` | Opens under its row in the panel, so it never covers the page. |
| Containers | `.card`, `.card-head`, `.facts`, `.kv`, `.notice` (`info`, `ok`, `warn`, `err`), `.empty-state`, `.footnote` | Inside a card, a notice is filled with `--gaze-surface`. |
| Search match | `mark` inside a row label, row detail, or suggestion | Text colour plus a 2 px accent underline; neither changes any width. |

---

## 8. The parts of the chrome, one by one

Each part lists its contents, its behaviour, and its snapshots. Before
captures are of the binary before this work; after captures are of the final
binary. Scene numbers refer to `scripts/ui-snapshots.sh --list`.

### 8.1 Tab strip

- **Each tab** shows an icon and a fitted title. The icon is the address's
  scheme, or an attention state:
  - hourglass while loading;
  - warning shield while the page waits for an answer;
  - danger triangle when the page failed to load.
- **The active tab** has an accent bar on top and a close button. Inactive tabs
  show their close button on hover, over the end of their title.
- **Sizes.** Tabs share the room up to 200 px each. When they get crowded, the
  active tab keeps 128 px and the others shrink to 44 px, where only their icon
  shows. Only when even that does not fit does a window of tabs around the
  active one appear, with a "+N" chip that opens the Tabs panel (§10.3).
- **Pointer.** Drag a tab onto another to reorder it; middle-click closes it;
  right-click opens its menu in the Tabs panel.
- **The new-tab button** sits outside the list, so it is never clipped.

Before: [`25-many-tabs`](../screenshots/ui/before/25-many-tabs.png).
After: [`25-many-tabs`](../screenshots/ui/after/25-many-tabs.png) (30 tabs),
[`01-tabs-dark`](../screenshots/ui/after/01-tabs-dark.png).

![The tab strip's layout](diagrams/strip-layout.svg)

### 8.2 Toolbar and address box

- **Buttons** (left to right): show or hide the sidebar (Ctrl+B), Back, Forward,
  Reload, the address box, Find. Back and Forward are disabled when there is no
  history in that direction.
- **Removed buttons.** Three were removed. Each stays in `TOOLBAR_HTML` as an
  HTML comment with the reason:
  - **Go**: Enter does it, and its arrow looked like Forward's.
  - **☰**: the rail's Tabs button does the same.
  - **Palette cycling**: Appearance shows and sets the scheme.
- **The badge** at the left of the address says what kind of address the page
  has, and clicking it opens the page's permissions:
  - lock: https;
  - "Not secure": http;
  - accent shard: `f1r3://`;
  - hash: `f1r3h://`;
  - file;
  - built-in.

  While the box holds anything but the page's address, an empty box included,
  the badge is a magnifier: Enter will search or open what was typed.
- **Suggestions.** Typing shows suggestions in a dropdown over the page. They
  are open tabs (tagged "Switch to tab", which switches instead of opening a
  copy) and history. The text that matched is highlighted (§10.2).
  - ↓ and ↑ move through the suggestions and back to the typed text.
  - Enter opens the highlighted suggestion or the typed address.
  - Esc puts the page's address back and selects it.
  - Leaving the box closes the dropdown.

Before: [`34-url-suggestions`](../screenshots/ui/before/34-url-suggestions.png).
After: [`33-url-empty`](../screenshots/ui/after/33-url-empty.png),
[`34-url-suggestions`](../screenshots/ui/after/34-url-suggestions.png),
[`35-url-suggestion-down`](../screenshots/ui/after/35-url-suggestion-down.png),
[`36-url-escape`](../screenshots/ui/after/36-url-escape.png),
[`38-url-switch-to-tab`](../screenshots/ui/after/38-url-switch-to-tab.png).

![The address box's states](diagrams/address-states.svg)

### 8.3 Find in page

- **The box.** Ctrl+F or the magnifier opens a floating box at the top right of
  the page. The page does not move. The box contains:
  - the field, with its hint inside;
  - a count, `3/12`, in danger colour for `0/0`;
  - previous and next buttons;
  - close.
- **Keys and highlights.**
  - Enter and Shift+Enter step through the matches.
  - The current match is selected (the page's own selection colour) and
    ringed; the others are tinted.
  - Esc or × closes the box and returns the keyboard to the page.
  - Reopening selects the old query, so typing replaces it.
- **Ownership.** Find only ever clears a selection it made.
- **Tab switches.** Switching tabs moves the search to the new tab.
- **Pages without a layout yet.** A page that has no layout when it is attached
  is searched once it has one (ledger L1, H7).

![Find: states and the invariant](diagrams/find-states.svg)

Before: [`30-find-cleared`](../screenshots/ui/before/30-find-cleared.png) (the
bug: one character stays highlighted).
After: [`28-find-matches`](../screenshots/ui/after/28-find-matches.png),
[`30-find-cleared`](../screenshots/ui/after/30-find-cleared.png),
[`31-find-tab-switch`](../screenshots/ui/after/31-find-tab-switch.png).

### 8.4 Prompt bar

When the active page asks for a capability, a bar opens under the toolbar. It
has a warning bar, a shield, and the text. The asking site is named short and
bold, with the full origin in its hover label. Origins in the text drop their
default port (§10.6).

The controls: "1 of N" when several questions wait, **Remember for this site**,
Deny, and a primary **Allow**. "Remember" applies to one answer and is cleared
after it (ledger L6, X1).

Before: [`39-prompt-net`](../screenshots/ui/before/39-prompt-net.png).
After: [`39-prompt-net`](../screenshots/ui/after/39-prompt-net.png),
[`40-prompt-shard`](../screenshots/ui/after/40-prompt-shard.png).

### 8.5 Status bar

One line, in this order:
1. the stage chip: Loading, Verifying, Waiting for your answer, Running
   f1r3lang, Static page, or Couldn't load;
2. the reason, only for a failed load;
3. the page's notice;
4. the step-budget tag and **Let it run**;
5. on the right, a message that expires: 4 s for information and success, 8 s
   for warnings and errors. Entering full screen shows how to leave it
   ("Press F11 to leave full screen"; on macOS, Ctrl+Cmd+F), and leaving it
   takes the message away.

Everything is fitted to the window's width (`the_status_bar_fits_on_one_line`).

After: [`47-flash-shown`](../screenshots/ui/after/47-flash-shown.png),
[`48-flash-expired`](../screenshots/ui/after/48-flash-expired.png).

### 8.6 Rail and sidebar

- **The rail** holds Tabs, History, Site data, Permissions, Wallet, Console,
  and, at the bottom, Appearance. Each shows its name in a hover label, with its key where it has one
  (History · Ctrl+H).
  The open panel's button is accented, with a bar on its left edge. Clicking it
  again hides the sidebar.
- **The sidebar header** shows the panel's title, a count, and Hide.
- **The controls bar** under the header is constant per panel. Switching panels
  or reopening the sidebar starts with an empty search (ledger L5 and S9).
- **At startup** every window opens with the sidebar collapsed, whatever the
  last window left. The panel is still restored from `state/session.json`,
  so Ctrl+B or the toolbar's sidebar button reopens the panel last shown, and
  a rail button opens its own. `restore_sidebar = true` under
  `[appearance]` in `settings.toml` reopens the sidebar as the last window
  left it. The snapshot harness sets
  it, because every scene seeds its own sidebar
  (`ChromeDocument::new`; tests `the_sidebar_starts_collapsed`,
  `restore_sidebar_reopens_it`).

After: [`45-rail-hover`](../screenshots/ui/after/45-rail-hover.png),
[`19-collapsed-dark`](../screenshots/ui/after/19-collapsed-dark.png),
[`49-search-reset`](../screenshots/ui/after/49-search-reset.png).

### 8.7 Tabs panel

- **Rows.** Each tab is one row: icon, fitted title, and fitted address. The
  active row has an accent bar.
- **Row actions.** ⋯ (menu) and × appear on hover and on the active row.
- **Tree.** Nests tabs under the tab that opened them.
- **Search** (title and address, with small typos) highlights the matched
  text. A row that matched only in its address shows a window of the address
  around the match (§10.2).
- **Page text** also searches the text of loaded pages. A row then shows
  "N matches in page text".
- **The menu** opens under its row in three groups. Items that do not apply
  are disabled, and the menu closes after any action:
  - Duplicate, Copy address;
  - Move up, Move down;
  - Close other tabs, Close tabs below, Close tab and its children (only when
    it has children), Close tab.
- **Recently closed** lists the last five closed tabs, newest first.

Before: [`01-tabs-dark`](../screenshots/ui/before/01-tabs-dark.png),
[`21-tab-menu`](../screenshots/ui/before/21-tab-menu.png).
After: [`01-tabs-dark`](../screenshots/ui/after/01-tabs-dark.png),
[`21-tab-menu`](../screenshots/ui/after/21-tab-menu.png),
[`22-tab-menu-after-action`](../screenshots/ui/after/22-tab-menu-after-action.png),
[`23-tabs-search`](../screenshots/ui/after/23-tabs-search.png),
[`24-tabs-tree`](../screenshots/ui/after/24-tabs-tree.png),
[`46-row-hover`](../screenshots/ui/after/46-row-hover.png).

### 8.8 History

- **Groups.** Visits are grouped by how long ago they were: last hour, last 24
  hours, last 7 days, last 30 days, older. The buckets use elapsed time, which
  needs no time zone (§10.4).
- **Entries.** Each group lists an address once (its newest visit), with a
  relative time and, on hover, × to forget it.
- **Counts.** The header counts entries; a search reads "N of M".
- **Pages.** The list shows 200 entries at a time ("Show 200 more").
- **Clear…** asks first, "Clear all M entries from history?", whatever the
  search.

Before: [`03-history-dark`](../screenshots/ui/before/03-history-dark.png).
After: [`03-history-dark`](../screenshots/ui/after/03-history-dark.png),
[`26-history-search`](../screenshots/ui/after/26-history-search.png),
[`51-history-clear`](../screenshots/ui/after/51-history-clear.png).

### 8.9 Site data

- **Sites.** Only sites that keep something are listed: stored data,
  remembered permissions, or a live shard session. Each is a card with:
  - its short name and address;
  - facts in human units (zeros omitted);
  - Clear stored data, Forget permissions, and an End button for each live
    session.

  A session ends on the tab that owns it (ledger L6, X2).
- **The content cache card** follows. Its Clear button is disabled when the
  cache is empty.

After: [`05-sites-dark`](../screenshots/ui/after/05-sites-dark.png).

### 8.10 Permissions

- **Header.** The active page's site.
- **This page can use.** One row per capability: icon, name, a sentence on what
  it allows, an Allowed or Off tag, and Revoke.
- **Remembered for this site.** Choices kept by "Remember", each with Forget.
- **Empty states** explain built-in pages, static pages, and pages still
  waiting for an answer.

After: [`07-grants-dark`](../screenshots/ui/after/07-grants-dark.png).

### 8.11 Wallet

- **The paying wallet's card**: label, a "Pays for deploys" tag, the address
  (shortened in the middle, since people compare both ends), the balance with
  grouped digits, and Copy, Export, Remove.
- **Send**: labelled fields (Recipient address; Amount and Note) and a primary
  **Review transfer**. The review shows From, To, Amount, and Note, with Cancel
  and **Send N**. It is refused if the paying wallet changed in between.
- **Other wallets**, each with Use for payments.
- **Add a wallet**: Create new wallet, or import an F1R3Sky wallet file or hex
  key.
- **Notices.** A missing Embers configuration is explained in a notice.
- **Balances** load at start when the profile reopens on this panel (ledger
  L6, X6).

Before: [`11-wallet-one-dark`](../screenshots/ui/before/11-wallet-one-dark.png),
[`50-wallet-review`](../screenshots/ui/before/50-wallet-review.png).
After: [`09-wallet-empty-dark`](../screenshots/ui/after/09-wallet-empty-dark.png),
[`11-wallet-one-dark`](../screenshots/ui/after/11-wallet-one-dark.png),
[`50-wallet-review`](../screenshots/ui/after/50-wallet-review.png).

**After a switch to light with the Wallet open.**
- Before: [`60-theme-wallet-to-light`](../screenshots/ui/before/60-theme-wallet-to-light.png)
  shows the form's labels vanishing (ledger L2).
- After: [`60-theme-wallet-to-light`](../screenshots/ui/after/60-theme-wallet-to-light.png)
  is pixel-identical to a profile that starts in light.

### 8.12 Console

The newest messages come first: up to 200 shown, and 1000 kept per tab. Each
line has its level and its text. A single string literal is shown without
quotes. Warnings and errors carry a coloured bar and level. The controls are
Save replay log and Clear.

After: [`13-console-dark`](../screenshots/ui/after/13-console-dark.png).

### 8.13 Appearance

From top to bottom (ledger S12, part 3):
1. when the chosen theme file cannot be used, an error notice that names the
   file, says why, and says which scheme is shown instead;
2. the scheme as a segmented control: **System**, **Dark**, **Light**. A
   theme file lights none;
3. one sentence. With System it first says what the system prefers ("Follows
   your system, which prefers dark."), or that the system states no
   preference, or that its preference could not be read, so dark is used.
   Then what the scheme colours: the browser's controls, its built-in
   pages, and sites that offer light and dark styles (§3.6);
4. **Themes**, with their count: every theme file, the user's and those
   installed for everyone (a user's file hides an installed one of the same
   name), by name. A usable file's row can be chosen (`theme:use:<name>`),
   shows its path and its scheme as a tag (Dark or Light), and is lit when
   chosen. A file that cannot be used shows why, with an **Unusable** tag,
   and does nothing. With no file: "No theme files yet.";
5. swatches of the colours shown;
6. **Theme files**: the user's themes folder, a note on what a theme file
   is, and **New theme** and **Reload**.

The list is read when the panel is shown, when the sidebar reopens on it,
by New theme and by Reload; never at each render. New theme and Reload say
what they did in the status bar: "Theme created at …", "Themes reloaded",
or why the chosen theme cannot be used. A session that only reads refuses
New theme, saying why. The theme files' format is in
[`docs/storage/README.md`](../storage/README.md), section 6.2.

Scenes 15–18 and 72–82 of the snapshot harness (§12.2) show the panel. The
committed captures, [`15-appearance-dark`](../screenshots/ui/after/15-appearance-dark.png),
[`17-appearance-bad-palette`](../screenshots/ui/after/17-appearance-bad-palette.png) and
[`18-appearance-low-contrast`](../screenshots/ui/after/18-appearance-low-contrast.png),
still show the panel as it was before step 10, with Dark, Light and Custom,
until they are regenerated.

### 8.14 Built-in pages

**New tab.** The address box is empty, with its hint. The page explains the
address forms and runs a f1r3lang program, the lamp, whose one capability is
the document:
1. It asks the document for `#lamp` and listens for the button's clicks.
2. A single token, on `off` or on `on`, holds the lamp's state.
3. Each click adds the class `lit` and moves the token to `on`, or removes the
   class and moves it back to `off`.

So each click switches the lamp between unlit, in the scheme's button colours,
and lit, in yellow. The host theme styles both states (§4.1). Before ledger
L11 it styled only the unlit one, so the class changed and the lamp did not.
Snapshot scenes 69–71 click it (§12.2).

**Error page.** It is themed like the chrome. It shows:
- "Can't open this page";
- one sentence on what went wrong;
- the address once;
- **Try again**, which reloads instead of adding a history entry;
- the technical reason under Details.

Before: [`41-error-page`](../screenshots/ui/before/41-error-page.png).
After: [`41-error-page`](../screenshots/ui/after/41-error-page.png),
[`42-newtab`](../screenshots/ui/after/42-newtab.png).

Buttons on built-in pages, such as the new-tab page's lamp, show the hand
(`button{cursor:pointer}`), as the chrome's own buttons do. Links get the hand
from Blitz (§3.4).

---

## 9. Keyboard

Ctrl means Ctrl on Linux and Windows, and Cmd on macOS (`KeyPlatform::command`).
Blitz reports Cmd as the `SUPER` modifier, so that is the one the macOS
bindings test (ledger L12). Ctrl+Tab and Ctrl+Shift+Tab keep Ctrl on every
platform: on macOS, Cmd+Tab is the system's application switcher. The hover
tips name each key as the platform has it (`KEY_LABELS`; ledger L17).

| Keys | Where | Action |
|---|---|---|
| Ctrl+T, Ctrl+W, Ctrl+Shift+T | anywhere | new tab, close tab, reopen the last closed tab |
| Ctrl+Tab, Ctrl+Shift+Tab (Ctrl on every platform) | anywhere | next and previous tab |
| Ctrl+1…8, Ctrl+9 | anywhere | tab N, last tab |
| Ctrl+L | anywhere | focus the address box, its text selected |
| Ctrl+F | anywhere | open find, its query selected |
| Ctrl+R | anywhere | reload |
| Ctrl+H (Cmd+Y on macOS) | anywhere | History panel (again: hide the sidebar). On macOS the system's menu takes Cmd+H for Hide before the window sees it, so History is Cmd+Y there, as in Safari and Chrome (ledger L16) |
| Ctrl+B | anywhere | show or hide the sidebar |
| F11 (Ctrl+Cmd+F on macOS) | anywhere | enter or leave full screen; the status bar says how to leave. Checked before Find, and a chord held down toggles once |
| Ctrl+=, Ctrl+−, Ctrl+0 (Blitz's own) | anywhere | zoom the window by a tenth, from 25 % to 500 %, or back to 100 %; kept in `window.json` (ledger L15) |
| ↓ ↑ | address box, dropdown open | move through the suggestions and back to the text |
| Enter | address box | open the highlighted suggestion or what was typed |
| Esc | address box | put the page's address back and select it |
| Enter, Shift+Enter | find box | next and previous match |
| Esc | find box | close find |
| Esc | sidebar search | clear the search |
| Esc | elsewhere | close, in order: the dropdown, a tab menu, the history confirmation, a wallet confirmation (the page also receives the key) |

The shortcuts are handled before Blitz routes the key, so they work while a page
has the focus. On macOS, standard key bindings bypass Blitz's handlers (for
example, the arrow keys in a text field). The chrome's field keys are checked
before Blitz for the same reason.

---

## 10. Algorithms

The algorithms are given in literate form [Knuth 1984]: prose explains each
step, and pseudocode **chunks** named ⟨like this⟩ are defined with ≡ and used by
name.

### 10.1 Exact text fitting

*Problem.* Shorten a label so that it fits a width $`w`$ exactly as Blitz will
lay it out, keeping as much as possible, with "…" where text was removed.

*Measurement.* `TextFitter::width` lays the text out with parley, the engine
Blitz uses, under the same conditions:
- the same bundled fonts;
- the font family list, size, and weight from the stylesheet;
- collapsed white space;
- advances rounded to device pixels at the window's scale.

The test `measured_width_matches_blitz_layout` checks the result against
Blitz's own layout to within 1 px. Budgets keep 1 px spare (`ROUNDING`)
because Blitz rounds box sizes.

*What a fit keeps.* Every cut is one of four shapes, so a range of the original
text, such as a search match, can be located in what is shown:

```text
Kept = All                      the text fits
     | Ends { head, tail }      text[..head] + "…" + text[tail..]
     | Window { start, end }    ("…" if start > 0) + text[start..end] + ("…" if end < len)
     | Nothing                  not even "…" fits
```

```text
⟨fit text to w with cut⟩ ≡
    if text is empty → All;   if w ≤ 0 → Nothing
    key ← (font, cut, round(10·w), text)            -- tenths of a pixel
    if memo has key → memo[key]
    kept ← if width(text) ≤ w then All else ⟨cut text to w⟩
    if memo holds 4096 entries → clear it
    memo[key] ← kept;  return kept
```

Each cut searches for the largest $`k`$, a number of characters kept, whose
candidate fits. Fitting is monotone in $`k`$ for the end, middle, and start
cuts (more characters are never narrower). So a binary search over $`k`$ costs
$`O(\log n)`$ layouts for a text of $`n`$ characters:

```text
⟨largest k ≤ limit such that fits(k)⟩ ≡
    if not fits(0) → none
    low ← 0;  high ← limit
    while low < high:
        mid ← low + ⌈(high − low) / 2⌉
        if fits(mid) then low ← mid else high ← mid − 1
    return low                     -- always fits, even if fits is not quite monotone
```

The cuts:
- **End** keeps a prefix: `head = trim_end(text[..bounds[k]])`.
- **Middle** keeps ⌈k/2⌉ characters at the head and ⌊k/2⌋ at the tail. It is
  used for wallet addresses.
- **Start** keeps a suffix. It is used for host names, so the registrable
  domain survives.
- **URL** keeps `scheme://authority` whole and the longest path tail. The tail
  is moved to start at a `/` when that drops at most 8 characters
  (`file://…/site/notes.html`). If the authority alone would take over 60 % of
  the width, the URL is cut in the middle instead.

### 10.2 Search matches that stay visible

*Problem.* A row passes the search filter, but the cut hides the text that
matched. The user cannot tell why it is there (ledger L5 R9).

*The match.* `match_span(query, text)` applies the same rules as the filter
`search_score`. It returns the first case-insensitive occurrence; failing that,
the first word within the typo bound. That bound is one edit for queries
shorter than 6 characters and two up to 48, by the bounded Levenshtein
distance [Levenshtein 1966; Ukkonen 1985]. Queries with spaces or only digits
match exactly only. The test `a_row_is_highlighted_exactly_when_it_matches`
checks that a row is highlighted exactly when the filter accepts it.

*Case without losing positions.* The filter compares
`text.to_lowercase().contains(query)`. To highlight the right characters, the
match found in the lowercase string must be mapped back to the original.

Rust's `str::to_lowercase` maps each character with `char::to_lowercase`, with
one exception. A final capital sigma becomes ς, still a single character
[Unicode §3.13]. The lowercase string therefore aligns with the original
character by character. Each original character contributes
`c.to_lowercase().count()` characters; `İ` contributes two.

```text
⟨lowercase text, remembering where each byte came from⟩ ≡
    lower ← text.to_lowercase();  origins ← []
    for each character c of text, at bytes [s, e):
        repeat c.to_lowercase().count() times:
            take the next character l of lower
            push [s, e) to origins, once per byte of l
    return (lower, origins)

⟨find query in text, ignoring case⟩ ≡
    (lower, origins) ← ⟨lowercase text, remembering…⟩
    at ← position of query in lower, or none
    return origins[at].start .. origins[at + len(query) − 1].end
```

*Fitting around the match.* The fitted labels are produced as follows:

![Fitting a search result](diagrams/marked-fit.svg)

```text
⟨window of text around [m₀, m₁) that fits w⟩ ≡
    first, last ← character indices of m₀ and m₁
    candidate(k) ≡ right ← min(⌊k/2⌋, room right of last)
                   left  ← min(k − right, room left of first)
                   right ← min(k − left, room right of last)   -- an exhausted side gives way
                   Window { start: bounds[first − left], end: bounds[last + right] }
    if ⟨largest k such that candidate(k) fits⟩ exists → that candidate
    else → the longest Window starting at m₀ that fits (the match alone is too wide)
```

The label keeps its match in view. The address keeps its usual form when the
label already shows a match (`fit_detail`), so a row matched by its title
still reads `file://…/site/notes.html`.

The match is wrapped in `<mark>` and styled with colour and an underline. Bold
would change the width that was fitted.

### 10.3 Tab strip layout

Let $`W`$ be the window's width, $`n`$ the number of tabs, and every length in
px. The strip's padding, the gap, and the new-tab button leave

```math
R = W - 2 \cdot 8 - 4 - 30
```

for the tabs, which are 2 px apart. The equal share of $`n`$ tabs is

```math
e(n, R) = \min\!\left(200,\ \frac{R - 2(n-1)}{n}\right).
```

```text
⟨strip layout of n tabs, the active one at index i, in room R⟩ ≡
    if e(n, R) ≥ 128:   all n tabs, each e(n, R) wide
    a ← min(128, R);  o ← (R − a − 2(n − 1)) / (n − 1)
    if n ≤ 1 or o ≥ 44:  all n tabs; the active one a, the others o wide
    R′ ← R − 44 − 2                                  -- room for the "+N" chip
    k ← 1 + ⌊(R′ − 128) / 46⌋                         -- tabs that fit at the minimums
    [s, t) ← visible_window(n, i, k)
    the tabs [s, t), split as above in R′, and a chip "+(n − k)"

⟨visible_window(n, i, k)⟩ ≡
    if k ≥ n → [0, n)
    s ← min(max(0, i − ⌊k/2⌋), n − k);  return [s, s + k)
```

**Properties.** The test `the_strip_keeps_the_active_tab_readable_and_shrinks_the_rest`
checks them for windows of 320, 400, 900, 1280, 1920, and 3840 px, $`n`$ from 1
to 60, and the active tab first, middle, and last:

1. **The active tab is shown:** $`s \le i < t`$.
2. **No tab is wider than 200 px**, and the active one is never narrower than
   the others: $`a \ge o`$.
3. **When at least two tabs show, each is at least 44 px:** $`o \ge 44`$.
4. **The tabs and the chip fit:**
   $`a + (k-1)(o + 2) + [k < n]\,(44 + 2) \le R`$.
5. **A window is as large as possible:** $`128 + k\,(44 + 2) > R'`$.

**Why property 3 holds after windowing.** Let $`k`$ be as defined above and
$`k \ge 2`$. Then $`128 + (k-1)\cdot 46 \le R'`$, which rearranges to

```math
o = \frac{R' - 128 - 2(k-1)}{k-1} \ \ge\ \frac{46(k-1) - 2(k-1)}{k-1} = 44.
```

*Titles.* The active tab fits its title to $`a - 66`$:
- padding 10 + 6;
- borders 2;
- icon 14;
- two 7 px gaps;
- the 20 px close button.

The others fit theirs to $`o - 39`$, because their close button overlays the
title on hover instead of taking room. If nothing of a title fits (the fit is
empty or a bare "…"), the tab shows only its icon, centred (`.tab.narrow`).

At 1280 px: 6 tabs are 200 px each; 16 tabs are 128 px and 71.5 px; 24 tabs are
128 px and 45.9 px (icons only); 30 tabs show 23, with "+7".

### 10.4 History groups

A visit $`\Delta`$ seconds old falls in the first bucket whose bound exceeds
$`\Delta`$: one hour, one day, 7 days, 30 days, or Older. Within a bucket, an
address appears once, with its newest visit:

```text
⟨history entries for query q⟩ ≡
    seen ← ∅;  entries ← []
    for each visit v, newest first, that matches q:
        b ← bucket(now − v.at)
        if (b, v.url) ∉ seen: add it to seen; append (b, v) to entries
    total ← the same count with no query        -- the header's "M"
    show entries[..limit], grouped by b, each group headed by its count
```

### 10.5 Relative times and dates

`relative_time(now, at)` reads:
- "just now" under 45 s (and for times in the future);
- "N min ago" (at least 1) under an hour;
- "N h ago", "N d ago", "N wk ago" up to 30 days;
- after that, the UTC date `YYYY-MM-DD`.

Dates come from Howard Hinnant's `civil_from_days` [Hinnant]. It counts days
in 400-year eras of 146 097 days, and starts each year on 1 March, so the leap
day falls at the end of the year. For day number $`z`$ since 1970-01-01, with
divisions rounding down:

```math
\begin{aligned}
z' &= z + 719468, \quad \mathit{era} = \lfloor z'/146097 \rfloor, \quad \mathit{doe} = z' - 146097\,\mathit{era}\\
\mathit{yoe} &= \left\lfloor \frac{\mathit{doe} - \lfloor \mathit{doe}/1460 \rfloor + \lfloor \mathit{doe}/36524 \rfloor - \lfloor \mathit{doe}/146096 \rfloor}{365} \right\rfloor\\
\mathit{doy} &= \mathit{doe} - (365\,\mathit{yoe} + \lfloor \mathit{yoe}/4 \rfloor - \lfloor \mathit{yoe}/100 \rfloor), \quad
\mathit{mp} = \lfloor (5\,\mathit{doy} + 2)/153 \rfloor\\
d &= \mathit{doy} - \lfloor (153\,\mathit{mp} + 2)/5 \rfloor + 1, \quad
m = \mathit{mp} + 3 \text{ if } \mathit{mp} < 10 \text{ else } \mathit{mp} - 9, \quad
y = \mathit{yoe} + 400\,\mathit{era} + [m \le 2]
\end{aligned}
```

The symbols are:
- $`\mathit{doe}`$: the day of the era;
- $`\mathit{yoe}`$: the year of the era;
- $`\mathit{doy}`$: the day of the year, counted from 1 March;
- $`\mathit{mp}`$: the month counted from March.

The test `civil_dates_match_known_days` checks 1970-01-01, 2000-02-29 (a leap
day in a century year divisible by 400), 2023-11-14, and 2100-03-01 (2100 is
not a leap year).

### 10.6 Default ports in prompts

The broker names sites canonically (`https://example.org:443`), which is exact
but noisier than people expect. `display::without_default_ports` removes the
port only where it is the scheme's default:

```text
⟨drop default ports from text⟩ ≡
    for each "://" in text:
        scheme    ← the [A-Za-z0-9+.-] run before it
        authority ← the run after it of [A-Za-z0-9.-[]:@%_~], without trailing dots
        if (scheme, authority's end) is (https, ":443") or (http, ":80"),
           and something precedes the port:
            remove the port
```

Other ports, other schemes, and IPv6 literals such as `[::1]` are kept as they
are (`default_ports_are_dropped_from_origins_in_text`).

### 10.7 Bars that are exactly 3 px

Vello samples a gradient at integer pixel corners. It looks the colour up in a
512-entry ramp at index $`\mathrm{round}(511\,t)`$, where $`t`$ is the position
along the gradient divided by its length $`L`$
(`vello_shaders/shader/fine.wgsl`). A hard stop at $`s`$ px therefore moves to
the nearest ramp step, a multiple of $`L/511`$ px. The pixel at distance $`k`$
takes the bar's colour when

```math
\frac{1}{511}\,\mathrm{round}\!\left(\frac{511\,k}{L}\right) < \frac{s}{L}.
```

With $`s = 3`$, a 287 px row came out 4 px, a 261 px notice 3 px, and a 34 px
rail button 4 px. On the 1280 px prompt bar the step is 2.5 px.

Convention R8 draws the bar as a *solid* image the size of the bar: a gradient
of one colour, `0 0 / 3px 100% no-repeat`. A constant ramp has no stop to
quantize, and the bar's edge is the image's rectangle, at an exact pixel
boundary. The harness checks that every bar is exactly its width (§12.2).

---

## 11. Engine facts the design relies on

All facts are for Blitz `674d7d2`, parley `332c1c7`, Vello 0.10,
anyrender_vello 0.14.0 and winit 0.31.0-beta.3, and were verified in their
sources.

| Fact | Consequence in the chrome | Source |
|---|---|---|
| No rendering of `placeholder`, `text-overflow`, `title` tooltips, `:focus-within`, `content: attr()`. A `<label for>` click does not focus the input. | Ghost hints, exact fitting (R5), `.tip` labels, `#urlwrap.focus` set from Rust. | Blitz source review |
| winit reports the system's light or dark preference only on macOS and Windows (`ActiveEventLoop::system_theme`, `WindowEvent::ThemeChanged`); on X11 and Wayland it reports nothing. | The chrome asks the XDG desktop portal on Linux (`system_theme.rs`, through `dbus-send`), again when the window gains the focus; `application.rs` hands the window's reports to the chrome elsewhere. | winit-core 0.31.0-beta.3 `event_loop/mod.rs:177`, `event.rs:484` |
| A document's `prefers-color-scheme` is its viewport's `color_scheme`, light by default. Changing it restyles the whole document once. | The chrome sets the scheme shown on its own viewport (§3.6, L13). | blitz-traits `shell.rs:78-90`; blitz-dom `stylo_device.rs:38-56`, `document.rs:2072`; stylo 0.22.0 `servo/media_features.rs:66` |
| The parent's `resolve` copies its viewport's `color_scheme` into every sub-document's viewport, hidden ones included. | Pages follow the chrome's scheme at its next layout; nothing is set on a page (L13). | `resolve.rs:150` |
| `View::init` sets the document's scheme from `Window::theme()` (unknown, so light, on X11 and Wayland), and `ThemeChanged` from the system's, unless the view has a theme override. | `ChromeApplication` sets the override to the scheme shown (§3.6, L13). | `blitz-shell/src/window.rs:181, 268-272, 633-637` |
| Stylo gives `color-scheme: normal` the preferred scheme's system colours. Blitz's style sheet colours `mark`, `dialog` and popovers with system colours and form controls with fixed ones, and Blitz paints no `Canvas` behind a page ([DioxusLabs/blitz#1078](https://github.com/DioxusLabs/blitz/issues/1078)) nor reads `<meta name="color-scheme">` ([#1077](https://github.com/DioxusLabs/blitz/issues/1077)). | Every page gets `:root{color-scheme:light}`, which its own `color-scheme` overrides (L13, H2). | stylo 0.22.0 `device/servo.rs:336-351`; blitz-dom `assets/default.css:703-704, 968-969, 1129-1130` |
| `set_theme(None)` means dark on X11 (`_GTK_THEME_VARIANT`); `theme()` and `system_theme()` are always `None` on X11 and Wayland. macOS stops reporting `ThemeChanged` while a window has an appearance of its own. Windows' `set_theme` leaves the window's creation preference as it was, and a settings change re-applies the system's theme to a window created without one. | `decoration_theme`, `FollowSystem`, and the scheme asked for again after every report (§3.6). | winit-x11 `window.rs:980-1004, 2288-2290`, `event_loop.rs:785-787`; winit-wayland `event_loop/mod.rs:741-743`; winit-appkit `window_delegate.rs:636-658`; winit-win32 `window.rs:1163-1169`, `event_loop.rs:2509-2520` |
| blitz-paint draws scrollbars only with its `scrollbars` feature, which nothing in F1R3Gaze enables. | No scrollbar follows the scheme: none is drawn. | `blitz-paint/Cargo.toml:23`; `cargo tree -e features -i blitz-paint` |
| `View::init` makes the window hidden (`with_visible(false)`), shows it, and replaces the document's viewport (zoom 1, the window's theme). `BlitzApplication::add_window` and `can_create_surfaces` are public. | The window is made by Blitz's own code from `ChromeApplication`, and the zoom, scheme, maximize and full screen are set after `View::init` (§3.7). | `blitz-shell/src/window.rs:135-236`; `application.rs:33-35, 113-128` |
| Blitz's zoom keys add or take a tenth with no bound, and the viewport's logical size divides by the zoom. | The zoom is set back within $`[0.25, 5]`$ after every key (L15). | `blitz-shell/src/window.rs:646-666`; blitz-traits `shell.rs:118-124, 144-146` |
| X11: maximize and full screen are client messages that only a window manager carries out; full screen asked of a hidden window waits for it to be shown; `Moved` comes only from a window manager's synthetic ConfigureNotify; a window being mapped gets `Focused(false)`. | The window is read, not only watched (§3.7); a focus loss saves only after the window has had the focus. | winit-x11 `window.rs:1006-1058, 1151-1155, 1179-1198, 1312-1346`; `event_processor.rs:703-763, 879-890` |
| `with_position` places the frame's corner on X11 and Windows, the content's corner (in points) on macOS, and nothing on Wayland; `outer_position` is unsupported on Wayland. | `window_attributes` and `Sample::from_platform` use each platform's units (storage README §13). | winit-core `window.rs:237-261, 808-849`; winit-appkit `window_delegate.rs:714-731, 1284-1308` |
| winit-wayland's monitor scale is the whole-number `wl_output` scale; the compositor bounds a new window (`configure_bounds`). | No monitor clamp on Wayland (storage README §13). | winit-wayland `output.rs:53-56`, `window/state.rs:454-463, 534-551` |
| Windows keeps a theme given at creation for good; macOS's Quit closes the windows and drops the handler with no `CloseRequested`. | No creation theme on Windows (§3.6); `ChromeApplication`'s `Drop` writes the last reading. | winit-win32 `event_loop.rs:2508-2523`; winit-appkit `app_state.rs:188-192` |
| Bare text in a flex box becomes an anonymous block that is never restyled. | R1 (ledger L2). | `blitz-dom/src/layout/construct.rs`; `stylo.rs` |
| `add_user_agent_stylesheet` adds a sheet of the user-agent origin. Stylo compares importance and origin before specificity, and reverses the origin order for important declarations: an important user-agent declaration beats every author declaration ([CSS-Cascade-4] §6.1). | `builtin_css` overrides the built-in pages' own rules, so it styles their states too: `button.lit` (L11). | `blitz-dom/src/document.rs:1149-1153`; stylo 0.21.0 `rule_tree/level.rs:144-165` |
| Only a positioned element with z-index ≠ 0 is hoisted, above sub-documents, hit-tested first, unclipped. | R2 and the layer table. | `blitz-paint` `paint_tree.rs` |
| `visibility:hidden` skips paint and hit-testing. | R3 toggles. | `blitz-paint` |
| Click is dispatched before the Blur its default action causes. | Suggestions commit on Click; dismiss on Blur is safe. | `blitz-dom/src/events/pointer.rs` |
| A disabled button still receives Click. | The handler stops at `[disabled]`. | `pointer.rs` |
| Setting the `value` attribute calls `set_text`, which keeps the old selection clamped to the new text. | `select_whole` after a restore (ledger L5 R4). | `blitz-dom/src/mutator.rs`; `parley/src/editing/editor.rs:951` |
| `BaseDocument::with_text_input` exposes the editor's driver. | `select_byte_range(len, 0)`: all selected, caret at the start. | `blitz-dom/src/document.rs` |
| An inset box-shadow is a fill minus an analytic hole; a coincident edge keeps about 50 % of the colour. | R8 (ledger L5 R1). | `blitz-paint/src/render/box_shadow.rs:89-147` |
| Gradients are sampled at pixel corners through a 512-entry ramp. | R8 solid images (§10.7). | `vello_shaders-0.10.0/shader/fine.wgsl:26,1076,1225-1236` |
| On macOS, standard key bindings skip DOM handlers. | Field keys are handled before the event driver. | `blitz-shell` `driver.rs` |
| Hit-testing stops at a sub-document's host. A document asks its shell for a cursor only when its hovered element changes. | The cursor arbiter recomputes the cursor after a page has seen each event (§3.4). | `node/node.rs` `hit_inner`; `document.rs` `set_hover_to` |
| A parent reads a host's cursor before forwarding the event, from the sub-document's previous hover. `None` hides the cursor in blitz-shell. | `WindowShell` turns the chrome's cursor requests into "recompute". | `events/driver.rs`; `document.rs` `get_cursor`; `blitz-shell/src/lib.rs` |
| Sub-documents get no enter or leave events, and nothing clears their hover. | The arbiter clears a page's hover when the pointer leaves it. | `events/mod.rs` `map_dom_event_to_ui_event` |
| A document built without a shell provider gets `DummyShellProvider`. | Pages get `PageShell` through `Tab::config`. | `document.rs` `BaseDocument::new` |
| fontique registers a face under its name table's typographic family name, or else its family name. The vendored variable Fira Code names itself "Fira Code Light", after its default instance. When the requested weight differs from a variable font's `wght` default, fontique sets the axis to it. | `theme::BUNDLED_FONTS` registers each face under the stylesheet's name, and the `wght` axis gives Fira Code its weight (L10). | fontique `collection/mod.rs` `register_font_impl`; `font.rs` `FontInfo::synthesis` |
| `resolve` ends with `refresh_hover`, which re-resolves hover at the last pointer position. | A page told where the pointer is hovers the right element after its first layout. | `resolve.rs` |
| blitz-shell ignores a mouse `PointerLeft`, so a document goes on hovering where the pointer left the window. | `ChromeApplication` ends the chrome's and the page's hover when the mouse leaves (§3.5, L9 H8). | `blitz-shell/src/window.rs:701-734` |
| The parent's `resolve` sets each sub-document's viewport, then lays it out at once. Setting a viewport asks the document's provider for a redraw (`queue_device_changes`). | `begin_paint`/`end_paint`: a page's request made during a frame is answered by that frame (§3.5, L9 H9). | `resolve.rs:142-162`; `document.rs:1939-1946, 2036-2046` |
| `clear_damage_and_dirty_flags` returns early for a node without damage, leaving its `dirty_descendants` set. | `end_paint` compares hovered nodes, not dirty flags. | `layout/damage.rs:193-203` |
| `BaseDocument::has_changes` returns `changed_nodes.is_empty()`, the opposite of its name. | Not used. | `document.rs:1000-1002` |
| `BlitzApplication` polls a document through the event-loop proxy after every window event. winit-wayland delivers the wake-up before the next iteration's resizes and redraw. winit-appkit redraws inside the live-resize notification. | The window is polled at once after a resize (§3.5, L9 H7). | `blitz-shell/src/application.rs:165`; winit-wayland `event_loop/mod.rs:354-546`; winit-appkit `view.rs:170-183` |
| `View::with_viewport` calls `set_size` for every `SurfaceResized`, and Vello's `set_size` reconfigures the surface. | `CoalescingRenderer` applies the last size once per frame (§3.5, L9 H1). | `blitz-shell/src/window.rs:504-519, 607-627`; anyrender_vello `window_renderer.rs:418-422` |
| Hit-testing uses the paint tree of the last layout, whose stacking contexts keep the ids of hoisted children and index the node tree with them unchecked. An input event between a DOM change and the next layout can panic (`invalid SlotMap key`). | After the poll that follows a resize changes the chrome, the window is laid out at once (`ChromeDocument::lay_out_now`, L9). | `blitz-dom/src/layout/paint_tree.rs:134-147` |
| Vello ignores `set_size` until its resume completes. blitz-shell's `complete_resume` then sets the size again. | `CoalescingRenderer` counts a size as applied only if the renderer was active (L9). | `blitz-shell/src/window.rs:321-362`; anyrender_vello `window_renderer.rs:418-422` |
| anyrender_vello blocks on `device.poll(wait_indefinitely)` after every present. | A GPU kept busy by another process slows every frame. This is not worked around (L9 H11). | anyrender_vello `window_renderer.rs:476-479` |

The ledger records the defects these facts caused and the measurements that
established them.
Four of the defects are written up, with reproductions measured against the
pinned versions, in [`upstream/`](upstream/README.md). They are filed as
DioxusLabs/blitz#1037, #1038 and #1040, and linebender/vello#1975. #1040 is the
sub-document hover behind §3.4, reproduced on `main` too.

---

## 12. Verifying the chrome

### 12.1 Tests

`cargo test -p gaze-shell -p gaze-dom-blitz` runs 368 + 8 tests (373 + 8 with
`--features frame-times`); 87 of gaze-shell's are the chrome's own
(`chrome/tests.rs`), and the rest test the profile, the engine and the
command line (`docs/storage/README.md`, section 15). By area, for the
chrome:

| Area | Tests |
|---|---|
| Protocol and keys | `actions_parse`, `every_emitted_action_parses`, `chrome_keys_route_by_focus`, `browser_shortcuts_select_tabs_even_when_page_has_focus`, `arrow_keys_cycle_through_suggestions_and_back_to_the_text` |
| Markup | `chrome_markup_has_stable_ids`, `removed_toolbar_buttons_stay_commented`, `labels_are_never_bare_text_in_flex_boxes` (R1), `side_controls_are_constant` (R4) |
| Layout | `text_budgets_match_laid_out_boxes`, `the_status_bar_fits_on_one_line`, `measured_width_matches_blitz_layout` |
| Builders | the strip, tab attention, history groups and counts, search highlights, prompts, the error page |
| Behaviour | find (L1, H7), theme switches (L2), closed tabs (L3), Remember (X1), panel switches, the tab menu, the wallet review, suggestion clicks, select-all, the badge, the sidebar collapsed at startup and `restore_sidebar`, the new-tab page's lamp lighting and going out in both schemes (L11), a built-in scheme ignoring the custom theme (L14) |
| State and themes (storage S14) | the session and the history in their own files; a tab switch never rewrites the history, which is written once per delay and on closing; Clear history removes its backups; System follows the OS, an explicit choice ignores it, no preference is dark, an OS change leaves no stale label colour, a portal answer applies on the next poll, the override ignores the window's reports; a chosen scheme is saved (comments kept), or applies to this session only; start-up's notices are shown once; replay logs and exports go to the data folder; a read-only window saves nothing and creates no theme file |
| Appearance and the scheme beyond the chrome (storage S12, part 3; L13) | pages follow the chrome's scheme, hidden ones and new tabs too, and a page that declares no colour scheme keeps light system colours (L13); the window is asked to show the scheme, only the latest request kept, and asked again after every report; choosing System asks the system again; the panel's markup (the lit segment, the System sentence, the theme rows and their tags, the notice for a theme that cannot be used); a theme file chosen, reloaded after editing and dropped when broken; a file that cannot be used is never chosen; New theme copies the colours shown, never replaces anything, and is refused in a read-only session; the list is read again when Appearance is shown; installed themes are listed, and a user's file hides one |
| The pointer over pages (L8) | through a recording window provider and real pointer events: a fresh page never hides the cursor; the cursor follows links and text; leaving a page ends its hover; hover changes are repainted; a page loaded, or a tab switched, under a resting pointer gets its cursor; `cursor: none`; background pages; no redundant cursor requests; built-in buttons |
| Resizing (L9) | through a recording window provider, with frames bracketed as the window brackets them: a relayout hovers again at the pointer's last position (the mechanism); once the mouse has left, resizing hovers nothing and asks for no extra frame; a page's new viewport asks for a frame (the mechanism); a frame answers its pages' requests unless a hover changed during it, and then asks for one more |
| Window state and full screen (storage S13, part 2; L15–L17) | `window.json` written only when what it holds changed, a failed write tried again, a zoom change saved when due; readings in each platform's units (macOS points, X11's frame and surface, Wayland's none, Windows' minimized place); monitors in desktop units; Wayland's size left to the compositor; sizes capped where no monitor bounds them; Blitz's zoom kept in range (L15) and to hundredths; the full-screen chord on each platform, checked before Find, a held chord ignored, one request per press, and the hint on how to leave; History on Cmd+Y on macOS (L16); every key a tip names does what the tip says (L17) |
| Pure helpers | `display` (13), `text_fit` (14, with the shaping face), `theme` (7, with the bundled faces' names and the lit lamp's contrast), `ui_state` (6), `cursor` (6: the `None` rule, `page_point`, `WindowShell` forwarding, `PageShell` grants, requests set aside during a paint, and never another thread's), `application` (12: which events poll at once, report a scheme, ask the portal again, end the hover, or mark the window changed; a focus loss saves only after the window had the focus; the window system from the display handle; no creation theme on Windows; the window's size and place in each platform's units; the loop woken when a save is due; the decorations' theme on each platform; a scheme request changes only what differs), `renderer` (4: coalescing, pass-through, delegation, a size asked for while the renderer resumes), `frame_stats` (2, with `--features frame-times`) |

Each fix in the ledger has a **mutation check**: the fix is commented out, its
test must fail, and then the fix is restored.

CI (`.github/workflows/ci.yml`) runs:
- the workspace tests on Linux, macOS, and Windows;
- the storage smoke test, which also lights the new-tab page's lamp
  headlessly (`scripts/storage-smoke.sh`), and, on Linux and macOS, an old
  profile moved for real (`scripts/storage-migration-ci.sh`);
- a `lint` job: `cargo clippy --workspace --all-targets --locked -- -D
  warnings` with Rust 1.95, and `-p gaze-shell` with `frame-times`,
  `storage-trace`, and `crash-points,storage-trace`;
- the storage model, its traces and the real kills (`model` and `kill`;
  `docs/storage/README.md`, section 15).

### 12.2 The snapshot harness

`scripts/ui-snapshots.sh` drives the real binary under a virtual X server
(Xvfb). It uses `xdotool` for keys and pointer, and ImageMagick 7 for captures
and measurements. Every one of the 99 scenes starts from a seeded throwaway
profile, with the pointer parked on the status bar before the window opens,
and with the fake colour-scheme portal answering 0 (no preference), so no
scene depends on where the previous one left either.

**Cursor scenes (61–68, ledger L8).** A screen capture does not contain the X
cursor, so these scenes read it from the server. `scripts/x-cursor.py` uses
XFixes `GetCursorImage` (python-xlib), which returns the sprite the server
draws. It prints the size, the hotspot, the number of opaque pixels (0 for a
hidden cursor), and a SHA-256 of the sprite. Each scene first takes reference
sprites in the same run:
- the hand, over the Reload button (`button{cursor:pointer}`);
- the text cursor, over the address field;
- the arrow, over the rail's empty middle.

The checks then compare hashes, so they do not depend on which cursor theme
is installed. They need one, though: with no theme found, the X server draws
one fallback sprite for every shape, and `cursor_refs_distinct` fails. The
theme is found through `$XDG_DATA_DIRS`, which the harness points into its
work directory, so the harness names the search path in `XCURSOR_PATH`
first (ledger S14, H0). The sprites are saved in `cursors/`. The pages are `site/cursor.html` and
`site/cursor-next.html`: large boxes at fixed places, the second with its link
where the first has a plain block.

**Lamp scenes (69–71, ledger L11).** They click the new-tab page's lamp in
scene 58's layout (the sidebar open), where `CROP_LAMP` is calibrated. Each
waits a second after the page loads, so that its program is listening, and
captures the unlit lamp as a reference before the first click.
- Scenes 69 (dark) and 70 (light) click once. The lamp must change, show the
  page's yellow, and keep a legible label.
- Scene 71 clicks twice. No yellow may be left, and the lamp must match the
  reference pixel for pixel. Blitz outlines only focused inputs and text
  areas, so the clicked button has no focus ring.

**System-scheme, theme-file and page-scheme scenes (72–84; storage ledger
S12, part 3, and L13).** They need the current seed format and are skipped
with the legacy one (`FORMAT_IN`).
- **72–75**: theme System, Appearance open, the fake portal answering 1
  (dark), 2 (light), 0 (no preference) or failing. The strip must match
  scene 15's or 16's exactly (System looks like the scheme it follows), the
  System segment must differ from those scenes' (it is lit), and a portal
  that fails must be told apart from one that states no preference (the
  sentence). Scene 72 also checks the arguments `dbus-send` was given.
- **76**: the portal turns from dark to light while the window is open. The
  scene waits past start-up's query, takes the focus away and gives it
  back (python-xlib), and the sidebar and the strip must then match scene
  73's exactly: the window asked again and showed the answer.
- **77**: a portal that never answers. The title must appear within 3 s
  (start-up waits at most 100 ms), dark is shown, and no `dbus-send` is left
  running after its 1 s deadline.
- **78–82**: three theme files (`three_themes`: one that cannot be used, a
  dark one and a light one). The list (78); a row clicked (79), which must
  look like a start with that theme (80, the same profile path); New theme,
  then the file edited and reloaded (81, scrolled to the card's buttons); a
  chosen file that cannot be used (82), whose notice comes first.
- **83, 84**: a page with its own light and dark styles
  (`site/scheme.html`), in a dark and a light chrome. With the step-9 binary
  scene 83 showed the page light (`dark_page_px` 0): the red run of L13.

**The window and the storage scenes (85–99; storage ledger S13, part 2).**
They need the current seed format. The X server's monitor is named as
`xrandr` names it (`screen` under Xvfb), and `window.json` is seeded
(`seed_window`) and read (`wjson`) with jq. With no window manager, X11
reports no move and carries out no maximize, so these scenes read the
window as the X server has it (`xdotool getwindowgeometry`).
- **85–89**: the first start (the default size at the screen's corner, and
  the first save with no event to report the window, written before the
  history's save: without the loop's own wake-up, the history's would carry
  it, just in time for the other checks; it must look exactly like scene
  19, made at 800×600 and resized); a window made where it was
  left, which writes nothing; a window left off screen on a monitor that is
  gone, centred on the primary; a 4 s drag, saved during it (the 3 s
  bound) and after it; a move and resize closed within 500 ms
  (`close_window`, `WM_DELETE_WINDOW`), saved as read before the window
  went.
- **90–93**: Blitz's zoom keys, kept; a zoom restored; twelve presses of
  Ctrl+−, kept at 25 % (L15); F11 into `window.json` and back, with the
  hint. A window's rendering at a zoom depends a little on the zoom it
  started at (Blitz keeps the border widths of its first scale:
  [DioxusLabs/blitz#1076](https://github.com/DioxusLabs/blitz/issues/1076);
  ledger S13 part 2, E3), so a restored zoom is compared
  with the same window after Ctrl+0 and the keys, which match only if the
  zoom restored is exactly the keys'.
- **94–97**: storage scenes. An old single-folder profile found through
  HOME and moved by a start with no `--profile`, in a HOME and XDG folders
  of the scene's own (`new_machine`), whose `paths` must name only them
  (`machine_self_test`); a move that meets a file already there; damaged
  `session.json` and `window.json`, backed up and made anew; and a second
  F1R3Gaze on one profile, whose window and headless run are refused and
  whose reading command goes on.
- **98, 99**: with a window manager, openbox (`start_wm`; its configuration
  has no key or mouse bindings): full screen and back, made full screen
  again, and no creep by the frame; maximized, kept so with the size before
  it, made maximized again after the window is shown, and back. They run
  last, so no scene without a window manager runs after one. Without
  openbox the harness exits 2 when they are selected; leave them out with
  `--only '^([0-8][0-9]|9[0-7])-'`, which `run.txt` and `runs.tsv` record.

```sh
scripts/ui-snapshots.sh --after                  # all scenes → docs/screenshots/ui/after/
scripts/ui-snapshots.sh --before --bin OLD_BINARY
scripts/ui-snapshots.sh --after --only '^(2[7-9]|3[0-2])-'   # the find scenes
scripts/ui-snapshots.sh --list                   # scene names
scripts/ui-snapshots.sh --compare                # side by side, into the work directory
```

The options and exit codes:
- **Options.** `--size 1280x800` (the default), `--timeout SECS`, `--work DIR`
  (the work directory; default `target/ui-snapshots`), `--keep` (keep the
  profiles in the work directory), `--calibrate` (draw the coordinate table
  on a capture, points as crosses and crops as boxes; in after mode also on
  the Appearance panel with three theme files, at its top and scrolled to
  its end), `--seed-format current|legacy` and `--check-isolation`
  (below).
- **Exit codes.** 0 for success; 1 when a scene or check failed; 2 when a tool,
  the binary, or a display is missing; 3 for an unsafe path, or an isolation
  self-test that failed; 64 for usage.

**Safety.**
- The harness works in `target/ui-snapshots` under a lock. The directory is
  fixed, so the demo site's `file://` URLs, which the captures show, are the
  same in every run. It is on disk because `/tmp` is often tmpfs, i.e. RAM.
  Runs before 2026-10-04 used `${TMPDIR:-/tmp}/f1r3gaze-ui-snapshots`, so
  `file://` URLs in older captures show that path. Each run records its work
  directory in `run.txt`.
- **Isolation** (`scripts/lib/storage-isolation.bash`, shared with the
  resize scripts). Every profile is a portable root under the work
  directory. Every XDG variable points into the work directory, and so does
  `F1R3GAZE_PROFILE`. There is no session bus: a fake `dbus-send` answers the
  colour-scheme question from `$WORK/portal/scheme` and logs what it was
  asked. Every new profile resets the answer to 0 (no preference), and the
  system-scheme scenes set 1 (dark), 2 (light), `hang` or `fail` (`portal`). What the XDG folders also locate for
  other programs is named directly: the Vulkan driver (`VK_DRIVER_FILES`)
  and the X cursor theme (`XCURSOR_PATH`).
- **The guard.** A work directory or profile that is, holds or is inside a
  real F1R3Gaze folder is refused (exit 3) before anything is written. The
  real folders are computed from the real environment first: the five roots,
  both places of the old single-folder profile, the macOS folders, and
  `F1R3GAZE_PROFILE`.
- **The self-test.** Before Xvfb starts, `f1r3gaze paths` must name only the
  work directory, both with `--profile` and without it (with `HOME` in the
  work directory: an old profile is found through `HOME`, and a start
  without `--profile` would move it), and `dbus-send` must be the fake.
  `--check-isolation` runs only the guard and the self-test.
- **Seeding.** `--seed-format current` (the default) seeds
  `config/settings.toml` (`[appearance]` with `restore_sidebar` and the
  scene's `theme`, `[shard] observers = []`, and `[wallet] embers_api` when a
  scene asks for it), `state/session.json` and `state/history.json`, and a
  scene's palette as `config/themes/custom.css`, and theme files as
  `config/themes/<name>.css` (`write_theme`, `three_themes`). `--seed-format legacy` seeds
  the single folder's `settings.conf`, `workspace.json` and `palette.css`, for
  binaries from before the five-root layout (the S1 baseline), and skips the
  self-test, which needs `paths`.
- It runs a mock Embers server for wallet scenes.
- The wallet key is a fixed test key that is never funded.

**Checks.** They are recorded in `checks.tsv` for both captures and enforced
for `--after`. Every check names a crop of the screen and a limit:

| Check | Scene | Before | After | Limit |
|---|---|---|---|---|
| `page_reflow_px` (find opens) | 27 | 18 003 | 0 | ≤ 0 |
| `selection_pixels` (find cleared) | 30 | 720 | 0 | ≤ 0 |
| `rail_reflow_px` (suggestions open) | 34 | 431 | 0 | ≤ 0 |
| `bubble_px` (rail hover label) | 45 | 0 | 1 813 | ≥ 1 |
| `hover_px` (row hover) | 46 | 917 | 1 238 | ≥ 1 |
| `flash_cleared_px` (message expired) | 48 | 0 | 207 | ≥ 1 |
| `sidebar_px_vs_03` (search reset) | 49 | 4 416 | 0 | ≤ 0 |
| `recipient_px`, `amount_px`, `review_px` | 50 | 250 / 429 / 6 405 | 1 240 / 1 050 / 16 081 | ≥ 1 |
| `stale_colour_px_vs_56` / `_vs_58` (scheme switch) | 57, 59 | 335 / 218 | 0 / 0 | ≤ 0 |
| `export_label_contrast` (before) / `sidebar_px_vs_12` (after): the Wallet after a switch to light | 60 | 1.00 | 0 | ≥ 3 / ≤ 0 |
| `allow_label_contrast` / `lamp_label_contrast` | 56, 58 | 1.00 / 1.47 | 4.78 / 15.23 | ≥ 3 |
| `*_edge_artifact_px` (row, rail, notice) | 02, 16 | — | 0 | ≤ 0 |
| `*_bar_px` (row, rail, tab, notice, prompt) | 02, 16, 57 | — | 3, 3, 2, 3, 3 | = width |
| `menu_opened_px`, `duplicated_px`, `flash_shown_px` | 21, 22, 47 | recorded | > 0 | ≥ 1 |
| `cursor_refs_distinct`: the three reference sprites differ | 61–65, 67, 68 | — | 1 | = 1 |
| `page_opaque_px`, `page_is_arrow`: the cursor is visible and the arrow after entering a page from Back, a rail button, or a Tabs-panel row | 61, 62, 63 | 0 (hidden) | 272, 1 | ≥ 1, = 1 |
| `link_is_hand`, `text_is_text_cursor`: over the page's link and text | 64, 65 | 0, 0 (hidden) | 1, 1 | = 1 |
| `hover_fill_px`, `left_fill_px`: the link's `:hover` fill while hovered, and after the pointer leaves the page | 66 | 0, 18 000 | 18 000, 0 | ≥ 15 000, = 0 |
| `resting_is_hand`: a page loaded under the resting pointer | 67 | 0 (arrow) | 1 | = 1 |
| `hidden_opaque_px`, `back_opaque_px`, `back_is_arrow`: `cursor: none`, then off it | 68 | 0, 0, 0 | 0, 272, 1 | = 0, ≥ 1, = 1 |
| `lamp_lit_px`, `lit_fill_px` (pixels of `#FFCF3F` in the 52 × 22 `CROP_LAMP`), `lit_label_contrast`: the lamp after one click, in dark / in light | 69, 70 | 0, 0, 13.05 / 0, 0, 15.23 | 584.6, 926, 10.67 / 319.0, 926, 10.67 | ≥ 1, ≥ 500, ≥ 4.5 |
| `out_fill_px`, `out_px_vs_unlit`: the lamp after two clicks | 71 | 0, 0 | 0, 0 | = 0, ≤ 0 |
| `portal_argv_ok`: the fake portal was asked with `ReadOne`'s exact arguments | 72 | — | 1 | = 1 |
| `strip_px_vs_15` / `strip_px_vs_16`: System, or a theme that fell back, looks exactly like the scheme shown | 72, 74, 75, 77, 82 / 73 | — | 0 | ≤ 0 |
| `system_segment_vs_15` / `_vs_16`: System is lit, not Dark or Light | 72 / 73 | — | 330.7 / 217.7 | ≥ 1 |
| `note_px_vs_74`: "could not be read" is not "states no preference" | 75 | — | 594.8 | ≥ 1 |
| `portal_queries`, `sidebar_px_vs_73`, `strip_px_vs_73`: the refocus flip | 76 | — | 2, 0, 0 | ≥ 2, ≤ 0, ≤ 0 |
| `title_ms`, `hung_children`: a portal that never answers | 77 | — | 633, 0 | ≤ 3 000, = 0 |
| `broken_icon_px` (pixels of `#FF8A80` in the theme rows) | 78 | — | 81 | ≥ 40 |
| `solar_bg_px`, `stale_colour_px_vs_79`: a theme file chosen in place, then started with | 79, 80 | — | 38 020, 0 | ≥ 20 000, ≤ 0 |
| `new_theme_file`, `new_theme_chosen`, `reloaded_bg_px`: New theme, then Reload | 81 | — | 1, 1, 38 020 | = 1, = 1, ≥ 20 000 |
| `error_notice_px` (pixels of `#FF8A80` at the panel's top) | 82 | — | 223 | ≥ 100 |
| `dark_page_px` / `dark_page_px`, `white_page_px` (in `CROP_PAGE_LEFT`, 396 800 px): the page's own styles in a dark / a light chrome | 83 / 84 | 0 / 0, 392 688 | 392 688 / 0, 392 688 | ≥ 300 000 / = 0, ≥ 300 000 |
| `default_size_and_place`, `first_save`, `first_save_before_history` (the window's state written before `history.json`), `first_start_px_vs_19` (over `CROP_ALL`) | 85 | 0, 0, 0, 511 593 | 1, 1, 1, 0 | = 1, = 1, = 1, ≤ 0 |
| `restored_size_and_place`, `restored_writes_nothing` | 86 | 0, 1 | 1, 1 | = 1, = 1 |
| `centred_on_primary`, `saved_where_shown` | 87 | 0, 0 | 1, 1 | = 1, = 1 |
| `saved_during_drag`, `saved_after_drag` (the width) | 88 | 0, 900 | 1, 1 095 | = 1, = 1 095 |
| `closed_cleanly` (exit status), `saved_at_close`, `session_kept` (tabs) | 89 | 0, 0, 6 | 0, 1, 6 | = 0, = 1, = 6 |
| `zoom_saved`, `zoomed_px_vs_19` | 90 | 1.0, 70 145 | 1.2, 70 145 | = 1.2, ≥ 1 |
| `zoomed_px_vs_19`, `zoom_restored_px_vs_keys` | 91 | — | 70 034.8, 0 | ≥ 1, ≤ 0 |
| `alive_after_zoom_out`, `zoom_floor_saved`, `zoom_floor_px_vs_keys` (L15) | 92 | 1, 1.0, — | 1, 0.25, 0 | = 1, = 0.25, ≤ 0 |
| `fullscreen_saved`, `fullscreen_hint_px`, `fullscreen_left_saved` | 93 | 0, 0, 0 | 1, 306.3, 1 | = 1, ≥ 1, = 1 |
| `migrated_note`, `user_id_moved`, `originals_kept`, `old_files_left`, `settings_converted`, `session_tabs`, `history_visits` (from before the start) | 94 | 1, 1, 2, 0, 2, 6, 8 | 1, 1, 2, 0, 2, 6, 8 | = 1, = 1, = 2, = 0, = 2, = 6, = 8 |
| `strip_px_vs_02`, `sidebar_px_vs_02`, `notice_px_vs_02` | 94 | 18 128.6, 52 533.4, 27 155.3 | 0, 0, 968.9 | ≤ 0, ≤ 0, ≥ 1 |
| `both_ids_kept`, `conflict_listed`, `marker_written`, `warning_px` (`#EFBF75` in the status bar) | 95 | 2, 1, 1, 0 | 2, 1, 1, 43 | = 2, = 1, = 1, ≥ 21 |
| `session_backup_kept`, `window_backup_kept`, `default_window`, `warning_px`, `session_restarted` | 96 | 1, 1, 0, 0, 1 | 1, 1, 1, 43, 1 | = 1, = 1, = 1, ≥ 21, = 1 |
| `second_window_refused`, `holder_named`, `headless_refused`, `reader_goes_on`, `first_unchanged_px_vs_19` | 97 | 1, 1, 1, 1, 0 | 1, 1, 1, 1, 0 | = 1, ≥ 1, = 1, = 1, ≤ 0 |
| `framed_place`, `fullscreen_state`, `fullscreen_geometry`, `fullscreen_px_vs_19` (`CROP_ABOVE_STATUS`), `left_fullscreen_place`, `no_creep`, `restored_fullscreen_state`, `restored_fullscreen_geometry`, `restored_then_left_place` | 98 | 0, 0, 0, 493 914, 0, 1, 0, 0, 0 | 1, 1, 1, 0, 1, 1, 1, 1, 1 | = 1 each, `fullscreen_px_vs_19` ≤ 0 |
| `maximized_state`, `maximized_geometry`, `normal_kept_while_maximized`, `restored_maximized_state`, `restored_maximized_geometry`, `unmaximized_place` | 99 | 1, 1, 0, 0, 0, 0 | 1 each | = 1 each |

The "Before" column of the cursor rows is the release binary from before the
L8 fix (`635ed212…`), run with `--out` in a scratch directory, not the
`before/` capture. That of the lamp rows is likewise the binary from before
the L11 fix (`56c385c1…`, built from `d2bfc05`). With it a click changed no
pixel of the lamp, and its label contrasts are those of the unlit lamp. The
limit of 500 yellow pixels is about half the crop: the lit lamp fills 926,
and the label takes the rest. That of scenes 83 and 84 is the step-9 binary
(`0dc5c59c…`), before L13; scenes 72–82 are new with step 10. That of scenes
85–99 is the step-10 binary (`f48811d8…`), before S13 part 2: its window
opened at winit's 800×600, so the storage scenes' window checks failed
there too, and scenes 94–97 otherwise check behaviour of steps 9 and 10.
In scene 99 the window manager maximizes that window too, and `no_creep`
passes in 98 because a binary that saves nothing leaves the seeded place;
a hand mutation that saves the surface's corner instead of the frame's
makes it fail (storage ledger S13, part 3).

How the less obvious checks measure:
- **Effect checks** (`≥ 1`) compare a capture taken just before an interaction
  with one after it. A click that misses its target changes nothing and fails
  the run.
- **Pixel differences** (`ae`) are ImageMagick's AE metric. The effect checks
  use them, and so do the reflow checks and the comparisons with another scene
  or a reference (`sidebar_px_vs_03`, `sidebar_px_vs_12`, `out_px_vs_unlit`).
  Under ImageMagick 7.1.2 the metric is not a count of differing pixels. It
  is the sum over pixels of the mean absolute channel difference, each between
  0 and 1. A pixel that changes completely counts 1, so the value is
  fractional and can be smaller than the number of pixels that changed. It is
  0 only for identical crops (ledger L11).
- **The theme checks** compare colour histograms of a screen switched to a
  scheme with one started in it. A label left in the old colour shows up even
  if a glyph moved by a pixel. Both captures of a pair use the same profile
  path, so the themes folder the Appearance panel shows is the same text in
  both (ledger L10). Scenes 76 and 80 use their comparison scene's profile
  path for the same reason.
- **`edge_artifact`** takes a band across an edge. It counts pixels that are not
  a blend of the colour outside (the band's first row) and the colour inside
  (its last row).

**On other desktops.** The window's keeping, full screen and the colour
scheme were checked on KDE Plasma's Wayland session of the development
machine (storage ledger S18, part 1). What only macOS and Windows show (Cmd
shortcuts and their tips, the title bar's scheme, sizes in points or at a
display scale, maximize and minimize) is a checklist for a person at each:
[`docs/storage/manual-checks.md`](../storage/manual-checks.md).

### 12.3 Profiling the render loop

**The `frame-times` feature.** It is off in normal builds. With it, the
window's renderer prints one line per frame on stdout. Blitz's resolve phases
(`Resolve(N): …`) and Vello's frame phases (`vello: … cmd, render, present,
wait`) print next to it. The frame line's fields:

| Field | Meaning |
|---|---|
| `n`, `t`, `epoch`, `interval` | the frame number; milliseconds since the window opened; the wall clock in milliseconds, to line frames up with outside logs; the time since the previous frame |
| `size`, `resizes`, `reconfigures`, `reconfigure` | the size painted; `set_size` calls since the previous frame; how many reached Vello, and the time they took |
| `render`, `latency` | the time in Vello's `render`; from the first resize since the previous frame to the end of this one |
| `polls`, `poll`, `chrome_renders`, `chrome_render`, `events`, `event` | the chrome's polls, render passes, and event handling, counted and timed (`crate::frame_stats`) |
| `page_redraws`, `chrome_redraws`, `cursor_sets` | redraws asked for by pages and by the chrome, and cursors sent to the window, counted |
| `window_saves`, `window_save` | the window's readings since the previous frame, each of which writes `window.json` if it changed, counted and timed (§3.7). A reading made while the window sits still shows on the next frame, however much later |
| `coalesce` | whether resizes are coalesced |

Each reading of the window also prints a line of its own when it ends,
`window_save epoch=… took=… changed=0|1`: its wall-clock time in
milliseconds, how long it took, and whether `window.json` was written
(`frame_stats::WindowSave`). Only this line says when a reading happened.

In a `frame-times` build, five environment variables turn one behaviour off,
so one binary measures before and after: `F1R3GAZE_COALESCE_RESIZE=0`,
`F1R3GAZE_POLL_ON_RESIZE=0`, `F1R3GAZE_END_HOVER_ON_LEAVE=0`,
`F1R3GAZE_ANSWER_IN_PAINT=0`, and `F1R3GAZE_KEEP_WINDOW=0` (no
`window.json` kept; `resize-bench.sh --keep-window 0`).

```sh
# A profiling build, kept out of the normal target directory.
CARGO_TARGET_DIR=target/scratch/profiling CARGO_PROFILE_RELEASE_STRIP=none \
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
  cargo build --release -p gaze-shell --features frame-times

# Xvfb: sweep 1280×800 → 900×600 → 1280×800 in 4 px steps at 120 Hz, 5 runs.
scripts/resize-bench.sh --bin target/scratch/profiling/release/f1r3gaze
# One resize per frame, so every extra frame shows: 20 px steps at 10 Hz.
scripts/resize-bench.sh --bin … --rate 10 --step 20
# A build that restores no tabs: the page on the command line.
scripts/resize-bench.sh --bin … --single-tab
# Without window.json kept, and the writes of window.json counted
# (storage ledger S13, part 2).
scripts/resize-bench.sh --bin … --keep-window 0 --watch-saves
# Fold and summarise a log taken anywhere.
scripts/resize-bench.sh --analyze LOG

# A window on your own desktop: drag a corner, then close it.
scripts/resize-live.sh --bin … --no-perf --label NAME
```

`resize-bench.sh` writes one row per frame (`frames-N.tsv`), one row per
reading of the window (`saves-N.tsv`), each sweep's start and end
(`sweeps.tsv`), one row per run (`runs.tsv`), and medians over the runs
(`summary.txt`). With `--watch-saves` it also counts, with inotify, the
writes of `window.json` during each sweep and in the whole run
(`watch.tsv`).

A run's row covers its sweep: the frames from the first one with a resize to
the last one with a resize, among those painted from the sweep's start until
a second after its end. A log given to `--analyze` has no sweep times, so its
row runs from its first frame with a resize to its last. Among the columns:
- `resize_frames` counts the frames that applied a size, and `extra_frames`
  those painted without one.
- `saves` counts the readings of the window made while it was being resized,
  from the first resize's arrival (its frame's `epoch` less its `latency`) to
  the last resize frame; `save_writes` counts those that wrote `window.json`,
  and `save_max_ms` is the longest. They come from the `window_save` lines,
  not from the frame line, which would put a reading made in the wait before
  a sweep on the sweep's first frame (storage ledger S13, part 2).

Xvfb renders with software Vulkan on X11: it measures the CPU side and the
pattern of events, not a GPU or a Wayland compositor.

`resize-live.sh` records the same log while you drag. Since S13 part 2 its
throwaway profile's window opens at 1280×800 logical pixels, the default
size, rather than winit's 800×600. Without `--no-perf` it
also attaches `perf record` and writes `perf-report.txt`,
`perf-children.txt` and `flamegraph.svg`.

**Comparing builds by hand.** L9's comparisons opened the builds one after
another in a random order, kept the order hidden until the user had rated
each window, and gave builds that look alike the same tabs. To compare
`main`, which has no `frame-times` feature, export it and add only the frame
log (ledger L9).

**Pitfalls.**
- **perf.** On AMD Zen 3, `perf record --call-graph lbr` fails
  (`sys_perf_event_open() … Invalid argument`), because the CPU has no LBR.
  Use `-e cycles:u --call-graph dwarf,16384`.
- **Other GPU users.** Anything else that keeps the GPU busy slows every frame
  (§3.5). Log `nvidia-smi pmon -s u -d 1` (or the platform's equivalent) next
  to any live timing, and leave files that an indexer watches alone while it
  runs (ledger L9, H11).

---

## 13. References

- **[W3C-WCAG21]** W3C. *Web Content Accessibility Guidelines (WCAG) 2.1.* W3C
  Recommendation. <https://www.w3.org/TR/WCAG21/>. Definitions of
  relative luminance and contrast ratio; success criteria 1.4.3 and 1.4.11.
- **[CSS21-E]** W3C. *CSS 2.1, Appendix E: Elaborate description of stacking
  contexts.* <https://www.w3.org/TR/CSS21/zindex.html>.
- **[CSS-Cascade-4]** W3C. *CSS Cascading and Inheritance Level 4.* W3C
  Candidate Recommendation Snapshot, 13 January 2022.
  <https://www.w3.org/TR/css-cascade-4/>. §6.1 Cascade Sorting Order: origin
  and importance, from transitions and important user-agent declarations down
  to normal user-agent declarations.
- **[CSS-Images-3]** W3C. *CSS Images Module Level 3*, linear gradients and
  colour-stop fix-up. <https://www.w3.org/TR/css-images-3/>.
- **[Knuth 1984]** D. E. Knuth. "Literate Programming." *The Computer Journal*
  27(2):97–111, 1984. [doi:10.1093/comjnl/27.2.97](https://doi.org/10.1093/comjnl/27.2.97).
- **[Levenshtein 1966]** V. I. Levenshtein. "Binary codes capable of correcting
  deletions, insertions, and reversals." *Soviet Physics Doklady*
  10(8):707–710, 1966.
- **[Ukkonen 1985]** E. Ukkonen. "Algorithms for approximate string matching."
  *Information and Control* 64(1–3):100–118, 1985.
  [doi:10.1016/S0019-9958(85)80046-2](https://doi.org/10.1016/S0019-9958(85)80046-2).
- **[Hinnant]** H. Hinnant. *chrono-Compatible Low-Level Date Algorithms.*
  <http://howardhinnant.github.io/date_algorithms.html>.
- **[Unicode §3.13]** The Unicode Consortium. *The Unicode Standard*, Chapter 3,
  §3.13 Default Case Algorithms (Final_Sigma).
  <https://www.unicode.org/versions/latest/core-spec/chapter-3/>.
- **Blitz** <https://github.com/DioxusLabs/blitz> (rev `674d7d2`); **parley**
  <https://github.com/linebender/parley> (rev `332c1c7`); **Vello**
  <https://github.com/linebender/vello> (0.10).
