# Chrome snapshots

Captured by [`scripts/ui-snapshots.sh`](../../../scripts/ui-snapshots.sh):
- 1280 × 800, under Xvfb;
- one seeded, throwaway profile per scene.

How the captures are made and checked is in
[`docs/ui/README.md` §12.2](../../ui/README.md#122-the-snapshot-harness).

| Folder | What it shows | Binary (sha256) |
|---|---|---|
| [`before/`](before/) | The browser before the UIX work. 60 scenes. Its checks are recorded only. | `f7a6d0b3…c55a5` |
| [`after/`](after/) | The finished chrome. 60 scenes, every check enforced and passing. | `635ed212…ba30` |
| [`review-1/`](review-1/) | The first complete "after" capture: the 15 scenes cited as evidence in [ledger L5](../../ui/ledger.md#l5--review-of-the-first-complete-after-capture). | `c7faee73…a7027` |

Each capture folder holds:
- `NN-name.png`: one image per scene.
- `checks.tsv`: the measurements and their limits.
- `run.txt`: the latest run.
- `runs.tsv`: one line per run, so a partial re-run (`--only`) does not hide
  where the other images came from.
- `contact-sheet.png`: every scene on one page.
