# Mutation checks of the storage work

Each file here lists the mutation checks of one entry of the
[storage ledger](../ledger.md). A mutation check shows that a test guards a
fix:

1. **Undo the fix.** Replace the code that makes the fix with code that does
   not.
2. **Run the test** that is meant to catch it. The test must fail ("go red").
3. **Restore the file**, and compare it byte for byte with the copy taken
   before step 1.

A test that stays green under its mutation does not guard the fix. The
ledger records every such finding and what was done about it.

## Running

```text
scripts/mutation-check.py docs/storage/mutations/s9-settings.json target/scratch/storage/mutations/s9-settings
```

The script prints one line per mutation:

```text
<id> TAB <verdict> TAB restored=<True|False> TAB <test> TAB <what>
```

`<verdict>` is one of:

| Verdict | Meaning |
|---|---|
| `red` | the test failed, as it must |
| `GREEN` | the test passed: it does not guard the fix |
| `COMPILE ERROR` | the mutation does not compile |
| `NO TEST RAN` | no test matched the name |
| `TEXT FOUND n TIMES` | the text to replace is not unique (`n` is 0 or more than 1) |
| `OTHER LINT` | (`check` = `clippy`) clippy failed, but not with the named lint |
| `retired` | the entry has `retired`: the code it guarded was replaced or found redundant, so it is reported with the reason, and not run |
| `IN A COMMENT` | the text to replace is found only in comment lines (code commented out rather than deleted), so it is not run: mutating a comment proves nothing |

The script exits 0 only if every mutation was `red` or `retired` and every
file was restored exactly. The test logs go to the output folder.

## Format

A spec is a JSON list. Each mutation is an object:

| Key | Meaning |
|---|---|
| `id` | the mutation's name in the ledger, such as `M9a` |
| `what` | the defect the mutation brings back, in a few words |
| `file` | the file it changes, relative to the workspace root |
| `package` | the cargo package whose test is run |
| `old` | the text it replaces; it must occur exactly once in `file` |
| `new` | the text that replaces it |
| `test` | the name of the test that must go red |
| `target` | optional cargo arguments selecting the test binary; `["--lib"]` if left out |
| `check` | optional: `"test"` (the default), or `"clippy"` for a fix no test can observe, such as a folder made durable on the real file system. Then `test` names the lint that must fire (`disallowed_methods`), and the mutation is checked with `cargo clippy -p PACKAGE --all-targets -- -D warnings` |
| `retired` | optional: why the mutation is no longer run (its fix was replaced, or found redundant). The entry stays, so the ledger's references keep their meaning |

## Files

| File | Ledger entry | What it checks |
|---|---|---|
| `s3-layout.json` | S3 | where the roots are |
| `s4-gaze-fs.json` | S4 | the durable writes |
| `s5-lock.json` | S5 | the instance lock |
| `s6-salvage.json` | S6 | backups of damaged site stores |
| `s6-reconcile.json` | S6 (part 2) | start-up's repair, the busy flag and the barrier, durable folders |
| `s7-migrate.json` | S7 | moving an old profile, the conversion, `MIGRATED.txt`, crash enumeration on every core, and the lint that keeps every folder durable |
| `s8-trace.json` | S8 | real start-ups as behaviours of the model: run with `F1R3GAZE_TLA2TOOLS` set (and `F1R3GAZE_TRACE_QUICK=1` for speed) |
| `s8-kills.json` | S8 (part 2) | real kills as behaviours of the model (the `crash-points` and `storage-trace` features) |
| `s9-settings.json` | S9 | `settings.toml` |
| `s12-themes.json` | S12 | theme files |
| `s12-system-theme.json` | S12 | the system's light or dark preference |
| `s12-chrome.json` | S12 (part 3) | the Appearance panel, the theme operations, and the scheme in pages and in the window (L13) |
| `s13-window-state.json` | S13 | state files and the window's geometry |
| `s13-window.json` | S13 (part 2) | the window made where it was and kept in `window.json`, full screen, the zoom's range (L15), History's key on macOS (L16) and the keys the tips name (L17) |
| `s14-switch-over.json` | S14 (part 1) | the switch-over to the five roots: start-up, the lock policy, read-only sessions, the engine and the chrome on the new profile |
