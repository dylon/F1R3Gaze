# Engine defects found while building the chrome

The chrome works around three engine defects instead of patching pinned
dependencies (design decision 5; [ledger](../ledger.md) L2 and L5). Each
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

[`repro-paint.html`](repro-paint.html) holds both paint reproductions: three
gradient bars and the inset-shadow box. Its measurements, taken with the
release binary on 2026-10-04, are quoted in each report.

All three were filed on 2026-10-04, under design decision 5 ("reported
upstream"), after searching both trackers for duplicates. Related reports are
cited in the issues: DioxusLabs/blitz#349 and linebender/vello#1245 (single
corner radius), and linebender/vello#1058 (the gradient lookup table). The
issue text is each file here without its title, with the local links written
out and a closing note on where the defect was found. When one of the issues
is fixed and the pins are raised, conventions R1 and R8 can be revisited.
