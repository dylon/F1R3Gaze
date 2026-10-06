# The canvas and the default text colour ignore the root's colour scheme

**Filed as** <https://github.com/DioxusLabs/blitz/issues/1078> (2026-10-06).

**Project:** [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz)
(blitz-paint, blitz-dom). Present on `main` at `2335458` (2026-10-06).

## Reproduction

The program [`repro-canvas-color-scheme.rs`](repro-canvas-color-scheme.rs)
makes each document in a 400 × 300 viewport, paints it with
`blitz_paint::paint_scene` into anyrender's recording `Scene`, and prints
every solid fill that covers the whole canvas, and the colour of the first
glyph run. The body is `<p>text</p>`; the documents are:
1. a light preference, nothing declared;
2. a dark preference, `:root { color-scheme: dark }`;
3. as 2, with `html { background: Canvas }`;
4. as 2, with `html { color: CanvasText }`.

**Observed** on `main` at `2335458`:

```text
light preference, nothing declared                   fills covering the canvas: ["Rgba8 { r: 0, g: 0, b: 0, a: 0 }"]; text: Rgba8 { r: 0, g: 0, b: 0, a: 255 }
dark preference, :root { color-scheme: dark }        fills covering the canvas: ["Rgba8 { r: 0, g: 0, b: 0, a: 0 }"]; text: Rgba8 { r: 0, g: 0, b: 0, a: 255 }
dark preference, html { background: Canvas } too     fills covering the canvas: ["Rgba8 { r: 30, g: 30, b: 30, a: 255 }"]; text: Rgba8 { r: 0, g: 0, b: 0, a: 255 }
dark preference, html { color: CanvasText } too      fills covering the canvas: ["Rgba8 { r: 0, g: 0, b: 0, a: 0 }"]; text: Rgba8 { r: 232, g: 232, b: 232, a: 255 }
```

**Expected.** A page that declares `color-scheme: dark` and sets no colours
of its own should get the dark scheme's canvas and text: the canvas painted
`Canvas` (here 30, 30, 30) and the text `CanvasText` (232, 232, 232), as
documents 3 and 4 get only by asking for them.
- CSS Color Adjust 1 §2.4: "On the root element, the element color scheme
  additionally must affect the surface color of the canvas, and the
  viewport's scrollbars"
  ([§2.4](https://drafts.csswg.org/css-color-adjust-1/#color-scheme-effect)).
- CSS Color 4 gives `color` the initial value `CanvasText`
  ([CSS Color 4, the `color` property](https://drafts.csswg.org/css-color-4/#the-color-property)).

Instead the canvas is left transparent, so what shows is whatever the
embedder painted first: `examples/screenshot.rs`, the window renderer and
the WPT runner fill white (#523). The text is black. A dark page that relies
on the defaults is therefore black on white, and on any dark surface black
on dark.

## Cause

- **The canvas.** `BlitzDomPainter::paint_scene`
  (`packages/blitz-paint/src/render.rs:175` on `main`) fills the canvas with
  the root element's `background-color`, or the `<body>`'s when the root's
  is transparent, and with nothing else: when both are transparent, the
  fill is transparent too.
- **The text.** Stylo's initial value of `color` is
  `crate::color::AbsoluteColor::BLACK` (stylo 0.22.0
  `properties/longhands.toml:556-558`), not `CanvasText`, and blitz-dom's
  default style sheet sets `color: CanvasText` only on dialogs and popovers
  (`packages/blitz-dom/assets/default.css:968-969, 1129-1130`).

## Suggested fix

- In `paint_scene`, when neither the root element nor a propagating
  `<body>` has a background colour, fill the canvas with the `Canvas` system
  colour resolved for the root element's style, so that it follows the
  root's `color-scheme`. For an embedded document whose root's colour scheme
  differs from its host element's, §2.4 asks for "an opaque canvas of the
  Canvas color appropriate to the embedded document's root element's element
  color scheme instead of a transparent canvas", which is the same fill.
- Give the root `color: CanvasText` in blitz-dom's default style sheet, which
  stands in for the initial value CSS Color 4 gives and Stylo does not.

## Related

- #1011, #672 and #640 (open) paint the root's or the `<body>`'s background,
  images included, on the canvas; none of them paints a canvas for a page
  that sets no background.
- #523 (merged) fills white under the WPT runner's scenes, as
  `examples/screenshot.rs` and the window renderer do.

## Where it was found

[F1R3Gaze](https://github.com/F1R3FLY-io/F1R3Gaze), a Blitz-based browser,
lets pages follow the scheme its chrome shows (its chrome ledger, L13). A
page that asks for dark with `color-scheme` and sets no background stays on
the chrome's white page area with black text: legible, but not dark.
