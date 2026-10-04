# After

The finished chrome. Two runs produced these images.

**Scenes 01–60.** One full run at 08:13 on 2026-10-04, with the binary of
sha256 `635ed2121774739fa05546ec90074c6ddf32dbbfc485a0820187d2b1d9c2ba30`, built
from the source of the UI overhaul. All 28 of their checks in `checks.tsv`
pass. The harness then worked in `/tmp/claude-1000/…/f1r3gaze-ui-snapshots`,
which is the path the `file://` URLs in these captures show.

**Scenes 61–68, the cursor scenes (ledger L8).** A run at 16:23 on
2026-10-04, `--only cursor`, with the binary that fixes the cursor over pages
(`62f4282bcafee0bb500568047ec8fe0d263bc7828aa727ae85049bbb0d2dac27`). The work
directory was `target/ui-snapshots`. All 21 of their checks pass.
- A screen capture does not show the X cursor. So these scenes read it from
  the X server, and save every sprite they read in [`cursors/`](cursors/) as
  `<scene>.<what>.png`.
- `ref-hand`, `ref-text` and `ref-arrow` are each scene's references. The other
  sprites are the cursor over the page.
- A hidden cursor is a 1×1 transparent sprite.

The cursor fix does not change scenes 01–60. Both binaries were run through
all 68 scenes with the same harness, and those 60 scenes are pixel-identical
(ledger L8, "Regression"). `run.txt` describes the latest run only, and
`runs.tsv` lists every run that wrote here.

`runs.tsv` also lists a run before the 08:13 one. That run used a binary
(`11245cc1…`) whose stylesheet comment for R3 had not yet been corrected. Its
first line was written by hand, from that run's `run.txt`, because the harness
only began writing `runs.tsv` afterwards. Every image is pixel-identical
across the two runs. Both runs are also pixel-identical, for all 59 scenes
they share, to a capture taken before the lint cleanup (ledger L7).
