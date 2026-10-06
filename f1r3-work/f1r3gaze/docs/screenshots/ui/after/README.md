# After

The finished chrome, with the storage work: the five roots, themes that
follow the system, and the window kept between starts.

**Scenes 01–99.** One full run at 05:53 (UTC−4) on 2026-10-06, with the
binary of sha256
`7eb838afd20c6105c8cfc32aad093c435878d2d654d93ac2469f0e05cf4b76d1`, built
from branch `feature/state-management` (storage ledger S15, part 3). All
144 of the checks in `checks.tsv` pass. The work directory was
`target/ui-snapshots`, which is the path the `file://` URLs in these
captures show. Scenes 98 and 99 ran with a window manager, Openbox 3.6.1,
on the same virtual screen (storage ledger S13, part 3).

The images of scenes 01–97 are byte-identical to those of the step-11 run
the storage ledger records (S13 part 2, E8), and scenes 19, 98 and 99 to
those of the first openbox run (S13 part 3): the holder's record written
without syncing (S15 part 3) changes nothing on screen.

**The cursor scenes (61–68; chrome ledger L8).**
- A screen capture does not show the X cursor. So these scenes read it from
  the X server, and save every sprite they read in [`cursors/`](cursors/) as
  `<scene>.<what>.png`.
- `ref-hand`, `ref-text` and `ref-arrow` are each scene's references. The other
  sprites are the cursor over the page.
- A hidden cursor is a 1×1 transparent sprite.

**The record of runs.** `run.txt` describes the latest run only, and
`runs.tsv` lists every run that wrote here, the earlier ones included: the
60 scenes of 2026-10-04 (binary `635ed212…`), the cursor scenes of the same
day (`62f4282b…`), and a run before them whose first line was written by
hand (`11245cc1…`), because the harness only began writing `runs.tsv`
afterwards. The images of those runs are replaced by this one; git keeps
them.

`run.txt`'s `window_manager` line was corrected by hand: the harness wrote
a second line, `none installed`, after the version, because under
`pipefail` the `head` that read the version failed its pipeline and the
fallback ran too. The harness reads the version without a pipe since.
