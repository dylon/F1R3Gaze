#!/usr/bin/env python3
"""storage-bench-pairs.py — before against after in alternating pairs, for
scripts/storage-bench.sh.

    python3 scripts/lib/storage-bench-pairs.py OUT CPUS BEFORE AFTER PAGE_PAIRS OTHER_PAIRS WARMUP

hyperfine runs all of one build's starts, then all of the other's. On a
machine whose load drifts, the two blocks see different machines, and the
difference of their medians holds the drift: on 2026-10-06 a steady page
start measured +3.58 ms that way and +1.64 ms in alternating pairs (ledger
S15, part 2, H-drift). This script measures the same comparisons in pairs:
for each command (page, wallet list) and profile (fresh, steady, old-3,
old-full), PAGE_PAIRS or OTHER_PAIRS pairs of one start of each build, the
order within a pair alternating, after WARMUP pairs that are not kept. Each
start's profile is prepared as hyperfine's --prepare prepares it, and the
preparation is not timed: `fresh` is removed, `old-3` and `old-full` are
copied again from OUT/fixtures (OUT/restore.sh), and `steady` is made once,
by a page start of its own build, before the pairs. A start is timed from
before its process is made until it has exited (`time.perf_counter`), pinned
to CPUS with taskset, and must exit 0.

It writes OUT/pairs.tsv: command, profile, pair, which build went first, and
each build's wall time in milliseconds. Its profiles are OUT/profiles/
pairs-BUILD-COMMAND-PROFILE. It runs in the environment storage-bench.sh
isolated, which it inherits.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import time
from pathlib import Path

COMMANDS = {
    "page": ["--headless", "gaze://newtab", "--timeout", "5"],
    "wallet": ["wallet", "list"],
}
PROFILES = ("fresh", "steady", "old-3", "old-full")


def main() -> int:
    if len(sys.argv) != 8:
        print(__doc__, file=sys.stderr)
        return 64
    out, cpus = Path(sys.argv[1]), sys.argv[2]
    bins = {"before": sys.argv[3], "after": sys.argv[4]}
    pairs = {"page": int(sys.argv[5]), "wallet": int(sys.argv[6])}
    warmup = int(sys.argv[7])
    restore = out / "restore.sh"

    def profile_of(build: str, command: str, kind: str) -> Path:
        return out / "profiles" / f"pairs-{build}-{command}-{kind}"

    def prepare(build: str, command: str, kind: str) -> None:
        profile = profile_of(build, command, kind)
        match kind:
            case "fresh":
                shutil.rmtree(profile, ignore_errors=True)
            case "old-3" | "old-full":
                subprocess.run([str(restore), str(out / "fixtures" / kind), str(profile)], check=True)
            case _:
                pass

    def start(build: str, command: str, kind: str) -> float:
        prepare(build, command, kind)
        argv = ["taskset", "-c", cpus, bins[build], "--profile", str(profile_of(build, command, kind)), *COMMANDS[command]]
        began = time.perf_counter()
        done = subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        took = (time.perf_counter() - began) * 1e3
        if done.returncode != 0:
            raise SystemExit(f"{' '.join(argv)}: exit {done.returncode}")
        return took

    rows = ["command\tprofile\tpair\tfirst\tbefore_ms\tafter_ms"]
    for command in COMMANDS:
        for kind in PROFILES:
            if kind == "steady":
                for build in bins:
                    profile = profile_of(build, command, kind)
                    shutil.rmtree(profile, ignore_errors=True)
                    subprocess.run([bins[build], "--profile", str(profile), *COMMANDS["page"]],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
            for i in range(warmup + pairs[command]):
                order = ("before", "after") if i % 2 == 0 else ("after", "before")
                took = {build: start(build, command, kind) for build in order}
                if i >= warmup:
                    rows.append(f"{command}\t{kind}\t{i - warmup}\t{order[0]}\t{took['before']:.3f}\t{took['after']:.3f}")
            print(f"pairs {command} {kind}: {pairs[command]}", file=sys.stderr, flush=True)
    (out / "pairs.tsv").write_text("\n".join(rows) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
