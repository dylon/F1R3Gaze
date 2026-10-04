# Inset box-shadows: hairlines on coincident edges, an averaged radius, and an unscaled offset

**Filed as** <https://github.com/DioxusLabs/blitz/issues/1038> (2026-10-04).

**Project:** [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz), rev `674d7d2`
(blitz-paint, `src/render/box_shadow.rs`).

## Reproduction

```html
<!doctype html>
<html><body style="margin:20px;background:#fff">
  <div style="width:240px;height:40px;border-radius:7px;background:#f5f7fa;
              box-shadow:inset 3px 0 0 #087f86"></div>
</body></html>
```

**Observed at scale 1.** Measured on [`repro-paint.html`](repro-paint.html),
which contains this box, in a Blitz-based browser window (Xvfb, Vello on Mesa
lavapipe).
- A one-pixel line of about half the shadow colour runs along the whole top
  edge. That row is `#78B8BD`, between white above it and `#F5F7FA` below.
- Depending on the box's height, the line also appears along the bottom edge.
- Slivers of the shadow colour appear at the right-hand rounded corners.
- A 5 px band across the top edge (x 90–289 of the screen) has 200 of 200
  pixels lying off the blend of the colours outside and inside the edge.
- A control band in a flat area has 0.

**Expected.** A 3 px bar along the left edge, following the left-hand corners,
and nothing on the other edges. This is the inset shadow defined in
[CSS Backgrounds and Borders 3, §6.1.1 "Shadow Shape, Spread, and Knockout"](https://www.w3.org/TR/css-backgrounds-3/#shadow-shape).

## Cause

`draw_inset_box_shadow` (`box_shadow.rs:89-147`) paints each inset shadow in
three steps:
1. It pushes a clip layer of the padding-box path and fills that path with the
   shadow colour.
2. It pushes a `Compose::DestOut` layer and draws the "hole" with
   `scene.draw_box_shadow(transform, hole, WHITE, radius, blur)`. The hole is
   the padding box, shrunk by the spread and moved by the offset.
3. It pops both layers.

Three things go wrong:

1. **Hairlines on coincident edges.** `draw_box_shadow` is an analytic
   blurred-rectangle shader. At zero blur, its coverage exactly on the
   rectangle's edge is about 0.5. Wherever an edge of the hole coincides with
   an edge of the padding box (top and bottom for a horizontal offset), DestOut
   removes only about half of the colour the first fill put there, and a
   half-intensity line remains.
2. **Averaged radius.** The hole uses `self.frame.border_radii.average()`
   (`:110`), not the padding box's per-corner radii. Where they differ, the
   hole fails to cover the corner and slivers remain (see also the TODO on
   `:109`). This is the single-radius limitation of `draw_blurred_rounded_rect`
   behind DioxusLabs/blitz#349 and linebender/vello#1245, here on the inset
   path.
3. **Unscaled offset.** The hole's translation is
   `shadow.base.horizontal.px() as f64` (`:111-114`). The outset path
   multiplies the same value by `self.scale` (`:56-59`), and spread and blur
   are scaled in the inset path too (`:134`, `:141`). On a 2× display, an inset
   shadow is therefore offset by half the intended distance. *This item was
   found by reading the source; F1R3Gaze's tests run at scale 1.*

## Still present on `main`

Checked on 2026-10-04 against `main` at `0db8c74`. The inset path differs from
the pin only by the new `Fill::NonZero` argument to `push_layer`; the hole,
the averaged radius, and the unscaled offset are unchanged.

## Suggested fix

- **For zero blur**, draw the shadow as one shape instead of fill-then-subtract:
  fill the padding-box path minus the hole path (the hole as a rounded rect
  with the padding box's per-corner radii, offset, shrunk by the spread) with
  `Fill::EvenOdd`, inside the padding-box clip. A single rasterisation has no
  coincident-edge conflation.
- **For blur**, keep the analytic shader, but clip the result with the
  padding-box path rather than subtracting from a separate fill.
- **Scale** the offset by `self.scale`, as the outset path does.

## Workaround

F1R3Gaze draws edge bars as a solid image the size of the bar,
`linear-gradient(c, c) 0 0 / 3px 100% no-repeat`, over the background colour.
It never uses an inset box-shadow for a bar. Its snapshot harness checks that
every bar's edges show 0 off-blend pixels.
