# Linear gradients are sampled at pixel corners through a 512-entry ramp, so hard stops move with the gradient's length

**Filed as** <https://github.com/linebender/vello/issues/1975> (2026-10-04).

**Project:** [linebender/vello](https://github.com/linebender/vello) 0.10.0
(`vello_shaders` 0.10.0, `vello_encoding` 0.10.0), GPU renderer. Seen through
Blitz `674d7d2`, which turns a CSS `linear-gradient` into a `peniko`
gradient with two stops at the same offset.

## Reproduction

The same CSS hard stop, 3 px of one colour and then another, on elements of
different widths:

```html
<!doctype html>
<html><body style="margin:0;background:#fff">
  <div style="margin:8px;width:287px;height:40px;
              background:linear-gradient(to right,#087f86 3px,#dde9ed 3px)"></div>
  <div style="margin:8px;width:261px;height:40px;
              background:linear-gradient(to right,#087f86 3px,#dde9ed 3px)"></div>
  <div style="margin:8px;width:34px;height:34px;
              background:linear-gradient(to right,#087f86 3px,#dde9ed 3px)"></div>
</body></html>
```

**Observed** at scale 1, measured on [`repro-paint.html`](repro-paint.html)
(these boxes and one more) in a Blitz-based browser window (Xvfb, Vello on Mesa
lavapipe). The count is of `#087f86` pixels on one row through each box; the
boxes start at screen x = 50:

| Width $`L`$ (px) | Bar | Columns |
|---|---|---|
| 287 | 4 px | 50–53 |
| 261 | 3 px | 50–52 |
| 34 | 4 px | 50–53 |

**Expected.** 3 px each time. With a 3 px stop at integer positions, every
pixel centre lies unambiguously on one side.

## Cause

- The fine stage evaluates the gradient at integer pixel coordinates,
  `xy = vec2(f32(global_id.x * PIXELS_PER_THREAD), f32(global_id.y))`
  (`vello_shaders/shader/fine.wgsl:1076`, `:1203`). That is the pixel's corner,
  not its centre.
- The colour comes from a ramp of `GRADIENT_WIDTH = 512` entries at index
  `round(t * 511)` (`fine.wgsl:26`, `:1225-1236`). The ramp is baked with
  `N_SAMPLES = 512` (`vello_encoding/src/ramp_cache.rs:12`).
- A stop at offset $`s/L`$ therefore moves to the nearest multiple of
  $`1/511`$. The pixel whose corner is $`k`$ px from the start takes the first
  colour exactly when the ramp entry it lands on precedes the stop:

```math
\frac{1}{511}\,\mathrm{round}\!\left(\frac{511\,k}{L}\right) < \frac{s}{L}.
```

  This predicts every observation above. For example, at $`L = 287`$ and
  $`k = 3`$: $`511 \cdot 3/287 = 5.34`$ rounds to 5, and
  $`5/511 = 0.009785 < 3/287 = 0.010453`$, so pixel 3 takes the bar colour and
  the bar is 4 px.
- The error grows with the gradient's length: up to $`L/1022`$ px either way,
  or 1.25 px on a 1280 px element. Hard-stop stripes, a common CSS pattern,
  land on the wrong pixels.

## Context, and `main`

- The 512-entry ramp is the lookup-table approach discussed in
  linebender/vello#1058, which already expected banding on very large
  gradients. Hard stops show the error at ordinary sizes, a few hundred pixels
  wide.
- Checked on 2026-10-04 against `main` at `f3000c8d9e7a`: the code is
  unchanged. With the original compute-centric renderer moved to `research/`,
  it is at `research/vello_shaders/shader/fine.wgsl:26`, `:1076`, and
  `:1225-1236`.

## Suggested fix

- Sample at pixel centres, `xy + 0.5`, in the gradient paths of the fine stage.
- Make hard stops exact: evaluate the stop list analytically, or snap stop
  offsets into the ramp so the transition falls on the right side of each
  pixel centre. Raising the ramp's resolution alone only shrinks the error.

## Workaround

For a bar along one edge, F1R3Gaze uses a gradient of a single colour, sized
to the bar: `linear-gradient(c, c) 0 0 / 3px 100% no-repeat`, over the
background colour. A constant ramp has no stop to quantize, and the bar's edge
is the rectangle's own edge.
