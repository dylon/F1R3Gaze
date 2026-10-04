# After

The finished chrome. The binary has sha256
`635ed2121774739fa05546ec90074c6ddf32dbbfc485a0820187d2b1d9c2ba30`, built from
the final source.

All 60 scenes come from one full run at 08:13 on 2026-10-04. All 28 checks in
`checks.tsv` pass.

`runs.tsv` also lists the run before it. That run used a binary
(`11245cc1…`) whose stylesheet comment for R3 had not yet been corrected. Its
first line was written by hand, from that run's `run.txt`, because the harness
only began writing `runs.tsv` afterwards. Every image is pixel-identical
across the two runs. Both runs are also pixel-identical, for all 59 scenes
they share, to a capture taken before the lint cleanup (ledger L7).
