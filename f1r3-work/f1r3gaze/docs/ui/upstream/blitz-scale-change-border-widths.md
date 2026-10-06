# A document whose scale changes keeps the border widths of its first scale

**Filed as** <https://github.com/DioxusLabs/blitz/issues/1076> (2026-10-06).

**Project:** [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz)
(blitz-dom). Present at rev `674d7d2` and on `main` at `2335458`
(2026-10-06).

## Reproduction

Six boxes with 1px borders:

```html
<html><head><style>
body { margin: 0 }
div { width: 200px; height: 20px; border: 1px solid black }
</style></head><body>
<div id="box"></div><div></div><div></div><div></div><div></div><div id="last"></div>
</body></html>
```

The program [`repro-scale-change.rs`](repro-scale-change.rs) works headlessly
through the public API. It makes the document in a 1280 × 800 viewport four
ways, each ending at a total scale (`hidpi_scale * zoom`) of 1.2:
1. at zoom 1.2 from the start;
2. at zoom 1.0, resolved, then `viewport_mut().set_zoom(1.2)` and resolved;
3. at a hidpi scale of 1.2 from the start;
4. at a hidpi scale of 1.0, resolved, then `viewport_mut().set_hidpi_scale(1.2)`
   and resolved.

It prints the computed `border-top-width` of the first box
(`resolved_style_value`) and where the last box is laid out.

**Observed.** On 2026-10-06, identical at `674d7d2` and at `main` `2335458`:

```text
made at zoom 1.2                   scale 1.2  border-top-width 0.816667px    last box at y = 108
made at zoom 1.0, then zoom 1.2    scale 1.2  border-top-width 1px           last box at y = 110
made at hidpi scale 1.2            scale 1.2  border-top-width 0.816667px    last box at y = 108
made at scale 1.0, then scale 1.2  scale 1.2  border-top-width 1px           last box at y = 110
```

**Expected.** The same four lines: a document's computed values and layout
should depend on its current scale, not on the scale it was first styled at.
After a zoom, or after a window moves to a monitor with another scale
factor, the borders keep the old scale's snapping. They then fall between
device pixels, and boxes are placed a pixel or more away from where the same
document made at that scale places them. A second resolve does not change
it.

## Cause

- Stylo snaps a border width to whole device pixels when it computes the
  value, as CSS Values 4's "snap a length as a border width" says:
  `snap_as_border_width` in `style/values/specified/border.rs` (stylo
  0.22.0, lines 235–246) rounds down with
  `context.device().app_units_per_device_pixel()`.
- `BaseDocument::flush_pending_device_changes`
  (`packages/blitz-dom/src/document.rs` on `main`) rebuilds the stylist's
  device on a scale change (`DeviceChanges::SCALE`) and invalidates the
  inline layouts, but restyles only when a media query result changes
  (`set_stylist_device` calls `force_stylesheet_origins_dirty` only for a
  non-empty `OriginSet`), when viewport units are used, or when the colour
  scheme changes (`RestyleHint::recascade_subtree()` on the root). A border
  width is none of these, so its computed value keeps the old device pixel
  ratio.
- Before #782 ("Avoid full restyle on viewport resize", merged 2026-08-24)
  every device change restyled the whole document, which hid this. #785
  then coalesced the device changes and kept the recascade for colour-scheme
  changes only.

## Suggested fix

Recascade on a scale change as on a colour-scheme change: the device pixel
ratio is, like the colour scheme, an input to values resolved at cascade
time.

```diff
-        if changes.contains(DeviceChanges::COLOR_SCHEME) {
+        if changes.intersects(DeviceChanges::COLOR_SCHEME | DeviceChanges::SCALE) {
             if let Some(root_id) = self.try_root_element().map(|el| el.id) {
                 self.nodes[root_id].set_restyle_hint(RestyleHint::recascade_subtree());
             }
         }
```

With this change on a clone of `main` `2335458`, the reproduction prints
`0.816667px` and `y = 108` for all four ways, and `cargo test -p blitz-dom`
passes (57 tests). Scale changes are as rare as colour-scheme changes, so
the full-tree recascade costs little.

## Related

- #902 (open) fixes layout rounding that can zero a one-device-pixel border
  at a fractional scale; it rounds the layout of a given scale, and does not
  change what happens when the scale changes.
- #837 (open), 1px borders not rendering, is the issue #902 fixes.
- #784 (merged) made `zoom_by` and `zoom_to` invalidate the inline layouts
  and redraw; the inline layouts are not where this difference comes from
  (the reproduction has no text).

## Where it was found

[F1R3Gaze](https://github.com/F1R3FLY-io/F1R3Gaze)'s chrome is a Blitz
document. A window restored at a zoom of 1.2 drew its chrome slightly
differently from a window zoomed to 1.2 with the keys: the status bar's border one row lower,
and glyphs in the tab strip and the address field placed differently. The
two zooms were the same `f32`, and the difference followed the zoom each
window started at (F1R3Gaze's storage ledger, S13 part 2, E3). The
reproduction above narrows it to the border widths: without borders, or
with outlines instead, the layouts are identical. F1R3Gaze's snapshot
harness meanwhile compares a window with itself at a zoom.
