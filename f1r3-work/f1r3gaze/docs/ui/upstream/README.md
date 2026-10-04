# Engine defects found while building the chrome

The chrome works around four engine defects instead of patching pinned
dependencies (design decision 5; [ledger](../ledger.md) L2, L5 and L8). Each
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
