# Text in an anonymous block keeps its old colour after a colour-only restyle

**Filed as** <https://github.com/DioxusLabs/blitz/issues/1037> (2026-10-04).

**Project:** [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz), rev `674d7d2`
(blitz-dom, blitz-paint).

## Reproduction

```html
<!doctype html>
<html class="dark">
<head>
<style>
  :root { --fg: #eaf0f7; --bg: #161b26; }
  :root.light { --fg: #182734; --bg: #f5f7fa; }
  body { background: var(--bg); }
  button { color: var(--fg); background: var(--bg); }
</style>
</head>
<body>
  <button id="bare">Bare label</button>
  <button id="wrapped"><span>Wrapped label</span></button>
</body>
</html>
```

1. Load the document and lay it out.
2. Change the root's class from `dark` to `light`: set the `class` attribute
   on `<html>` through `DocumentMutator::set_attribute`.
3. Restyle, lay out, and paint.

**Observed.** Run through the blitz-dom API on 2026-10-04: `parse`,
`resolve`, `set_attribute(<html>, "class", "light")`, `resolve`. Afterwards,
each anonymous text run's computed `color` was compared with its parent's.
- Before the switch, nothing is stale.
- After the switch, exactly one run is stale: `"Bare label"`. Its anonymous
  block's computed colour is sRGB (0.918, 0.941, 0.969), the old `#eaf0f7`. Its
  `<button>` computes (0.094, 0.153, 0.204), the new `#182734`.
- `"Wrapped label"` follows its `<span>` and is painted in `#182734`.
- Paint takes the glyph colour from the anonymous block (see step 5 below).
  So the label is drawn near-white `#eaf0f7` on the new `#f5f7fa` background,
  a contrast of 1.07:1, effectively invisible. The expected contrast is
  14.19:1.

**Expected.** Both labels use the button's current `color`, as in any browser.

## Cause

1. The user-agent sheet makes every `<button>` a flex container
   (`blitz-dom/assets/default.css`, the `button, input[type=…]` rule:
   `display: inline-flex`).
2. Bare text inside a flex container is wrapped in an anonymous block. Its
   style is computed once, from the parent's style at that moment, in
   `create_anonymous_block` (`blitz-dom/src/layout/construct.rs:134`).
3. The style traversal visits only real DOM children (`blitz-dom/src/stylo.rs`),
   so the anonymous block is never restyled.
4. A colour-only change causes no layout damage
   (`compute_layout_damage`, `blitz-dom/src/layout/damage.rs:259`): only
   display, float, position, contain, visibility, and font changes rebuild
   boxes. So the anonymous block is not rebuilt either.
5. Paint reads the glyph colour from the anonymous block's own style
   (`blitz-paint/src/text.rs`, the text brush resolves its node's style).

Nothing that restyles the parent reaches the anonymous child, so its colour
stays as it was when the box was built.

## Still present on `main`

Checked on 2026-10-04 against `main` at `0db8c74`, 30 commits after the pin:
- `create_anonymous_block` is unchanged.
- The changes to `stylo.rs` do not touch anonymous nodes.
- `damage.rs` only gained a rebuild for inline `<svg>`.

## Suggested fix

Any one of the following:
- When an element with anonymous children is restyled, recompute their styles
  by inheritance from the element's new primary style (they have no rules of
  their own).
- Treat a change of an inherited property on an element with anonymous
  children as damage that rebuilds those children.
- At paint time, resolve inherited text properties of an anonymous block
  through its parent.

## Workaround

F1R3Gaze never puts bare text directly inside a flex or grid box: every label
is an element (`<span>`). A test lints the laid-out tree for text in anonymous
boxes, and another compares every anonymous text run's colour with its parent's
after a scheme switch. Generated content (`::before`, `::after`) is restyled
correctly with its element.
