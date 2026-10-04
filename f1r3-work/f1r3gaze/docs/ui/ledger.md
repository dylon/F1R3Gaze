# Chrome defect ledger

This ledger records how each chrome defect was diagnosed and fixed: what was
observed, the hypotheses tested, the experiment for each, the raw result, and
the verdict.

Rules for every entry:
- One variable changes per experiment.
- Every fix gets a **mutation check**: comment the fix out, confirm the
  regression test goes red, then restore it. (No `git stash` or `git reset`.)

The design these defects led to is in [README.md](README.md).

Evidence comes from two sources:
- **Snapshots** taken by `scripts/ui-snapshots.sh` with the release binary from
  before the change (sha256 `f7a6d0b3…c55a5`). They are in
  [`../screenshots/ui/before/`](../screenshots/ui/before/), and their
  measurements are in [`checks.tsv`](../screenshots/ui/before/checks.tsv).
- **Tests** in `crates/gaze-shell/src/chrome.rs` and `crates/gaze-shell/src/pages.rs`.

Entry format: **ID | hypothesis | prediction | experiment | raw result | verdict | evidence**.

---

## L1 — Find leaves a character highlighted after the query is cleared

**Observation (O1).** Steps: open find (Ctrl+F), type `alpha`, press Enter, then
BackSpace five times. The `a` in the heading "Field notes on g**a**ze…" stays
highlighted. See [`30-find-cleared.png`](../screenshots/ui/before/30-find-cleared.png).

**Measurements:**
- **E1.** Pixels of exactly `#B4D5FF`, Blitz's text-selection colour
  (`blitz-paint/src/lib.rs`):

  | Scene | Pixels |
  |---|---|
  | `30-find-cleared` | 720 |
  | `28-find-matches` (overlay only) | 0 |
  | `27-find-open` | 0 |

  The find overlay's own colours (`#ffe27988`, `#70d5cfaa`) cannot produce
  `#B4D5FF`.
- **E2.** The count label reads "Find in page". `render()` shows that only when
  `find_query` is empty, so the empty query *did* reach the chrome.

| ID | Hypothesis | Prediction | Experiment | Raw result | Verdict |
|---|---|---|---|---|---|
| H1 | `refresh_find()` returns early on an empty query without `select_find_hit(None)` | After `Input("")`, the selection is the first hit of `a` in document order: the heading, not the `alpha` that Enter chose | Test `find_clears_selection_when_query_is_emptied`, unfixed | Control assertion `Some("a")` passed; then `panicked … H1: an empty query must not leave a hit selected` | **Supported** |
| H2 | BackSpace to empty sends no Input event | `find_query` would stay `"a"` | Test `find_input_follows_real_key_events`: real `UiEvent::KeyDown` events through Blitz's text input | `ok`: the query went `"a"` then `""`. Also `keyboard.rs` dispatches `Input` after every edit, and E2 | **Refuted** |
| H3 | The 250 ms rescan in `poll()` re-selects | Clearing would not survive polling | Same test as H1: three `poll(None)` calls after clearing | No reselection once H1 was fixed; the rescan is guarded by `!find_query.is_empty()` | **Refuted** |
| H4 | A highlight overlay is left behind | `rendered["findoverlay"]` would not be empty | Same test as H1 | Empty; E1 also excludes overlay colours | **Refuted** |
| H6 | `select()` (tab switch) neither clears the outgoing tab's selection nor re-runs find on the incoming tab | Old tab stays highlighted; new tab shows "0 matches" | Test `find_selection_follows_the_active_tab` | `panicked … H6: the outgoing tab keeps no find selection`, and still red after the H1 fix alone | **Supported** (independent of H1) |

### Chronological log (2026-10-04)

1. **Red.** `cargo test -p gaze-shell` with the tests added and no fix:
   ```
   test chrome::tests::find_clears_selection_when_query_is_emptied ... FAILED
   test chrome::tests::find_input_follows_real_key_events ... ok
   test chrome::tests::find_selection_follows_the_active_tab ... FAILED
   H1: an empty query must not leave a hit selected
   H6: the outgoing tab keeps no find selection
   ```
2. **H1 fix only.** Clear the selection in the empty-query branch of
   `refresh_find`. H1 goes green; H6 stays red, which shows the two defects are
   independent:
   ```
   test chrome::tests::find_clears_selection_when_query_is_emptied ... ok
   test chrome::tests::find_selection_follows_the_active_tab ... FAILED
   H6: the outgoing tab keeps no find selection
   ```
3. **H6 fix.** `select()` clears the outgoing selection and calls
   `refresh_find()`:
   ```
   test result: ok. 3 passed; 0 failed
   ```
4. **Mutation check.** With the H1 line commented out, the test fails again:
   ```
   test chrome::tests::find_clears_selection_when_query_is_emptied ... FAILED
   H1: an empty query must not leave a hit selected
   ```
   Restored, it passes: `test result: ok. 3 passed`.
5. **Final form.**
   - Find records which tab holds *its* selection (`find_selected_tab`).
   - `clear_find_selection()` clears only that selection. It is called by
     `refresh_find`, `select`, `close`, and `FindHide`.
   - A selection the user made therefore survives opening find. The new test
     `find_keeps_a_selection_it_did_not_make` checks this.
   - `FindHide` returns keyboard focus to the page. The floating box is only
     hidden, so focus would otherwise stay in an invisible field.
   - `reveal_find_hit` takes a top margin (62 px), so a revealed hit is never
     under the floating box.
   - All find tests are green.

**Invariant:** a find selection exists only when the find box is open, the
query is non-empty, there is a hit, and the selection is on the active tab.

### H7 — a tab attached while find is open

**Origin.** The fix for H6 searches the incoming tab when it becomes active.
Reading `poll()` while making that fix showed that a tab's document is
attached before it has any layout. The question was what a search at that
moment finds.

**Hypothesis H7.** `refresh_find` runs when the tab's document is attached.
`poll()` attaches documents before they have a layout, and `RhoDocument::find`
reads text positions from the layout, so the search finds nothing. Nothing
searches again once the layout exists.

**Prediction.** If H7 holds, find has no hits right after attaching and still
none after the page is laid out. With a rescan, the hit appears once the page
is laid out.

**Experiment.** `find_searches_a_new_page_once_it_is_laid_out`:
1. Open find with "alpha".
2. Open a second page and attach it without laying it out (`attach_unlaid`).
3. Lay it out and poll.

**Fix.**
- At attach, `page_is_laid_out` is false, so `refresh_find` sets `find_rescan`.
- `schedule_wakes` asks the exported `Pacer` (in `gaze-dom-blitz`, one timer
  thread that wakes the window at the earliest pending deadline) for a poll
  30 ms later (`FIND_RESCAN_DELAY`).
- The next `poll` searches the laid-out page and clears `find_rescan`.

**Mutation check (2026-10-04), first attempt: confounded.** With the rescan in
`poll` commented out, the test still found the hit. It failed only at
`assert!(!chrome.find_rescan)`. The cause: `poll` has an older, periodic
rescan. When a sub-document reports a change and 250 ms have passed since the
last search, it searches the active page again, and here it found the hit.
That rescan depends on page activity and timing, so a static page can stay at
"0/0" in the window. The flag assertion still caught a real regression: left
set, it would make the `Pacer` wake the window every 30 ms indefinitely.

**Mutation check, controlled.** The test now sets `last_find_scan` to now just
before the poll that follows layout. The periodic rescan cannot run, so only
H7's rescan can find the hit (one variable at a time).
- Rescan commented out: `searched once laid out: left 0, right 1` (red).
- Restored, and the file byte-identical to its backup: `ok`.

**Verdict.** Supported. The same `Pacer` expires status-bar messages (G5)
instead of a sleeping thread per message.

---

## L2 — Labels keep the previous scheme's colour after a theme switch

**Observation (O2).** Switch Dark→Light with the Wallet panel open, and the wallet's
button labels vanish. See
[`60-theme-wallet-to-light.png`](../screenshots/ui/before/60-theme-wallet-to-light.png).

**Measurements:**
- **Pixel sampling.** The "Export" label is still drawn in the dark scheme's
  `--gaze-text` (`#EAF0F7`) on the light button background (`#EAF0F4`).
- **WCAG contrast** between label and button, measured by the harness's
  `max_contrast`:

  | Label | Scene | Contrast |
  |---|---|---|
  | Export | `60-theme-wallet-to-light` | **1.00** |
  | Allow (prompt bar) | `56-theme-prompt-to-light` | **1.00** |
  | lamp (new-tab page) | `58-theme-newtab-to-light` | **1.47** |
  | Export (legible control) | `11-wallet-one-dark` | 11.2 |

- **Switched versus fresh.** A screen switched to a scheme should match one
  started in that scheme. The harness compares colour histograms, which ignore
  a glyph that merely moved by a pixel:

  | Switched scene | Fresh scene | Pixels in a different colour |
  |---|---|---|
  | `56-theme-prompt-to-light` | `57-theme-prompt-fresh-light` | 335 |
  | `58-theme-newtab-to-light` | `59-theme-newtab-fresh-light` | 218 |
  | `52-theme-appearance-to-light` (Appearance re-renders) | `53-theme-appearance-fresh-light` | 0 |

**Mechanism (source-verified).**
1. Blitz's user-agent style sheet makes every `<button>` `display:inline-flex`
   (`blitz-dom/assets/default.css`).
2. Bare text inside a flex container is wrapped in an *anonymous block*.
3. That block's style is computed once, in `create_anonymous_block`
   (`blitz-dom/src/layout/construct.rs`), from its parent's style at that moment.
4. The style traversal visits only real DOM children (`stylo.rs`), so the block
   is never restyled.
5. A colour-only change does not rebuild layout boxes
   (`layout/damage.rs::compute_layout_damage`: only display, float, position,
   contain, visibility, or font changes do).
6. Glyph colour is read at paint time from that anonymous block's style
   (`blitz-paint/src/text.rs`).

| ID | Hypothesis | Test | Verdict |
|---|---|---|---|
| T1 | The mechanism above | Predicts exactly which labels go stale; see the red result below | **Supported** |
| T2 | The button element's own colour is stale | The `<i>` icon glyphs inside the same toolbar buttons recolour correctly | **Refuted** |
| T3 | Paint caches glyph colours | `TextBrush` holds only a node id | **Refuted** |
| T4 | Root custom properties never restyle | Backgrounds and `<span>` text recolour | **Refuted** |
| T5 | The `set_html` cache keeps the stale nodes alive | Panels whose HTML does not change across schemes are never re-inserted, so their anonymous blocks persist | **Supported** (contributing condition) |

**Red result.** The invariant check is `stale_text_colours`: every anonymous
block that carries visible text must have its DOM parent's `color`. It runs in
`theme_switch_leaves_no_stale_label_colours`, for every panel and both
directions:
```
tabs dark→light: ["Tabs"]
history dark→light: ["Clear", "Open", "Remove"]
sites dark→light: ["Clear cache and parts"]
wallet dark→light: ["Export", "Remove", "New wallet", "Import", "Review…"]
console dark→light: ["Save replay log"]
(the same lists for light→dark)
```
The built-in-page test `host_theme_swap_leaves_no_stale_label_colours` fails
with `["lamp"]`.

**Decision (2026-10-04, by the user).**
- Fix it in the chrome: every label is an element (`<span>`), never bare text
  in a flex box (convention R1, enforced by the invariant tests).
- `apply_theme()` also clears the render cache, so dynamic regions are rebuilt.
- Blitz stays at the specification's pin. The engine defect is written up for
  upstream in
  [upstream/blitz-anonymous-block-restyle.md](upstream/blitz-anonymous-block-restyle.md),
  with a reproduction checked against blitz-dom on 2026-10-04. Filed as
  [DioxusLabs/blitz#1037](https://github.com/DioxusLabs/blitz/issues/1037). Pages whose
  f1r3lang changes a button's text colour remain affected until Blitz fixes
  it.

### Green result (2026-10-04)

**Fix applied.** Every label in the chrome and in `pages.rs` is now an
element: `<button><i …></i><span>Export</span></button>` and
`<button id="lamp"><span>lamp</span></button>`. `apply_theme()` also clears
the render cache, except the constant side controls.

**Regression tests:**

| Test | What it checks | Result |
|---|---|---|
| `theme_switch_leaves_no_stale_label_colours` | All 7 panels, dark→light and light→dark | `ok` |
| `host_theme_swap_leaves_no_stale_label_colours` | `gaze://newtab` after a host-theme swap | `ok` |
| `labels_are_never_bare_text_in_flex_boxes` | The R1 lint, over the live shell plus sample output of every builder | `ok` |

**Mutation check.** Only the wallet form's "Import" label was unwrapped
(`<span>Import</span>` → `Import`). Both tests fail and name exactly that
label; restored, both pass.
```
test chrome::tests::labels_are_never_bare_text_in_flex_boxes ... FAILED
test chrome::tests::theme_switch_leaves_no_stale_label_colours ... FAILED
#walletcard: ["Import"]          (R1 lint, once per wallet-region sample)
wallet dark→light: ["Import"]
wallet light→dark: ["Import"]
```

**Lint refinement found while turning it green.** The R1 lint first reported
every Font Awesome glyph (`\u{f252}`, `+`, …). Icon glyphs are generated
content (`::before { content: "\f252" }`), and Blitz restyles pseudo-element
boxes together with their element. The stale-colour test already passed for
them. Pseudo-element boxes are therefore excluded from the lint, as they
already were from `stale_text_colours`.

---

## L3 — Recently closed tabs dropped the newest

**Observation.** In `close()`, the code ran `closed.push(tab)` and then
`closed.truncate(20)`. `truncate` keeps the *oldest* twenty entries, so once
twenty tabs had been closed, each newly closed tab was discarded at once.
Ctrl+Shift+T then reopened old tabs instead of the most recent one.

**Fix.** After the push, the oldest entries beyond `CLOSED_LIMIT` are drained
from the front.

**Test.** `reopening_closed_tabs_starts_with_the_most_recent` closes 25 tabs.
It checks that the list keeps tabs p05…p24 and that restoring reopens p24.

## L4 — Text budgets: a tab spends 66 px, not 64, on everything but its title

**Observation.** `text_budgets_match_laid_out_boxes` lays the chrome out with
Blitz and compares each text box with the width its labels are fitted to. It
failed with: `.tab-title: Blitz lays it out 134 px wide, the budget assumes
136 px`.

**Cause.** The arithmetic left out `.tab`'s two 1 px (transparent) borders.

**Fix.** `geometry::TAB_CHROME` = 66 = padding 10 + 6, borders 2 × 1, icon 14,
two 7 px gaps, close button 20. The test now passes at 1280 px and 900 px
windows.

---

## L5 — Review of the first complete "after" capture

**Setup.**
- Release binary sha256 `c7faee73…a7027` (`run.txt`), all 59 after-scenes.
- Every enforced check passed (`checks.tsv`).
- Each scene was then inspected at 1× and, where needed, at 4–8× crops
  (`magick … -filter point -resize 400%`).

The review found thirteen problems the checks did not cover:

| ID | Where | Observation |
|---|---|---|
| R1 | notices, console lines, active row, active rail button, active tab, prompt bar | A faint line in the bar colour runs along edges that should be plain (top edge of the active row; top and right corners of notices) |
| R2 | harness, scene 50 | The transfer review never appears: the clicks on Recipient, Amount, and Review land 30 px below the fields |
| R3 | harness, scene 46 | "Row hover" hovers the *active* row, whose actions are always visible, so the scene shows no hover |
| R4 | address box | After Escape restores the address, the caret sits at character 3 (where "cap" ended), mid-word |
| R5 | address box | Focused and emptied, the box still shows the page's scheme icon, while its hint says "search tabs and history" |
| R6 | tab strip | With 16 tabs every tab, the active one included, is 97 px ("Fie…"). The plan says inactive tabs shrink to 44 px and the active tab keeps ≥ 128 px |
| R7 | History | The header says 8 while "Clear all 9 visits?" says 9: the list shows entries (one per address per group), the prompt counts stored visits |
| R8 | sidebar | Content starts at four different left edges: controls 52 px, rows 48 px, cards 50 px, wallet fields 52 px with wallet section heads at 60 px |
| R9 | Tabs and History search | A row can match in the part of its address that the ellipsis hides, so nothing shows why it matched ("gaze" → "Cross-site fetch", via `f1r3gaze-ui-snapshots`) |
| R10 | prompt bar | "wants to fetch from https://example.org:443": the default port shows |
| R11 | Site data | "Clear cache" is enabled on an empty cache |
| R12 | History search | Filtered, the header reads "1"; the Tabs panel reads "5 of 6" |
| R13 | address and find boxes | Ctrl+L and Ctrl+F focus the field without selecting its text, so typing is inserted into the old address or query |

### R1 — hairlines from inset box shadows

**Mechanism (source-verified, `blitz-paint/src/render/box_shadow.rs:89-147`).**
An inset shadow is painted in two steps:
1. Fill the padding box with the shadow colour.
2. Cut a "hole" out of it with `Compose::DestOut`. The hole is the padding box
   moved by the offset, drawn by `draw_box_shadow`, an analytic blurred-rect
   shader, at zero blur, with the *averaged* corner radius.

Where an edge of the hole coincides with an edge of the padding box, the
shader gives that edge about half coverage. About half of the bar colour
therefore survives as a one-pixel line. Corners whose radius differs from the
average leave slivers.

**Measurement.** `edge_artifact`
(`scripts/ui-snapshots.sh`) takes a 5 px band across an edge. It uses two
colours:
- A, the outside colour: the commonest colour of the band's first row.
- B, the inside colour: the commonest colour of the band's last row.

It counts pixels farther than 8 (on the 0–255 RGB scale) from the segment
A–B. Anti-aliasing between two fills only produces colours on that segment.

| Scene | Edge | Off-blend pixels |
|---|---|---|
| `16-appearance-light` | notice, top | 220 of 220 |
| `17-appearance-bad-palette` | notice, top | 220 of 220 |
| `02-tabs-light` | active row, top | 150 of 150 |
| `02-tabs-light` | active rail button, top | 16 of 16 |
| `01-tabs-dark` | active row, top | 150 of 150 |
| *control:* `02-tabs-light` | active row, bottom (offset moves the hole away) | 0 |
| *control:* flat card interior, inactive rows | — | 0 |

Sampled pixels match the mechanism:
- Notice top: `#78B8BD`, about 50 % of accent `#087F86` over `#F5F7FA`.
- Active row top: `#6DB1B7`, not a blend of the outside `#FFFFFF` and the
  inside `#DDE9ED`.

**Hypothesis H-R1.** A bar painted as a hard-stop `linear-gradient`
background has no hairline. That background is one fill inside one clip
(`render/background.rs:134-166`); no second shape is subtracted along the
same edge.

**Prediction.** Every band above measures 0, the controls stay 0, and the
bars keep their width and colour.

**Experiment 1 (H-R1): hard-stop gradient.** The background became
`linear-gradient(to right, bar 3px, fill 3px)`.

| Band | Before | After |
|---|---|---|
| Notice top | 220 | 0 |
| Active row top | 150 | 0 |
| Rail top | 16 | 0 |

The hairlines are gone, so H-R1 holds for the edges. The second half of the
prediction failed: the bars did not all keep their width. Pixels sampled across
each bar:

| Bar | Gradient length *L* | Bar colour run |
|---|---|---|
| Active row | 287 px (horizontal) | 4 px, `x` = 48…51 |
| Active rail button | 34 px (horizontal) | 4 px |
| Notice | 261 px (horizontal) | 3 px |
| Active tab | 32 px (vertical) | 2 px |

**Mechanism of the width error (source-verified).**
- Vello's fine stage samples gradients at integer pixel corners
  (`vello_shaders-0.10.0/shader/fine.wgsl:1076`, `:1203`).
- It looks the colour up in a 512-entry ramp at index $`\mathrm{round}(511\,t)`$
  (`fine.wgsl:26`, `:1225-1236`; `vello_encoding-0.10.0/src/ramp_cache.rs:12`).
- A hard stop at $`s`$ px is therefore quantized to steps of $`L/511`$ px.
- The pixel just past the stop takes the bar colour exactly when the ramp entry
  it lands on still lies before the stop:

```math
\text{bar}(k) \iff \frac{1}{511}\,\mathrm{round}\!\left(\frac{511\,k}{L}\right) < \frac{s}{L}
```

  Here $`k`$ is the pixel's distance in px from the start of the gradient.

The rule predicts all four observations:

| Bar | $`k`$, $`s`$, $`L`$ | $`511k/L`$ | Ramp entry | Compared with $`s/L`$ | Prediction |
|---|---|---|---|---|---|
| Row | $`k=3,\ s=3,\ L=287`$ | 5.34 | 5 → 0.009785 | < 0.010453 | bar → 4 px |
| Notice | $`k=3,\ s=3,\ L=261`$ | 5.87 | 6 → 0.011742 | > 0.011494 | fill → 3 px |
| Rail | $`k=3,\ s=3,\ L=34`$ | 45.09 | 45 → 0.088063 | < 0.088235 | bar → 4 px |
| Tab | $`k=2,\ s=2,\ L=32`$ | 31.94 | 32 → 0.062622 | > 0.0625 | fill → 2 px |

On the 1280 px prompt bar the step is 2.5 px, so a 3 px bar could have come
out anywhere from 2 to 5 px.

**Hypothesis H-R1b.** A bar painted as a solid image the size of the bar,
`linear-gradient(c, c) 0 0 / 3px 100% no-repeat` over the background colour,
is exactly 3 px wide and has no hairline:
- A constant ramp has no stop to quantize.
- The bar's edges are the geometry of a rectangle.
- The fill is one clipped layer per edge.

**Experiment 2 (H-R1b).** The convention is now R8 (`chrome.rs`, `CSS`).
Harness checks `*_edge_artifact_px` (`le 0`) and `*_bar_px` (`eq`): pixels of
the bar colour along a one-pixel line across the bar.

| Scene | Check | Result |
|---|---|---|
| `02-tabs-light` | row / rail edge artifacts | 0 / 0 |
| `02-tabs-light` | row / rail / tab bar widths | 3 / 3 / 2 |
| `16-appearance-light` | notice edge artifact, bar width | 0, 3 |
| `57-theme-prompt-fresh-light` | prompt bar width | 3 |

**Verdict.** H-R1b is supported, and every bar is its specified width. The
`.row-actions` fade remains a smooth gradient: quantizing a smooth gradient is
invisible. Inside a card, a notice is now filled with `--gaze-surface`, so the
callout stands out from the card around it.

Both engine defects are written up for upstream, with a reproduction page
whose measurements match the chrome's:
- [upstream/blitz-inset-box-shadow.md](upstream/blitz-inset-box-shadow.md),
  filed as [DioxusLabs/blitz#1038](https://github.com/DioxusLabs/blitz/issues/1038).
  The source review for it also found that the inset path does not scale its
  offset by the device pixel ratio.
- [upstream/vello-gradient-quantization.md](upstream/vello-gradient-quantization.md),
  filed as [linebender/vello#1975](https://github.com/linebender/vello/issues/1975).

### R2–R13

| ID | Cause | Fix | Regression evidence |
|---|---|---|---|
| R2 | Calibrated coordinates went stale when "Copy address" became "Copy" (the card lost a line, 30 px) | Recalibrated (`WALLET_TO` 190 332, `WALLET_AMOUNT` 120 392, `WALLET_REVIEW` 259 431). The scene now checks that each step changed the screen: `recipient_px`, `amount_px`, `review_px` (`ge 1`) | after: 1240 / 1050 / 16081. The same checks showed the **before** coordinates had always missed: `recipient_px` = 0, `review_px` = 0, because the clicks hit Export on the wallet row. Recalibrated to the Send row (y = 749): 250 / 429 / 6405 |
| R3 | The scene hovered the first row, which is the active tab | `SECOND_ROW`, an inactive tab (before 180 273, after 170 234). Check `hover_px` `ge 1` | after 1238, before 917 |
| R4, R13 | parley's `PlainEditor::set_text` clamps the old selection to the new text (`editor.rs:951-967`). Ctrl+L and Ctrl+F only moved focus | `select_pending`: Ctrl+L, Escape (while focused), and Ctrl+F select the whole field after the next render, through `BaseDocument::with_text_input` with `select_byte_range(len, 0)` (caret at the start, so a long address shows its beginning) | `focusing_or_reverting_a_field_selects_its_text`; scene 36 |
| R5 | The badge keyed off "suggestions are open", which is false for an empty query | The badge is "searching" when the box is focused and holds anything but the page's address (an empty box included) | `the_badge_turns_into_a_magnifier_when_the_address_is_edited`; scene 33 |
| R6 | The implementation used a uniform 96 px minimum instead of the planned 44 px inactive / 128 px active | `strip_layout` follows the plan. An inactive tab's close button overlays its title (`TAB_CHROME_INACTIVE` = 39, checked against Blitz's layout). A tab with no room for any title shows only its icon (`.tab.narrow`). Scene 25 uses 30 tabs, more than fit even at 44 px | `the_strip_keeps_the_active_tab_readable_and_shrinks_the_rest` (properties over 6 widths × 60 counts × 3 positions), `crowded_tabs_shrink_to_their_icons`, `text_budgets_match_laid_out_boxes` |
| R7, R12 | The header counted entries; the prompt counted stored visits. A filter showed only the match count | Both count entries. A filter reads "N of M", as in Tabs. The prompt says the whole history is cleared | `history_groups_by_recency_and_keeps_each_address_once_per_group` |
| R8 | Each region had its own padding (10 / 6 / 6 + 2 / 10 px) | Convention R7: one 6 px gutter. Boxes (rows, cards, notices, the controls bar) span it; headings, labels, notes, and in-flow controls start 8 px further in (`.form`). `card_inner()` lost its wallet case | `text_budgets_match_laid_out_boxes` (card content widths); scenes 09–12 |
| R9 | Rows were filtered by `search_score`, but the cut hid the matched text | `ui_state::match_span`: the same rules as `search_score`, as a byte range. `find_ignoring_case` aligns `str::to_lowercase` with the original characters (final sigma, `İ`). `TextFitter::{end_marked, url_marked, url_marked_in_place}` keep the match visible with a window `…context match context…`. Each cut now returns what it kept (`Kept`), which also shrinks the memo. `fit_detail`: the address keeps its usual form when the title already shows the match. Matches are marked with an accent underline and text colour, which leave fitted widths unchanged | `match_spans_point_at_the_original_characters`, `a_row_is_highlighted_exactly_when_it_matches` (consistency with `search_score`), five `text_fit` tests including a property test, `search_results_show_why_they_matched`; scenes 23, 26, 34 |
| R10 | The broker names sites canonically (`https://example.org:443`) | `display::without_default_ports` on the prompt text. The broker keeps the canonical text (its answers use the structured `Held::Net { site }`) | `default_ports_are_dropped_from_origins_in_text`, `prompts_drop_default_ports`; scene 39 |
| R11 | `.btn:disabled` gave every button a border, so a disabled ghost button looked more prominent than an enabled one | Disabled buttons use `--gaze-border` text, like disabled menu items, segments, and icon buttons. Disabled ghost buttons stay borderless | scene 05 ("Clear cache" on an empty cache) |

**Mutation checks (2026-10-04).** A script backed up `chrome.rs`, undid one
fix with `sed`, ran that fix's test, and restored the file from the backup.
The file was byte-identical after every restore. The edits, in `sed` syntax on
`crates/gaze-shell/src/chrome.rs`:

```text
M1  s#^            changed |= self.select_whole(id);#            // changed |= self.select_whole(id);#
M2  s#let badge = scheme_badge_html(page, searching);#let badge = scheme_badge_html(page, typing);#
M3  s#plural(total, "entry", "entries")#plural(visits.len(), "visit", "visits")#
M4  s#match_span(query, row.title), width#None, width#
    s#let mark = match_span(query, url);#let mark: Option<std::ops::Range<usize>> = None;#
    s#match_span(query, title), width#None, width#
    s#match_span(&query, title), width#None, width#
M5  s#rest = escape(&display::without_default_ports(rest)),#rest = escape(rest),#
M6  s#let first = TAB_ACTIVE_MIN.min(room);#let first = equal;#
```

M4 is listed in its final form. It was first run before `fit_detail` existed,
with the address marks removed in the row builders directly, and was re-run
against the final code with the edits above: red, then green after the
restore.

| Mutation | Test | Result |
|---|---|---|
| M1: select-all not applied | `focusing_or_reverting_a_field_selects_its_text` | red: "Ctrl+L left: None" |
| M2: badge keyed off `typing` | `the_badge_turns_into_a_magnifier_…` | red: "emptied: a search" |
| M3: prompt counts `visits.len()` | `history_groups_by_recency_…` | red |
| M4: no `match_span` in rows | `search_results_show_why_they_matched` | red |
| M5: prompt text unprocessed | `prompts_drop_default_ports` | red: shows `:443` |
| M6: active tab gets the equal share | `the_strip_keeps_the_active_tab_readable_…` | red: 75.0 ≠ 128 |

The first M6 attempt (`(false, _) if false`) did not compile. The first
version of the script reported that as "GREEN" because no FAILED line
appeared. The script now classifies every run as red, GREEN, or
did-not-compile, and M6 was redone as above.

**Harness defects found while adding the checks.**
- Visit times were relative to the start of the run (`NOW` was set once), so
  a scene seeded minutes later showed different relative times. Times are now
  relative to each scene's seeding. This was required before scene 49 could be
  compared with scene 03 pixel for pixel: `sidebar_px_vs_03`, after 0 (the
  panel opens unfiltered), before 4416 (S9).
- Panel scenes now park the pointer on the status bar before the capture.
- Other before values recorded by the new checks: `bubble_px` = 0 (no hover
  labels, G2) and `flash_cleared_px` = 0 (the status message never expires,
  G5).

**Captures.**
- The pre-fix "after" capture is kept as evidence in
  [`../screenshots/ui/review-1/`](../screenshots/ui/review-1/): the fifteen
  scenes cited above, with its `run.txt` and `checks.tsv`.
- [`after/`](../screenshots/ui/after/) and
  [`before/`](../screenshots/ui/before/) were both re-captured with the final
  harness: 59 and 60 scenes, 0 failed.
- All 27 after checks pass.
- *Later the same day:* scene 60 gained an after-mode path (see L7's capture).
  After that, `after/` holds 60 scenes and 28 checks, all passing.

---

## L6 — Audit defects X1, X2, X6

These were found by reading the chrome while the plan was drafted (X1, X2) and
while the wallet panel was rebuilt (X6). Each fix is behaviour, not styling.

| ID | Observation | Cause | Fix | Evidence |
|---|---|---|---|---|
| X1 | After "Remember for this site" was ticked for one prompt, every later prompt was remembered too | `remember` was never reset | `Action::Answer` reads `remember` for this answer only, then resets it | `remember_applies_to_one_answer` |
| X2 | "End" on a live shard session in Site data closed a session on the **active** tab's shard connection, which need not own it | The verb carried only the session label and acted on `self.tabs[active]` (old `chrome.rs`, `site_op`) | The verb is `site:session:<tab id>:<label>`. `session_target` splits it, and the session closes on the tab that owns it | `small_pure_helpers` (`session_target`, including labels containing `:`). There is no behavioural test: it needs a live shard session, which the unit-test profile does not start (`observers =`). The change is checked by reading the source against the old `chrome.rs` |
| X6 | A profile reopened on the Wallet panel showed wallets without balances until the panel was opened again | Balances were fetched only by the panel-open action and after wallet operations | `ChromeDocument::new` fetches them when the restored sidebar shows the Wallet panel | Scenes `11-wallet-one-dark`: "Balance 1,250,000" at start (after). In `before/11` there is no balance |

---

## L7 — Workspace lint cleanup (decision 8)

**Goal.** `cargo clippy --workspace --all-targets --locked -- -D warnings`
passes on the whole workspace, through behaviour-neutral edits, and CI builds
with the declared `rust-version` (1.95).

**Baseline (2026-10-04).** 34 warnings:

| Lint | Count | Fix |
|---|---|---|
| `collapsible_if` | 23 | let chains (edition 2024) |
| `type_complexity` | 3 | type aliases |
| `manual_is_multiple_of` | 3 | `is_multiple_of` |
| `inherent_to_string` | 1 | see below |
| `identity_op` | 1 | see below |
| `unnecessary_to_owned` | 1 | — |
| `field_reassign_with_default` | 1 | a struct initialiser |

How the fixes were made:
- **Machine-applicable fixes** were applied with `cargo clippy --fix`. Its
  let chains keep the inner bodies one level too deep. Only the lines the fix
  added were re-indented to rustfmt's layout (`{` on its own line). The
  workspace is not rustfmt-clean, so formatting whole files would have changed
  code nobody touched.
- **Type aliases.**
  - `gaze_dom_core::BatchRefs<N>` for `apply_batch`, also used by its two
    implementations.
  - `RecordedDispatch` in `gaze-exec`.
  - `Finder<B>` in the conformance test.
- **`identity_op`.** `| 0` in the protobuf key was dropped; a comment now
  states the wire type it encoded (0, varint).
- **`inherent_to_string`.** `J::to_string` in `gaze-reach` was replaced by a
  `Display` impl that writes the same text. The old method is kept commented
  out, with the reason.
- **Toolchain.** CI pins in `ci.yml`, `reach.yml`, and `release.yml` were
  raised from 1.91 to 1.95. Release builds failed the same way as CI on 1.91.
- **CI gate.** `ci.yml` gained a `lint` job on Ubuntu, with Rust 1.95 and
  clippy, running `cargo clippy --workspace --all-targets --locked -- -D
  warnings`. A new warning now fails CI instead of accumulating. It is
  Linux-only because that is where the clean state was verified:
  `cargo +1.95 clippy … -D warnings` passes locally. macOS- and Windows-only
  code is checked by the existing test matrix's builds, not by clippy.

**Verification.**
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean.
- With Rust 1.95 (`cargo +1.95`, a separate target directory):
  - the workspace and its tests build;
  - `cargo test --workspace --locked`: 33 test binaries, 140 tests, 0 failed;
  - the CI smoke test (`--headless gaze://newtab --click "#lamp"`) prints
    `id="lamp" class="lit"`.
- **Behaviour-neutral, measured.** The full "after" capture was taken with the
  binary from before the lint edits (sha256 `7d77bea2…`) and with the binary
  after them (`11245cc1…`). All 59 scenes common to both are pixel-identical
  (`magick compare -metric AE` = 0 for every scene).
- **The final capture.** A last edit corrected R3's comment in the stylesheet
  string, which changes the binary but not the rendering. The release binary
  was rebuilt from the final source (`635ed212…`) and the after capture taken
  again: 60 scenes, 28 of 28 checks pass, and all 60 images are pixel-identical
  to the `11245cc1…` capture.

---

## L8 — The cursor disappears over pages, and links show no hand

**Report (user, 2026-10-04).** "The mouse cursor tends to disappear over the
content pane after clicking left panel tab icons or other buttons like the
back button; the cursor does not become a pointer over clickable elements in
the content pane."

**Mechanism, from the sources (Blitz `674d7d2`).** The design that resulted is
in [README §3.4](README.md#34-the-pointer-over-pages-cursor-and-hover).
1. **The chrome never asks again.** The chrome's hit test stops at a page's
   host `.view` (`node/node.rs` `hit_inner` walks only the chrome's own boxes).
   `set_hover_to` (`document.rs:1811-1888`) calls
   `shell_provider.set_cursor(get_cursor())` only when the hovered or hit node
   changes. Moving inside a page changes neither.
2. **The cursor comes from before the event.** `EventDriver::handle_ui_event`
   (`events/driver.rs:148-269`) updates hover, and with it the cursor, *before*
   it dispatches the event. That dispatch is what forwards it to the page
   (`events/mod.rs:119`).
   - `get_cursor` (`document.rs:2147-2199`) delegates to the page's
     `get_cursor()`, which still describes where the pointer was when the page
     last saw it.
   - A page that has never seen the pointer has no hover node, so the answer
     is `None`. `BlitzShellProvider::set_cursor(None)` hides the cursor
     (`blitz-shell/src/lib.rs:101-111`).
3. **Pages cannot report.** `Tab::config` left `shell_provider` unset, so pages
   got `DummyShellProvider` (`document.rs:427-429`). Their own `set_cursor` and
   `request_redraw` calls were dropped.
   - Sub-documents get no enter or leave events (`events/mod.rs:53-58`), and
     nothing calls a page's `clear_hover`.
   - `resolve` ends with `refresh_hover` (`resolve.rs:138`), which re-resolves
     hover at the last pointer position. The chrome's runs before its pages
     are resolved.

**Defects predicted from the mechanism.**

| ID | Defect |
|---|---|
| D1 | The cursor never follows page content: no hand over links |
| D2 | The cursor is hidden on entering a page that has not seen the pointer, and stays hidden while the pointer is in the page |
| D3 | A page's `:hover` state outlives the pointer's visit |
| D4 | A page's hover restyles are not painted. The page's redraw request goes to the dummy, the chrome's hover does not change, and `RhoDocument::drive` reports only f1r3lang frame changes |

**Hypotheses, experiments and verdicts.** The experiments ran 2026-10-04,
first through the chrome with a recording window provider, then on the
release binary under Xvfb.

| ID | Hypothesis | Prediction | Experiment | Raw result | Verdict |
|---|---|---|---|---|---|
| H1 | D2 | Entering a never-hovered page records `set_cursor(None)`, then nothing | `entering_a_fresh_page_never_hides_the_cursor`, unfixed. Click Back, then move into the page Back loaded | The window was asked for `[Some(Pointer), None]`: the hand over Back, then hidden | **Supported** |
| H2 | D1 | Moving from a plain block to a link records no call, while the page itself answers `Pointer` | `the_cursor_follows_links_and_text_inside_a_page`, unfixed | Control `page_cursor == Some(Pointer)` passed. The window was asked for `[None]` on entering, and nothing after the move to the link | **Supported** |
| H3 | D3 | After the pointer moves to the rail, the page's hover is still the link | `leaving_a_page_ends_its_hover`, unfixed | Control: `#go` background `rgb(204, 0, 0)` while hovered (passed). After the move: `left: Some("go") right: None` | **Supported** |
| H4 | D4 | Moving onto a link with a `:hover` rule requests no redraw, and `poll` reports no change | `hover_changes_inside_a_page_are_repainted`, unfixed | `0 requests, poll() = false` | **Supported** |
| R1 | The cursor is hidden by CSS | Some chrome or page style has `cursor: none` | `grep cursor` in the chrome's and pages' CSS | No `cursor: none` anywhere | **Refuted** |
| R2 | winit or the compositor hides the cursor | The provider would not be asked for `None` | H1's log at the provider boundary | `None` is requested exactly on entering the page | **Refuted** |
| R3 | The page computes the wrong cursor | The page's own `get_cursor()` would not be `Pointer` over a link | H2's control | `Some(Pointer)` | **Refuted** |

**Red, end to end.** The harness's new cursor scenes ran on the release
binary before the fix (sha256 `635ed212…ba30`, the binary of the committed
`after/` capture). Each scene reads the X cursor's sprite through XFixes
(`scripts/x-cursor.py`) and compares it with reference sprites taken in the
same run. The references were: the hand over Reload (24×24, 335 opaque px),
the text cursor over the address field (24×24, 233 px), and the arrow over the
rail (24×24, 272 px).

| Scene | Interaction | Cursor before the fix |
|---|---|---|
| 61 | Back, then into the page | 1×1 sprite, 0 opaque px: **hidden** |
| 62 | a rail button, then into the page | hidden |
| 63 | a Tabs-panel row, then into the page | hidden |
| 64 | onto the page's link | hidden (no hand) |
| 65 | onto the page's text | hidden |
| 66 | onto the link, then out to the rail | `:hover` fill 0 px while hovered, **18 000 px after leaving** (D3 and D4 together) |
| 67 | a page loads under the resting pointer | arrow, where the new page has its link |
| 68 | onto `cursor: none`, then off it | hidden, and still hidden after leaving |

The first red run ([`cursor-red`]) put scene 65's hand over plain text. The
window had opened under the pointer where the previous scene left it, so the
page was hovered at that spot and the hover was never cleared (D3). Entering
the page later then showed the cursor of that old spot (mechanism 2). This is
the second failure mode the mechanism predicts: a stale cursor rather than a
hidden one. The harness now parks the pointer on the status bar before every
launch, so no scene depends on the one before it. The deterministic red run
(`cursor-red-2`) is the table above. The raw logs are kept in the session's
scratch directory (`target/scratch/session-28795e59/cursor/`).

Scene 67's arrow, rather than a hidden cursor, probably came from typing the
address: the suggestions dropdown moved the chrome's hover off the page and
back, so the old page's cursor was asked for. That path was not measured
separately. The check's prediction, "not the hand", holds either way.

### Fix

`crate::cursor` makes one place decide the cursor, after the page has seen the
event (README §3.4):
- **`WindowShell`** wraps blitz-shell's provider. It forwards every request
  except `set_cursor`, which becomes a request to recompute.
- **`PageShell`** is every page's provider (through `Tab::config`). It reports
  hover changes and redraw requests, and grants nothing else.
- **`CursorArbiter::sync`** clears the hover of the page the pointer left, and
  tells the page it entered where the pointer is. It then computes
  `window_cursor(get_cursor(), hidden_by_css)` and sends the result to the
  window only when it changes. Blitz's `None` hides the cursor only for
  `cursor: none`.
- **`page_attached`** and **`view_removed`** cover a page loaded under the
  pointer and a closed tab.
- Built-in pages give buttons `cursor: pointer`. Blitz's default sheet sets
  none, which left a text cursor over the lamp's label (mutation M7 below).

Two tests were added after the fix, for paths the first set did not reach:
- L8j `switching_tabs_under_a_resting_pointer_shows_the_new_page_cursor`
  (seeding on a host change without an event);
- a stronger L8e that also asserts the cursor is never hidden while a page
  loads (the `None` rule before the page's first layout).

Their red evidence is mutations M4b and M3.

### Green (2026-10-04)

- `cargo test -p gaze-shell -p gaze-dom-blitz`: 87 + 8 passed. That is 73
  before, plus 10 L8 behaviour tests, plus 4 unit tests in `cursor`.
- The cursor scenes on the fixed binary (`62f4282b…`), all checks pass:

  | Scene | Cursor after the fix |
  |---|---|
  | 61, 62, 63 | the arrow, visible (272 opaque px) |
  | 64 | the hand |
  | 65 | the text cursor |
  | 66 | `:hover` fill 18 000 px while hovered, 0 after leaving |
  | 67 | the hand, with no pointer movement |
  | 68 | hidden over `cursor: none` (0 px), the arrow again after leaving |

### Mutation checks

Each check commented out one part of the fix, ran the L8 tests and the
`cursor` unit tests, and restored the file byte for byte (`cmp`). The script
and the raw output of every run are in the session's scratch directory
(`cursor/mutations/`).

| ID | Part commented out | Tests that went red |
|---|---|---|
| M1 | `WindowShell` lets Blitz's own cursor through | L8c (re-entry after leaving: a stale `None` stood, because the arbiter believed the window already showed the arrow), L8e, L8g, L8j, `window_shell_forwards_everything_but_the_cursor` |
| M2 | pages keep `DummyShellProvider` | L8b, L8d, L8e, L8f |
| M3 | Blitz's `None` always hides | L8e (hidden while the new page loaded), `blitz_none_hides_the_cursor_only_for_css_none` |
| M4a | no seeding when a page attaches under the pointer | L8e |
| M4b | no seeding when the hovered page changes without an event | L8j |
| M5 | no clearing when the pointer leaves a page | L8c, L8j |
| M6 | a page's redraw requests are dropped | L8d, `page_shell_only_reports` |
| M7 | built-in buttons without `cursor: pointer` | `built_in_buttons_show_the_hand`. The window was asked for `[Some(Default), Some(Text)]`: Blitz gives a text cursor over the label |

### Regression

The plan was to compare a fixed-binary run with the committed `after/`
capture. That comparison would have mixed two variables. The harness's work
directory moved off `/tmp` in the same change (README §12.2), and the demo
site's `file://` URLs, which the captures show, moved with it. So both
binaries were instead run through the full suite with the same harness and
the same work directory, and only the binary differs:
- **B0**, before the fix: `635ed212…`.
- **B1**, after it: `62f4282b…`.

| Measure | B0 (before) | B1 (after) |
|---|---|---|
| Scenes run, failed | 68, 0 | 68, 0 |
| The 28 checks of scenes 01–60 | 28 pass | 28 pass |
| The 21 cursor checks | 8 pass (the reference checks, and `hidden_opaque_px` by accident) | 21 pass |

Scenes 01–60 are pixel-identical between B0 and B1 (`magick compare -metric
AE` = 0 for all 60). So are cursor scenes 61, 62, 63, 65 and 68. Three cursor
scenes differ, and each differs by exactly the link's `:hover` fill. It is a
320×80 box of `#CC0000`, 25 600 px:

| Scene | Changed region | `#CC0000` px, B0 → B1 | Why |
|---|---|---|---|
| 64 (on the link) | `320x80+242+264` | 0 → 25 600 | the hover is now painted (D4) |
| 66 (left the page) | `320x80+242+264` | 25 600 → 0 | the stale hover is now cleared (D3) |
| 67 (loaded under the pointer) | `320x80+242+144` | 0 → 25 600 | the new page is now told where the pointer is, and painted |

In B0, scene 66 shows D3 and D4 at once. The fill is not painted while the
link is hovered, and it is painted after the pointer has left.

### Upstream

The defect is in the browser's own code: pages were left with Blitz's dummy
provider (mechanism 3). The two facts behind it, though, are Blitz's own, and
they affect any embedder of sub-documents, iframes included:
- a sub-document's hover is never ended (mechanism 1);
- the parent reads the sub-document's cursor before the event reaches it
  (mechanism 2).

They were reproduced without F1R3Gaze by a headless program on blitz-dom's
public API ([`upstream/repro-subdocument-hover.rs`](upstream/repro-subdocument-hover.rs)).
The output was identical at the pin `674d7d2` and on `main` at `0db8c74`:
- the child keeps `:hover` after the pointer leaves its host;
- a relaid-out child sends `set_cursor(None)` while the pointer is over the
  parent;
- entering asks for the child's pre-event cursor (`None`) first.

After a duplicate search of open and closed issues and pull requests, which
is recorded in [`upstream/README.md`](upstream/README.md), it was filed as
[DioxusLabs/blitz#1040](https://github.com/DioxusLabs/blitz/issues/1040)
([report](upstream/blitz-subdocument-hover.md)).

### Not fixed here (noted)

- blitz-shell ignores `PointerLeft` for the mouse (`window.rs:701-734`). So
  the chrome's hover outlasts the pointer leaving the window, and with it the
  hover of the page under it. A toolbar button or a page link the pointer was
  on stays in its `:hover` style until the pointer comes back, and the next
  move inside the window resets both. This is blitz-shell's behaviour for any
  document, not part of L8. It is not filed.
- Blitz's default style sheet sets no `cursor` for buttons, so a button's
  label on an arbitrary page shows the text cursor. Built-in pages set
  `button{cursor:pointer}` (M7). Other pages get what their own CSS asks for.
