#!/usr/bin/env python3
"""storage-bench-table.py — the tables of scripts/storage-bench.sh.

    python3 scripts/lib/storage-bench-table.py OUT > OUT/table.md

OUT is a storage-bench.sh output folder. The Markdown printed has four
parts:

1. the machine, before and after the runs (machine.txt, machine-after.txt);
2. every hyperfine measurement: mean, standard deviation, median, range,
   runs, and the exit codes seen;
3. before against after, for each command and profile both builds ran:
   the difference of the means with its 95 % confidence interval (Welch's
   t-test, which does not assume equal variances), the difference of the
   medians, and the two-sided p-value of the Mann-Whitney U test, which
   does not assume normal times either; and, from the alternating pairs
   (pairs.tsv, scripts/lib/storage-bench-pairs.py), the median of the
   pairs' differences, their interquartile range, and the two-sided p-value
   of the Wilcoxon signed-rank test, which drift between hyperfine's two
   blocks cannot bias;
4. what one start did to its profile, from strace -f -y: every traced call
   whose arguments name a path inside the profile, counted by kind, and the
   paths written, relative to the profile.

A call counts as on the profile when the profile's absolute path appears in
it followed by `/`, `"` or `>` (strace -y prints a descriptor's path as
`3</path>`). For `write` and `pwrite64` only the descriptor's path counts:
their quoted argument is the data written, which may name the profile's
folders (a report printed to stdout) without touching them (ledger S17). `openat` counts as a write when its flags include O_WRONLY,
O_RDWR, O_CREAT or O_TRUNC. A call strace splits into `<unfinished ...>` and
`<... resumed>` lines counts once, at its first line.
"""

from __future__ import annotations

import json
import re
import statistics
import sys
from collections import Counter
from pathlib import Path

from scipy import stats

CALL = re.compile(r"^\d+\s+\d\d:\d\d:\d\d\.\d+\s+([a-z0-9_]+)\((.*)$")
WRITE_FLAGS = ("O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC")
# The columns of part 4, each a set of system calls.
KINDS = [
    ("open to write", None),
    ("open to read", None),
    ("write", {"write", "pwrite64"}),
    ("fsync", {"fsync"}),
    ("fdatasync", {"fdatasync", "sync_file_range"}),
    ("rename", {"rename", "renameat", "renameat2"}),
    ("link", {"link", "linkat"}),
    ("unlink", {"unlink", "unlinkat"}),
    ("rmdir", {"rmdir"}),
    ("mkdir", {"mkdir", "mkdirat"}),
    ("chmod", {"chmod", "fchmod", "fchmodat"}),
    ("ftruncate", {"ftruncate"}),
    ("flock", {"flock"}),
]
# A quoted path, or a descriptor's path, in a call's arguments.
PATH_IN_CALL = re.compile(r'"([^"]*)"|<([^<>]*)>')
# Calls whose quoted argument is data, not a path, and the descriptor's
# path at the start of their arguments (strace -y: `3</path>`).
DATA_CALLS = {"write", "pwrite64"}
FD_PATH = re.compile(r"^\d+<([^<>]*)>")


def ms(seconds: float) -> str:
    return f"{seconds * 1000:.1f}"


def load_results(out: Path) -> dict[str, dict]:
    results = {}
    for path in sorted((out / "hyperfine").glob("*.json")):
        data = json.loads(path.read_text())
        (result,) = data["results"]
        results[path.stem] = result
    return results


def timing_rows(results: dict[str, dict]) -> list[str]:
    rows = [
        "| Name | Mean ± σ (ms) | Median (ms) | Min–max (ms) | Runs | Exit codes |",
        "|---|---|---|---|---|---|",
    ]
    for name, r in results.items():
        codes = Counter(r.get("exit_codes", []))
        seen = ", ".join(f"{code} ×{n}" for code, n in sorted(codes.items()))
        rows.append(
            f"| `{name}` | {ms(r['mean'])} ± {ms(r['stddev'])} | {ms(r['median'])} "
            f"| {ms(r['min'])}–{ms(r['max'])} | {len(r['times'])} | {seen} |"
        )
    return rows


def comparison_rows(results: dict[str, dict]) -> list[str]:
    rows = [
        "| Command | Profile | Before (ms) | After (ms) | Δ mean (ms), 95 % CI | Δ median (ms) | Welch p | Mann–Whitney p |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for name, after in results.items():
        if not name.startswith("after-"):
            continue
        before = results.get("before-" + name.removeprefix("after-"))
        if before is None:
            continue
        command, profile = name.removeprefix("after-").split("-", 1)
        a, b = after["times"], before["times"]
        welch = stats.ttest_ind(a, b, equal_var=False)
        low, high = welch.confidence_interval(0.95)
        mann_whitney = stats.mannwhitneyu(a, b, alternative="two-sided")
        rows.append(
            f"| {command} | {profile} | {ms(before['mean'])} ± {ms(before['stddev'])} "
            f"| {ms(after['mean'])} ± {ms(after['stddev'])} "
            f"| {ms(after['mean'] - before['mean'])} [{ms(low)}, {ms(high)}] "
            f"| {ms(after['median'] - before['median'])} "
            f"| {welch.pvalue:.3g} | {mann_whitney.pvalue:.3g} |"
        )
    return rows


def pair_rows(out: Path) -> list[str]:
    path = out / "pairs.tsv"
    if not path.exists():
        return ["No pairs were measured (pairs.tsv is missing)."]
    rows = [
        "| Command | Profile | Pairs | Before median (ms) | After median (ms) | Median of the differences (ms) | Interquartile range (ms) | Wilcoxon p |",
        "|---|---|---|---|---|---|---|---|",
    ]
    lines = path.read_text().splitlines()
    head = lines[0].split("\t")
    pairs: dict[tuple[str, str], list[tuple[float, float]]] = {}
    for line in lines[1:]:
        cell = dict(zip(head, line.split("\t")))
        pairs.setdefault((cell["command"], cell["profile"]), []).append((float(cell["before_ms"]), float(cell["after_ms"])))
    for (command, profile), times in pairs.items():
        before = [b for b, _ in times]
        after = [a for _, a in times]
        differences = [a - b for b, a in times]
        quartiles = statistics.quantiles(differences, n=4) if len(differences) > 1 else [differences[0]] * 3
        p = float(stats.wilcoxon(differences)[1]) if len(differences) > 1 else float("nan")
        rows.append(
            f"| {command} | {profile} | {len(times)} | {statistics.median(before):.2f} | {statistics.median(after):.2f} "
            f"| {statistics.median(differences):+.2f} | {quartiles[0]:+.2f} to {quartiles[2]:+.2f} | {p:.3g} |"
        )
    return rows


def on_profile(profile: str):
    marker = re.compile(re.escape(profile) + r'(?=[/">])')
    return lambda line: marker.search(line) is not None


def relative(path: str, profile: str) -> str | None:
    if path == profile:
        return "."
    if path.startswith(profile + "/"):
        return path[len(profile) + 1 :]
    return None


def profile_calls(trace: Path, profile: str) -> tuple[Counter, list[str]]:
    counts: Counter = Counter()
    written: dict[str, None] = {}
    inside = on_profile(profile)
    for line in trace.read_text(errors="replace").splitlines():
        match = CALL.match(line)
        # Was: `if match is None or not inside(line): continue`, which
        # counted a write whose data named the profile (ledger S17).
        if match is None:
            continue
        call, args = match.groups()
        if call in DATA_CALLS:
            descriptor = FD_PATH.match(args)
            if descriptor is None or relative(descriptor.group(1), profile) is None:
                continue
        elif not inside(line):
            continue
        paths = [
            rel
            for quoted, described in PATH_IN_CALL.findall(args)
            if (rel := relative(quoted or described, profile)) is not None
        ]
        if call == "openat":
            if any(flag in args for flag in WRITE_FLAGS):
                counts["open to write"] += 1
                written.update(dict.fromkeys(paths[-1:]))
            else:
                counts["open to read"] += 1
            continue
        for kind, calls in KINDS:
            if calls is not None and call in calls:
                counts[kind] += 1
                if kind in ("rename", "link", "unlink", "rmdir", "mkdir"):
                    written.update(dict.fromkeys(paths))
                break
    return counts, list(written)


def profile_rows(out: Path) -> tuple[list[str], list[str]]:
    profiles = dict(
        line.split("\t", 1)
        for line in (out / "profiles.tsv").read_text().splitlines()
        if line.strip()
    )
    header = "| Name | " + " | ".join(kind for kind, _ in KINDS) + " |"
    rows = [header, "|---|" + "---|" * len(KINDS)]
    paths = []
    for name, profile in profiles.items():
        trace = out / "strace" / f"{name}.trace"
        if not trace.exists():
            continue
        counts, written = profile_calls(trace, profile)
        rows.append(f"| `{name}` | " + " | ".join(str(counts[kind]) for kind, _ in KINDS) + " |")
        paths.append(f"- `{name}` ({len(written)}): " + (", ".join(f"`{p}`" for p in written) or "nothing"))
    return rows, paths


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 64
    out = Path(sys.argv[1])
    results = load_results(out)
    lines = ["# Start-up benchmark", "", "Made by `scripts/storage-bench.sh`; the method is in its header.", ""]
    for title, file in (("The machine, before the runs", "machine.txt"), ("The machine, after the runs", "machine-after.txt")):
        if (out / file).exists():
            lines += [f"## {title}", "", "```text", (out / file).read_text().rstrip(), "```", ""]
    lines += ["## Timings", ""] + timing_rows(results) + [""]
    lines += ["## Before and after", ""] + comparison_rows(results) + [""]
    lines += ["### Before and after, in alternating pairs", ""] + pair_rows(out) + [""]
    rows, paths = profile_rows(out)
    lines += ["## What one start did to its profile (strace -f -y)", ""] + rows + [""]
    lines += ["### The paths each start wrote, in order", ""] + paths + [""]
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
