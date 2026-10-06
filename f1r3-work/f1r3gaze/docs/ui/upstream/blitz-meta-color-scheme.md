# `<meta name="color-scheme">` is ignored

**Filed as** <https://github.com/DioxusLabs/blitz/issues/1077> (2026-10-06).

**Project:** [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz)
(blitz-dom). Present at rev `674d7d2` and on `main` at `2335458`
(2026-10-06).

## Reproduction

The program [`repro-meta-color-scheme.rs`](repro-meta-color-scheme.rs) works
headlessly through the public API. Each document is made in a viewport whose
preferred colour scheme is dark (`ColorScheme::Dark`), with this body:

```html
<body><p id="p" style="color: CanvasText; background-color: Canvas">text</p><mark id="i">marked</mark></body>
```

and one of five heads: none; `<meta name="color-scheme" content="light">`;
`<style>:root{color-scheme:light}</style>`;
`<meta name="color-scheme" content="dark">`;
`<style>:root{color-scheme:dark}</style>`. It prints the computed `color` of
`#p` (`CanvasText`) and the `background-color` of the `<mark>`, which
blitz-dom's default style sheet takes from system colours.

**Observed.** On 2026-10-06, identical at `674d7d2` and at `main` `2335458`:

```text
no declaration                                   p CanvasText rgb(232, 232, 232)     mark background rgb(102, 92, 0)
<meta name=color-scheme content=light>           p CanvasText rgb(232, 232, 232)     mark background rgb(102, 92, 0)
:root { color-scheme: light } (CSS)              p CanvasText rgb(0, 0, 0)           mark background rgb(255, 235, 59)
<meta name=color-scheme content=dark>            p CanvasText rgb(232, 232, 232)     mark background rgb(102, 92, 0)
:root { color-scheme: dark } (CSS)               p CanvasText rgb(232, 232, 232)     mark background rgb(102, 92, 0)
```

**Expected.** The meta declarations should act as the CSS ones do: with
`content="light"` the page keeps light system colours under a dark
preference, as `:root { color-scheme: light }` does. The HTML standard
defines the `color-scheme` metadata name: "It determines the page's supported
color-schemes" ([HTML §4.2.5.1](https://html.spec.whatwg.org/multipage/semantics.html#meta-color-scheme)),
and CSS Color Adjust 1 says "In HTML, the color-scheme `<meta>` sets the page
color scheme" and "Individual elements have an element color scheme, which by
default matches the page color scheme"
([CSS Color Adjust 1 §2](https://drafts.csswg.org/css-color-adjust-1/#color-scheme-prop)).
A page that declares only light, to keep its black-on-white defaults
readable, is today given the dark scheme's system colours: light grey text,
dark `mark` backgrounds.

## Cause

No code in blitz-dom or blitz-html reads `<meta name="color-scheme">`: a
search of `packages/` on `main` finds `color-scheme` only in the viewport's
preferred scheme and its plumbing (blitz-traits' `shell.rs`, blitz-shell's
`window.rs` and `convert_events.rs`, blitz-dom's `stylo_device.rs`,
`document.rs` and `resolve.rs`, the test harness) and in the scrollbar
palette (blitz-paint's `render.rs`). The root element's `color-scheme` stays
`normal`, and Stylo's Servo device resolves `normal` from the preferred
scheme (`Device::is_dark_color_scheme`, stylo 0.22.0 `device/servo.rs:336-350`:
"If either both or none are supported, then use the preferred color
scheme").

## Suggested fix

Give the root element the page's supported color-schemes when its own
`color-scheme` is `normal`: find the first `meta` element in tree order whose
`name` is an ASCII case-insensitive match for `color-scheme` and whose
`content` parses as a `color-scheme` value, as HTML's algorithm says, and
apply that value to the root element below author style, for example as a
presentational hint (`synthesize_presentational_hints_for_legacy_attributes`
in `packages/blitz-dom/src/stylo.rs`). HTML asks that the value be found
again when such `meta` elements are inserted or removed, or their `name` or
`content` change, so the root would then be restyled.

## Where it was found

[F1R3Gaze](https://github.com/F1R3FLY-io/F1R3Gaze), a Blitz-based browser,
lets pages follow the scheme its chrome shows. Because Stylo gives a page
that declares nothing the preferred scheme's system colours, while CSS Color
Adjust says "Pages have to opt into color scheme support", F1R3Gaze gives
every page `:root{color-scheme:light}` in its user-agent style sheet, which a
page's own `color-scheme` overrides (its chrome ledger, L13). A page that
opts in only with the `<meta>` cannot, so it keeps light system colours in a
dark chrome.
