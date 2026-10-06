# Engine defects found while building the chrome

The chrome works around five engine defects, and lives with two more,
instead of patching pinned dependencies (design decision 5;
[ledger](../ledger.md) L2, L5, L8 and L18). Each
report below is written to be filed upstream as it stands. Each one contains:
- the versions;
- a minimal reproduction;
- what happens and what should happen;
- the cause, traced to source lines;
- a suggested fix;
- the workaround F1R3Gaze uses meanwhile.

| Report | Filed as | Status in F1R3Gaze |
|---|---|---|
| [Anonymous blocks keep a stale text colour](blitz-anonymous-block-restyle.md) | [DioxusLabs/blitz#1037](https://github.com/DioxusLabs/blitz/issues/1037) | worked around by convention R1 |
| [Inset box-shadows: hairline edges, averaged radius, unscaled offset](blitz-inset-box-shadow.md) | [DioxusLabs/blitz#1038](https://github.com/DioxusLabs/blitz/issues/1038) | worked around by convention R8 |
| [Gradients sampled at pixel corners through a 512-entry ramp](vello-gradient-quantization.md) | [linebender/vello#1975](https://github.com/linebender/vello/issues/1975) | worked around by convention R8 |
| [A sub-document keeps its hover after the pointer leaves its host, and still sets the window's cursor](blitz-subdocument-hover.md) | [DioxusLabs/blitz#1040](https://github.com/DioxusLabs/blitz/issues/1040) | worked around by the cursor arbiter (README §3.4) |
| [A document whose scale changes keeps the border widths of its first scale](blitz-scale-change-border-widths.md) | [DioxusLabs/blitz#1076](https://github.com/DioxusLabs/blitz/issues/1076) | the snapshot harness compares a window with itself at a zoom (storage ledger S13 part 2, E3) |
| [`<meta name="color-scheme">` is ignored](blitz-meta-color-scheme.md) | [DioxusLabs/blitz#1077](https://github.com/DioxusLabs/blitz/issues/1077) | pages get `:root{color-scheme:light}` unless their CSS says otherwise (chrome ledger L13) |
| [The canvas and the default text colour ignore the root's colour scheme](blitz-canvas-color-scheme.md) | [DioxusLabs/blitz#1078](https://github.com/DioxusLabs/blitz/issues/1078) | none: a dark page with no background of its own stays black on white (chrome ledger L13, "Not fixed here") |

[`repro-paint.html`](repro-paint.html) holds both paint reproductions: three
gradient bars and the inset-shadow box. Its measurements, taken with the
release binary on 2026-10-04, are quoted in each report.
[`repro-subdocument-hover.rs`](repro-subdocument-hover.rs) is the sub-document
reproduction, a headless program on blitz-dom's public API. Its output was
identical at the pin `674d7d2` and on `main` at `0db8c74`.

All four were filed on 2026-10-04, after searching the trackers for
duplicates. Related reports are cited in the issues: DioxusLabs/blitz#349 and
linebender/vello#1245 (single corner radius), and linebender/vello#1058 (the
gradient lookup table). The issue text is each file here without its title,
with the local links written out and a closing note on where the defect was
found. #1040 carries its reproduction program inline. When one of the issues
is fixed and the pins are raised, the corresponding workaround can be
revisited.

## The duplicate search for #1040

The search covered DioxusLabs/blitz, open and closed, issues and pull
requests, before filing. The queries were run with `gh search issues
--include-prs`:
- iframe hover;
- subdocument hover; sub-document hover; sub document;
- hover stuck; hover not cleared; sticky hover; hover state iframe;
- iframe cursor; cursor subdocument; cursor hidden; cursor disappears;
- mouseleave iframe; pointerleave; mouse leave; leave event;
- clear_hover; refresh_hover; subdoc; set_cursor;
- `hover`, `cursor` and `iframe` on their own, listing every result;
- immediately before filing: the 40 newest issues and 30 newest pull
  requests, and hover iframe, hover subdocument, hover leave, hover cleared,
  hover remains, cursor iframe, cursor sub-document, clear_hover subdocument,
  pointerleave subdocument.

The closest results were read in full and rejected:

| Candidate | Why it is not the same defect |
|---|---|
| #480 (open) "blitz-shell: CursorMoved doesn't request_redraw, so DOM-driven drags lag" | About repainting after a DOM mutation during a drag, which #580 addressed. Nothing about sub-documents or hover. |
| #257 (open) "Tracking: Event handling" | Tracks which DOM events scripts can handle. It does not cover forwarding to sub-documents. |
| #546 (merged) "Update hover/active/focus state when the referenced node is removed" | Hover after a node is *removed* within one document. |
| #635 (merged) "Implement support for `<iframe>` elements" and #743 (merged) "Generalize sub-documents…" | They introduced sub-documents. Neither forwards leave events or clears a sub-document's hover. |
| #985 (merged) "Propagate :hover/:active along DOM ancestors…" | Hover propagation within one document. |
| #483 (closed) "cursor: none should be supported" | Added the hiding of the cursor for `cursor: none`. |
| #119 (open) "Roadmap", #363 (open) "Browser UI features" | Mention iframes and per-tab hover state, but not this defect. |

The source of `main` at `0db8c74` was then checked, and the reproduction run
against it, before filing.

## The duplicate search for #1076

Run on 2026-10-06 over DioxusLabs/blitz, open and closed, issues and pull
requests (`gh search issues --include-prs`):
- zoom; zoom layout; zoom rounding; zoom font; zoom changes layout; zoom
  border; zoom layout differs; zoom not applied; zoom_to;
- scale factor layout; scale factor change; scale_factor; hidpi scale
  change; set_hidpi_scale; ScaleFactorChanged; monitor scale; DPI change;
  device pixel ratio; devicePixelRatio;
- border width zoom; border-width zoom; border width; border-width; border
  snapping; snap border; border rounding;
- restyle zoom; restyle device change; restyle scale; recascade; device
  changes; set_stylist_device; flush_pending_device_changes;
- immediately before filing: the 40 newest issues and 30 newest pull
  requests.

The closest results were read in full and rejected:

| Candidate | Why it is not the same defect |
|---|---|
| #902 (open) "Fix 1px borders vanishing when one side is zero-width or at fractional scales" | Rounds the layout of one scale to the device grid; it confirms that Stylo snaps border widths to device pixels, but says nothing of a scale that changes. |
| #837 (open) "1px borders sometimes not rendering" | The defect #902 fixes: a zero-width side and fractional-scale rounding at one scale. |
| #784 (merged) "Fix zoom_by/zoom_to skipping inline-context invalidation and redraw" | Invalidates inline layouts on zoom; already in the pin, and the reproduction has no text. |
| #782 (merged) "Avoid full restyle on viewport resize" and #785 (merged) "Coalesce stylist device changes…" | Where the full restyle on every device change stopped, keeping a recascade for colour-scheme changes only: the cause, cited in the report. |

The reproduction was run at the pin `674d7d2` and on `main` at `2335458`
before filing, with the same output, and the suggested fix was tried on a
clone of `main`: every way printed the same, and `cargo test -p blitz-dom`
passed.

## The duplicate search for #1077

Run on 2026-10-06 over DioxusLabs/blitz, open and closed, issues and pull
requests (`gh search issues --include-prs`): color-scheme; color scheme;
prefers-color-scheme; meta color-scheme; dark mode; meta tag; meta name;
color scheme meta; supported color schemes; light dark; light-dark; system
colors; CanvasText; theme-color; head meta. None is about the `<meta>`: the
`meta` hits are the WPT runner's `meta name=fuzzy` (#669, #818), and the
others are unrelated pull requests that mention colours. The reproduction was
run at the pin `674d7d2` and on `main` at `2335458` before filing, with the
same output, and `packages/` on `main` was searched for any reading of the
`<meta>`.

## The duplicate search for #1078

Run on 2026-10-06 over DioxusLabs/blitz, open and closed, issues and pull
requests (`gh search issues --include-prs`): canvas background; canvas color;
root background; background of the root; transparent background; Canvas
system color; default background; white background; initial color; default
text color; dark background text; color-scheme dark canvas; canvas surface.
The closest were read in full:

| Candidate | Why it is not the same defect |
|---|---|
| #1011, #672, #640 (open) | They paint the root's or the `<body>`'s background, images included, on the canvas, and offset the root by its margins; none paints a canvas for a page that sets no background. |
| #523 (merged) "WPT: render ref tests over a white background" | Makes the WPT runner fill white under each scene, as `examples/screenshot.rs` and the window renderer do: it shows the canvas is left transparent, and fills it white whatever the colour scheme. |
| #241 (closed), #248 (merged) | The background colour differing between the CPU and GPU renderers. |

The reproduction was run on `main` at `2335458` before filing.

