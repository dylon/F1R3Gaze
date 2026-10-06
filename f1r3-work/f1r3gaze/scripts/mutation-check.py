#!/usr/bin/env python3
"""Mutation checks for the storage ledger (docs/storage/ledger.md).

For each mutation in a spec, the script:

1. copies the file it changes into OUTDIR (the original, kept for the
   comparison in step 5);
2. replaces the spec's `old` text, which must occur exactly once, with its
   `new` text;
3. runs the named test (`cargo test --locked --offline -p PACKAGE [TARGET]
   -- TEST`);
4. restores the file from the copy;
5. compares the restored file with the copy byte for byte.

A mutation passes when its test goes red (a failing test) and the file is
restored exactly. The script prints one tab-separated line per mutation and
exits 0 only if every mutation passed.

    scripts/mutation-check.py docs/storage/mutations/s9-settings.json target/scratch/storage/s9

A spec is a JSON list of objects with the keys `id`, `what`, `file` (relative
to the workspace root), `package`, `old`, `new`, `test`, and optionally
`target` (a list of cargo arguments, default `["--lib"]`) and `check`.

A mutation whose fix no longer exists, because the code it guarded was
replaced, keeps its entry with `retired` set to the reason: it is reported
as retired, and not run. A Rust file's old text that is found only inside
comment lines (code commented out rather than deleted) is reported as
`IN A COMMENT`, and not run: mutating a comment proves nothing.

`check` is `"test"` (the default) or `"clippy"`. A fix whose effect no test
can observe, such as a folder made durable on the real file system, is
guarded by a lint instead: then the mutation must make
`cargo clippy -p PACKAGE --all-targets -- -D warnings` fail with the lint
named in `test` (for example `disallowed_methods`).
"""

import filecmp
import json
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def lint_verdict(run: subprocess.CompletedProcess, lint: str) -> str:
    """What a clippy run says about its mutation: red when the lint fires."""
    if "error[" in run.stderr:
        return "COMPILE ERROR"
    if run.returncode != 0 and f"clippy::{lint}" in run.stderr.replace("-", "_"):
        return "red"
    if run.returncode != 0:
        return "OTHER LINT"
    return "GREEN"


def verdict(run: subprocess.CompletedProcess) -> str:
    """What a test run says about its mutation."""
    if "error[" in run.stderr or "error: could not compile" in run.stderr:
        return "COMPILE ERROR"
    if "test result: FAILED" in run.stdout:
        return "red"
    if "running 0 tests" in run.stdout and "test result: ok. 0 passed" in run.stdout:
        return "NO TEST RAN"
    return "GREEN"


def only_in_comments(text: str, old: str) -> bool:
    """Whether every line `old` covers in `text` is a `//` comment."""
    start = text.index(old)
    first = text.rfind("\n", 0, start) + 1
    end = start + len(old)
    last = text.find("\n", end)
    lines = text[first:last if last != -1 else len(text)].split("\n")
    return all(line.lstrip().startswith("//") or not line.strip() for line in lines)


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 64
    spec = json.loads(pathlib.Path(sys.argv[1]).read_text())
    out = pathlib.Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    scratch_tmp = ROOT / "target" / "scratch" / "tmp"
    scratch_tmp.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, TMPDIR=str(scratch_tmp))
    rows = []

    def row(text: str) -> None:
        # One line per mutation as it finishes, so a long run shows progress.
        rows.append(text)
        print(text, flush=True)

    for m in spec:
        if m.get("retired"):
            row(f"{m['id']}\tretired\trestored=True\t{m['test']}\t{m['what']} (retired: {m['retired']})")
            continue
        path = ROOT / m["file"]
        original = out / f"{path.name}.{m['id']}.orig"
        shutil.copyfile(path, original)
        text = path.read_text()
        count = text.count(m["old"])
        if count != 1:
            row(f"{m['id']}\tTEXT FOUND {count} TIMES\trestored=True\t{m['test']}\t{m['what']}")
            continue
        if path.suffix == ".rs" and only_in_comments(text, m["old"]):
            row(f"{m['id']}\tIN A COMMENT\trestored=True\t{m['test']}\t{m['what']}")
            continue
        path.write_text(text.replace(m["old"], m["new"]))
        clippy = m.get("check", "test") == "clippy"
        if clippy:
            command = ["cargo", "clippy", "--locked", "--offline", "-p", m["package"], "--all-targets", "--", "-D", "warnings"]
        else:
            command = ["cargo", "test", "--locked", "--offline", "-p", m["package"]]
            command += m.get("target", ["--lib"]) + ["--", m["test"]]
        try:
            run = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
        finally:
            shutil.copyfile(original, path)
        (out / f"{m['id']}.log").write_text(run.stdout + run.stderr)
        restored = filecmp.cmp(original, path, shallow=False)
        found = lint_verdict(run, m["test"]) if clippy else verdict(run)
        row(f"{m['id']}\t{found}\trestored={restored}\t{m['test']}\t{m['what']}")
    passed = all(("\tred\t" in r or "\tretired\t" in r) and "restored=True" in r for r in rows)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
