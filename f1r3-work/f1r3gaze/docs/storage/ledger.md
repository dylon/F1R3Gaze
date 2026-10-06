# Storage ledger

This ledger records how F1R3Gaze's storage was moved to the operating
system's standard places and made crash-safe. For each step it gives what was
observed, the hypotheses tested, the experiment for each, the raw result, and
the verdict.

Rules for every entry:
- One variable changes per experiment.
- Every fix gets a **mutation check**: comment the fix out, confirm the
  regression test goes red, then restore it. A restored file is compared byte
  for byte (`cmp`) with the copy taken before the mutation. (No `git stash`
  or `git reset`.)
- Raw evidence goes to `target/scratch/storage/<entry>/`. Every command's
  output is kept there with `tee`, and this ledger quotes the lines that
  matter.
- What reproduces an experiment is versioned, because `cargo clean` removes
  `target/` (see "Lost evidence" below). The mutation runner is
  `scripts/mutation-check.py`, and each entry's mutations are in
  `docs/storage/mutations/<entry>.json`.

The design is in [README.md](README.md). The chrome defects found along the
way are entries L12 to L17 of the [chrome defect ledger](../ui/ledger.md):
L13, the pages' colour scheme (S12, part 3); L15, the zoom's range, L16,
History's key on macOS, and L17, the keys the tips name (S13, part 2).

Entry format: **ID | hypothesis | prediction | experiment | raw result | verdict | evidence**.

---

## S1 — Baseline: how the single-folder profile behaves today

**Goal.** Record, before any change, where the profile writes, how durably,
and what it does with damaged files, so that every later entry has a
reference to compare against.

**Baseline binary.** `target/release/f1r3gaze`, built from `3d5ba26` with
`cargo build --release --locked`. Its sha256 is `d4d10cae…0e638a81`: the
same binary as ledger L11. A copy is kept as
`target/scratch/storage/s1/f1r3gaze-baseline`.

**Census of writers.**
- 0 files under `crates/` call `sync_all` or `sync_data` (`rg -c`).
- Twelve source files write files: f1r3c (4 calls), gaze-blob (4),
  gaze-broker (3), gaze-shard `keys.rs` (3), gaze-shell `chrome.rs` (5),
  `engine.rs` (3), `main.rs` (2), `profile.rs` (4), `test_support.rs` (4),
  `ui_state.rs` (3), gaze-store (7), gaze-wallet `wallets.rs` (3).
- The "atomic" writes (`ui_state.rs`, `grants.tsv`, key files, the content
  cache, store compaction, the store index) are write-then-rename without a
  sync. `wallets.tsv` and `wallet-active` are plain overwrites.

**Hypotheses.** Each was tested with a throw-away probe,
`crates/gaze-shell/tests/zz_s1_probe.rs`, which passed only if the defect was
present. The probe was then removed, leaving the tree as before (`git status`
clean). Its source is kept in `target/scratch/storage/s1/`.

| ID | Hypothesis | Prediction | Raw result | Verdict |
|---|---|---|---|---|
| H1 | A corrupt `workspace.json` is lost at the next save | `UiState::load` returns defaults; `save` replaces the file; no copy survives | `theme=dark tabs=0 visits=0`; after save 132 bytes; 0 copies of the original | confirmed |
| H2 | One bad byte in `settings.conf` resets the settings | `Settings::load` returns defaults and writes `TEMPLATE` over the file | observers fell back to `http://localhost:40453`, `quorum=2`, `https_only=false`; the file is the template; 0 copies | confirmed |
| H3 | An undecodable `user-id` is regenerated over the original | a new id; the original gone | new id `57d7…6ba`; 0 copies | confirmed |
| H3u | An unreadable `user-id` changes on every start | two calls give two ids | `4c80…09c3` then `8ecd…3849` | confirmed |
| H4 | Grant lines that do not parse are erased at the next save | 1 of 2 lines loaded; the bad one gone after save | `loaded 1 of 2 lines`; only `a.example` left; 0 copies | confirmed |
| H4b | One bad byte in `grants.tsv` loses every decision | 0 decisions loaded; the next save empties the file | `loaded 0 decisions`; 0 bytes after save; 0 copies | confirmed |
| H5 | A relative `$XDG_DATA_HOME` is honoured (XDG says to ignore it) | with `XDG_DATA_HOME=rel`, the profile appears under the working directory | `./rel/f1r3gaze/{settings.conf,user-id}` created | confirmed |

**How the baseline writes** (strace of a fresh and then a steady headless
start on one scratch profile, `s1-strace.txt`):
- **Fresh start:** `mkdir(profile, 0777)`, then `openat(user-id,
  O_WRONLY|O_CREAT|O_TRUNC, 0666)` and the same for `settings.conf`. The
  resulting modes are 0755 for the folder and 0644 for the files.
- **Steady start:** two reads, no writes.
- **No call** in the fsync family on either start.

**Snapshot harness.** A full run of the baseline binary:
`scripts/ui-snapshots.sh --after --bin <baseline> --out
target/scratch/storage/s1/harness` with the default work directory.
- 71 scenes ran, 0 failed, and all 57 checks passed. These captures are the
  reference that later entries compare against.
- **A false start.** The first attempt passed a relative `--work`. The harness
  builds the demo site's URL as `file://$WORK/site`, so a relative path made
  `file://target/...`, which is not a valid URL. Every scene stopped at
  "Can't open this page" (`harness-relative-work-failed.log`). The work
  directory is also part of the captured address bar, so every comparable
  run must use the default one.

**Start-up time** (hyperfine 1.20, `taskset -c 2-9`, governor `performance`,
maximum 4.56 GHz, boost on; `bench/`):

| Command | Profile | Mean ± σ | Range | Runs |
|---|---|---|---|---|
| `--headless gaze://newtab --timeout 5` | steady | 258.6 ± 4.1 ms | 255.8–282.9 ms | 50 |
| `--headless gaze://newtab --timeout 5` | fresh (`--prepare` removes it) | 261.7 ± 5.6 ms | 255.8–283.2 ms | 50 |
| `wallet list` | steady | 2.5 ± 0.4 ms | 2.1–4.4 ms | 100 |
| `wallet list` | fresh | 2.6 ± 0.2 ms | 2.3–3.3 ms | 100 |

The headless page run waits about 256 ms for the page to settle, which hides
storage costs. `wallet list` opens the whole engine (settings, user id,
keystore, wallets, bridge, broker) and loads no page, so it measures opening
the profile. S15 repeats both.

---

## S2 — The start-up protocol, model-checked

**Goal.** Before writing the storage code, specify start-up (lock, barrier,
migrate, marker, reconcile) in TLA+ and check that no crash, at any step,
loses or overwrites data.

**Model.** `docs/storage/tla/ProfileStartup.tla` (the design is in
[README.md](README.md), section "The formal model"):
- The file system follows Pillai et al.'s abstract persistence model:
  directory operations are atomic (A1); they reach the disk in any subset and
  order unless a directory fsync makes them durable (A2); data is durable
  only after a file fsync (A3).
- Two kinds of crash: a process crash keeps everything the kernel has, and a
  power cut keeps only durable state plus any subset of the pending directory
  operations.
- Three legacy items cover the three ways an item moves: `u` (same file
  system), `k` (another file system), `s` (converted, like `settings.conf`).
  One managed file `m` (like `session.json`) can start missing, valid or
  corrupt.

**Hypotheses.** H1 to H8 each name a deliberate defect: if it is present,
TLC must find the invariant it breaks.

| ID | Fault | Predicted invariant | Measured (exit 12) | Trace | Verdict |
|---|---|---|---|---|---|
| H1 | `regenerate-before-backup` | CorruptKept | CorruptKept | 14 states | confirmed |
| H2 | `replace-destination` | NoOverwrite | NoOverwrite | 3 | confirmed |
| H3 | `marker-first` | MarkerImpliesComplete | MarkerImpliesComplete | 5 | confirmed |
| H4 | `no-fsync-before-publish` | PublishedComplete | PublishedComplete | 9 | confirmed |
| H5 | `no-dir-fsync-before-unlink` | NoLoss | NoLoss | 6 | confirmed |
| H6 | `resume-stale-temp` | PublishedComplete | PublishedComplete | 9 | confirmed |
| H7 | `no-start-barrier` | NoLoss | NoLoss | 8 | confirmed |
| H8 | `settle-only` | MarkerImpliesComplete | ReadyIsDurable | 13 | refuted; see below |

**Iteration 1.** SANY accepted the spec. The liveness configuration (one
crash) failed `ReadyIsDurable` at depth 21, after 4 215 distinct states.
- **The trace.** A process crash left a temporary file; the next start's
  reconcile swept it (`Unlink`). The managed file was valid, so nothing
  fsynced the folder afterwards, and start-up reported ready with the unlink
  still pending.
- **Why it matters.** The unlink itself is harmless: a power cut only brings
  the temporary file back, and the next start sweeps it again. But
  `ReadyIsDurable` is what allows a start that follows a clean one to skip
  the barrier, so it must hold strictly.
- **Fix in the protocol.** Every sweep of a stale temporary file, and every
  discard after a failed verification, fsyncs its folder before going on.
  The implementation does the same.

**Iteration 2** (`target/scratch/storage/s2/run1`): every configuration
passed, and 7 of the 8 faults broke their predicted invariant.
- **H8.** `settle-only` fsyncs only the copy it finds, instead of running the
  barrier. It broke `ReadyIsDurable` first. Without that invariant it breaks
  `MarkerImpliesComplete`, the predicted hazard, after 6 530 distinct
  states.
- **The one-crash control** (no barrier, one crash) was predicted to pass,
  and failed on `ReadyIsDurable`. Without that invariant it passes (9 464
  distinct states): without the barrier, every data invariant survives any
  single crash. Losing data needs two crashes; leaving operations pending at
  ready needs only one.
- **The runner** (`scripts/storage-model.sh`) now expects `ReadyIsDurable`
  for `settle-only`, and checks the two hazards on their own:
  - `hazard-settle-only-marker`: `settle-only` without `ReadyIsDurable` must
    break `MarkerImpliesComplete`;
  - `hazard-no-barrier-one-crash-pending`: no barrier, one crash, must break
    `ReadyIsDurable`;
  - `control-no-barrier-one-crash-data-safe`: no barrier, one crash, without
    `ReadyIsDurable`, must pass.

**Iteration 3** (`target/scratch/storage/s2/run2`): all 15 runs as
predicted, in 19 s with 12 workers.

| Run | Result | Distinct states | Depth |
|---|---|---|---|
| deep (two crashes, power loss) | no error | 57 350 | 57 |
| wide (four items, one crash) | no error | 25 084 | 63 |
| live (one crash, `Termination` too) | no error | 7 586 | 47 |
| 8 faults | exit 12 on the invariants above | — | 3–14 |
| 2 hazards | exit 12 on the invariants above | — | 13 |
| control: a missing fsync without power loss | no error | 25 930 | — |
| control: no barrier, one crash, data only | no error | 9 464 | — |

**Prediction refuted.** The design review predicted 5·10⁵ to 10⁷ distinct
states for the deep run. TLC found 57 350, about ten times fewer than the
low end. Each crash restarts the program from the beginning, so most crash
points lead to states already seen.

**What this establishes.**
1. **The barrier is needed.** Without it, two crashes can lose an item
   (H7). Fsyncing only the copy found is not enough (H8).
2. **Order of operations.** Every write publishes only after a file fsync
   (H4, H6). Every source is removed only after the folder holding its copy
   is fsynced (H5). A corrupt file is moved to a backup before its
   replacement is written (H1).
3. **Once ready, nothing is pending.** The next start may therefore skip the
   barrier when the previous one reached ready (`ReadyIsDurable`, with the
   sweep fix of iteration 1).
4. **Liveness.** With finitely many crashes, start-up always finishes.

---

## S3 — Where the roots are

**Goal.** Map the five storage classes (config, data, state, cache, runtime)
to the platform's folders: the XDG Base Directory specification v0.8 on
Linux, Apple's conventions on macOS, Known Folders on Windows. The mapping
must be testable for all three platforms on any one of them.

**Design.** `crates/gaze-shell/src/profile/layout.rs`.
- `platform_layout(platform, &Machine)` is a pure function of the platform
  and a `Machine` (home, environment, the Windows application-data
  folders). `Machine::detect()` is the only impure function.
- `--profile DIR` (or `F1R3GAZE_PROFILE`) gives a hermetic portable root:
  `DIR/{config,data,state,cache,runtime}`, with no system layers.

**Hypothesis S3-H1.** `Path::is_absolute` and `std::env::split_paths` judge a
path by the host, not by the target platform. So a Windows path cannot be
checked on Linux, and the mapping cannot be tested for every platform on one
machine.
- *Prediction:* on Linux, `Path::new(r"C:\Users\u").is_absolute()` is false.
- *Raw result:* false.
- *Verdict:* confirmed. `layout.rs` uses `is_absolute_on(platform, path)` and
  `split_colons` instead, both pure.

**Tests** (13, all passing). The XDG matrix sets each of the five variables
(`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`,
`XDG_RUNTIME_DIR`) to one of four states (unset, empty, relative,
absolute): $`4^5 = 1024`$ cases, each checked against the specification's
rule that an unset, empty or relative value means the default.

**Mutation checks** (`target/scratch/storage/s3/mutations.tsv`). All six red,
and every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M3a | relative XDG values accepted | `every_xdg_environment_maps_by_the_specification` |
| M3b | empty XDG values taken as set | `every_xdg_environment_maps_by_the_specification` |
| M3c | `--profile` not made absolute | `a_profile_argument_is_a_portable_root_made_absolute` |
| M3d | Windows config under the local application data | `windows_prefers_known_folders_then_the_environment` |
| M3e | macOS following XDG variables | `macos_uses_the_bundle_id_its_caches_and_tmpdir` |
| M3f | XDG lists keep relative entries | `xdg_lists_keep_their_order_and_drop_bad_entries` |

**Verdict.** The roots follow each platform's convention, and H5 (a relative
`$XDG_DATA_HOME` honoured) is fixed in the new layout. The old
`profile::default_dir` stays until the switch-over (step 9), as the oracle
for finding legacy profiles.

---

## S4 — Durable writes

**Goal.** One crate, `gaze-fs`, through which every write to the profile
goes, following the order the S2 model proves safe. Then the existing
writers are moved onto it without changing what any of them does.

**Design.** `crates/gaze-fs` (std only, `#![forbid(unsafe_code)]`).
- `write_atomic`: write a private temporary file, sync it, rename it over
  the target, then sync the folder.
- `publish_no_replace`: give a file a free name without ever replacing
  anything.
- `copy_verified` and `move_no_replace`: move a file, across file systems
  too.
- `sweep_temps`: clean up after a crash.
- `MemFs`, an in-memory file system, models the assumptions A1–A3. Tests
  crash every write at every operation, then cut the power keeping every
  possible subset of the pending directory operations.

**Hypothesis S4-H1 (Windows).** NTFS has no directory fsync, so A2 cannot be
met the way it is on Unix.
- *Prediction:* the S2 model, with NTFS semantics, loses a published name
  unless the file is synced after it is linked or renamed. The semantics:
  - W1: a power cut keeps a prefix of each volume's pending operations;
  - W2: `FlushFileBuffers` commits the volume's log.
- *Raw result:* `fault-ntfs-no-commit-name` breaks `NoLoss` (trace of 12
  states). The NTFS configuration with the fix passes (97 942 distinct
  states).
- *Verdict:* confirmed. `StdFs` syncs the file after each link or rename on
  Windows (`commit_name`), and `MemFs::ntfs()` emulates W1 and W2.

**Model rerun** (`target/scratch/storage/s4/model-run/`). All 18 runs as
predicted, in 35 s:
- deep: 57 350 distinct states; wide: 25 084; ntfs: 97 942; live: 7 586.
- 9 faults, 2 hazards and 2 controls, each with its predicted result.

**Mutation checks** (`target/scratch/storage/s4/mutations-2.tsv`). All eight
red, every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M4a | `write_atomic` without the file sync before the rename | `write_atomic_survives_every_crash` |
| M4b | publish without syncing the new name's folder before unlinking the old | `publishing_loses_nothing_and_shows_nothing_partial` |
| M4c | the no-hard-link path without its check that the name is free | `nothing_is_ever_replaced` |
| M4d | the sweep without syncing its folder | `sweeping_removes_only_other_processes_temporary_files` |
| M4e | the verified copy without its comparison | `a_copy_that_reads_back_wrong_is_never_published` |
| M4f | `write_atomic` without the final folder sync | `completed_writes_survive_any_power_cut` |
| M4g | publish without syncing the old name's folder | `completed_writes_survive_any_power_cut` |
| M4h | NTFS emulation without committing a file's new name (W2) | `moving_across_file_systems_loses_nothing` |

**Hardening the existing writers (plan step 5).** Each writer now goes
through `gaze-fs`, with no change in layout or behaviour:
- the key files, written private from creation;
- grants, wallets, the site stores' salvage and compaction, and the store
  index;
- `workspace.json`, the chrome's exports and logs, and `main.rs`'s export and
  log.

The freshness records (`gaze-shard/src/fresh.rs`) are new, wired in memory
for now. A clippy lint (`clippy.toml`, `disallowed-methods`) rejects
`std::fs::{write, rename, copy, File::create}` outside the crates' tests.

**Behaviour-neutral, measured.**
- The step-5 binary (`target/scratch/storage/s4/f1r3gaze-step5`, sha256
  `9e879dde…02ada4ee2f`) ran the whole harness: 71 scenes, 0 failed, and 57
  of 57 checks pass.
- All 72 images are byte-identical to the S1 baseline
  (`target/scratch/storage/s4/harness-vs-s1.txt`).
- The wasm reach build still links (`gaze_reach.wasm`, 490 577 bytes).

**Verdict.** Every write the profile makes is ordered as the model requires,
and each ordering step has a test that fails without it.

---

## S9 — `settings.toml`

**Goal.**
- Settings in TOML, layered over system-wide files.
- Every problem named by file, line and column, and nothing else lost
  because of it.
- Written back only by changing one value, with every comment kept.

**Design.** `crates/gaze-shell/src/profile/settings.rs`.
- **Reading.** `toml::de::DeTable` keeps spans. Each of the 14 settings has
  its own decoder, which assigns only a value that passed its checks.
- **Writing.** `toml_edit`, replacing a value in place.
- **One parser.** Both crates parse with the same `toml_parser` 1.1.3, so the
  reader and the writer agree on what is valid TOML.
- **Theme names.** `ThemeChoice` and the theme-name rules live in
  `theme.rs`.

**Predictions and results.** Each test's expected output was written before
the first run (`target/scratch/storage/s9-settings-tests-1.log`). The first
run: 52 passed and 4 failed.

| ID | Prediction | Raw result | Verdict |
|---|---|---|---|
| S9-P1 | Choosing a theme in the template puts `theme = …` right after `[appearance]`: the template's comments belong to the next header | as predicted | confirmed |
| S9-P2 | A missing table is appended as `\n[table]\nkey = …\n` | as predicted | confirmed |
| S9-P3 | A replaced value keeps its spaces and trailing comment | as predicted | confirmed |
| S9-P4 | In a file that mixes CRLF and LF, line endings are left as they are | the `[appearance]` header's CRLF became LF: `toml_edit` writes headers out itself | refuted; documented, test pinned |
| S9-P5 | Adding to an inline table gives `{ a = true, b = … }` | `{ a = true , b = … }`: the space before `}` belongs to the last value | refuted; fixed (`insert_inline` moves the space) |
| S9-P6 | `theem` at the top level is suggested as `theme` | no suggestion: in standard Levenshtein distance a swap costs 2, over the limit of 1 for five letters | refuted; fixed (optimal string alignment, where a swap costs 1) |
| S9-P7 | Column numbers count characters | the test's second input was not valid TOML | test defect; fixed |
| S9-P8 | A table known only through its subtable gets its own header before it | `[appearance]\ntheme = …\n[appearance.extra]…` | confirmed |

The second run passed all 24 tests (`s9-settings-tests-2.log`).

**Mutation checks** (`target/scratch/storage/s9/mutations.tsv`). The first
run: 9 of 10 red.
- **M9d survived.** Leaving the byte-order mark in did not break
  `a_byte_order_mark_and_crlf_line_endings_are_read`.
- *Hypothesis:* the parser accepts the mark itself, so stripping it only
  keeps the first line's columns as an editor counts them, and no test
  looked at a column on that line.
- *Experiment:* a diagnostic on line 1 of a file that starts with the mark
  must be at column 1.
- *Result:* the new assertion passes with the strip. With M9d applied it
  fails (`s9/rerun/mutations.tsv`).
- *Verdict:* confirmed. 10 of 10 red, every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M9a | `Table::insert` on an existing key (the comment above it is lost) | `a_value_is_replaced_in_place_keeping_every_comment` |
| M9b | the replaced value's spaces and comment not kept | `a_value_is_replaced_in_place_keeping_every_comment` |
| M9c | system files applied most important first | `layers_apply_lowest_first_and_the_users_file_last` |
| M9d | the byte-order mark not stripped | `a_byte_order_mark_and_crlf_line_endings_are_read` |
| M9e | CRLF files written back with bare line feeds | `a_byte_order_mark_and_crlf_survive_an_edit` |
| M9f | the inline-table space not moved | `dotted_and_inline_tables_are_edited_where_they_are` |
| M9g | suggestions by standard Levenshtein distance | `unknown_and_misplaced_settings_are_named_with_a_suggestion` |
| M9h | an unchanged setting rewritten anyway | `saving_a_theme_writes_only_when_it_changes` |
| M9i | a decoder that writes before it checks | `an_unusable_value_keeps_the_layer_below_and_says_which` |
| M9j | Windows device names accepted as theme names | `theme_names_are_portable_file_names` |

**Bounded nesting.** A file nesting 100 000 arrays deep is refused as invalid
TOML, by both parsers, without exhausting the stack
(`deep_nesting_is_refused_not_a_stack_overflow`). Neither crate's
`unbounded` feature is enabled.

**Verdict.** The settings file reads and writes as designed. The template
documents every setting: uncommenting its example lines gives exactly the
built-in defaults. Not wired in yet: `profile::Settings` (`settings.conf`)
serves the running browser until the switch-over (plan step 9).

---

## S5 — The instance lock

**Goal.** One F1R3Gaze at a time writes a profile. A second one is told who
holds it. A crash never leaves a stale lock.

**Design.** `crates/gaze-shell/src/profile/lock.rs`.
- **Two locks, taken in order.**
  1. The anchor, `<data>/instance.lock`, guards the data root, where two
     writers could damage the append-only site stores.
  2. The runtime lock, `<runtime>/instance.lock`, sits in the folder the
     platform keeps for such files. On Linux it has the sticky bit, which
     the XDG specification names as the way to keep cleaners away.
- **Both are `File::try_lock`.** That is `flock` on Unix and `LockFileEx` on
  Windows; the operating system releases them when the process ends.
- **Release order.** The locks are released in reverse, the runtime lock
  first.
- **Who holds it.** Each lock file, and an `instance.pid` beside it, records
  the pid, the mode, the start time and the data root. The `instance.pid`
  files exist because Windows forbids reading a locked file's bytes.
- **When locks fail.**
  - A file system without locks is reported, and the other lock guards
    alone.
  - When neither lock can be taken, or the anchor cannot be created, the
    profile can only be read.

**Hypotheses.**

| ID | Hypothesis | Prediction | Raw result | Verdict |
|---|---|---|---|---|
| S5-H1 | `flock` conflicts between two opens of one file in one process | a second `acquire` in the test process gets `Held`, naming this pid | `Held`, pid and mode as recorded | confirmed |
| S5-H2 | The OS releases the lock of a process that dies without running any destructor | a child process holds the lock; after `SIGKILL` it is free | `Held` naming the child's pid, then free after the kill | confirmed |
| S5-H3 | The anchor keeps a second writer out when the runtime lock file is removed, or when the second process sees another runtime folder | both get `Held` on the anchor | as predicted | confirmed |
| S5-H4 | The child's line `F1R3GAZE-LOCK-HELD`, printed with `println!`, can be found as a whole line | the parent reads `held` as a line | **refuted**: the harness prints `test <name> ... ` before the test without a line break, so the parent waited 60 s (`s5-lock-tests-1.log`) | the parent looks for a marker anywhere in a line |

**A defect in the test, found by a mutation.** Under M5b (no lock at all),
`a_holder_that_dies_releases_its_lock` panicked before killing its child.
The child kept running, sleeping, after the run (pid 1020489, reparented,
`--exact profile::lock::tests::child_process_that_holds_a_lock`). It was
stopped. The child is now owned by a guard that kills and reaps it however
the test ends, and the M5b rerun leaves no process behind
(`s5/rerun/mutations.tsv`).

**A test that did not test its claim.** `the_runtime_lock_is_released_first`
first popped the lock's vector itself, so it checked the order the locks
were taken in, not the order `Drop` releases them. `Drop` now goes through
`release()`, which returns the files in the order released, and the test
checks that order.

**Mutation checks** (`target/scratch/storage/s5/mutations.tsv`). All eight
red, every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M5a | no anchor: only the runtime lock | `the_anchor_holds_when_the_runtime_lock_is_removed_or_elsewhere` |
| M5b | the file opened but never locked | `a_holder_that_dies_releases_its_lock` |
| M5c | released in the order taken | `the_runtime_lock_is_released_first` |
| M5d | the runtime lock not sticky | `lock_files_are_private_and_the_runtime_one_sticky` |
| M5e | lock files created 0666 less the umask | `lock_files_are_private_and_the_runtime_one_sticky` |
| M5f | "permission denied" taken for "no locks here" (the session would write on the runtime lock alone) | `a_profile_that_cannot_be_written_cannot_be_locked_for_writing` |
| M5g | who holds the lock not recorded | `a_second_instance_is_refused_and_told_who_holds_the_profile` |
| M5h | the runtime fallback not reported | `a_runtime_fallback_is_reported` |

**Verdict.** The lock behaves as designed on Linux. The Windows path (the
lock and `instance.pid`) builds the same way, but it is exercised only by
the CI job on Windows. Which commands take the lock, and what each does when
it is held, is wired at the switch-over (plan step 9).

---

## S12 (part 1) — Theme files

**Goal.** Themes are files in `<config>/themes/`. The two built-in schemes are
shipped there as `default-dark.css.example` and `default-light.css.example`.
A theme can set the chrome's colours and nothing else.

**Design.** `crates/gaze-shell/src/theme.rs`.
- **The parser.** `parse_palette` reads a restricted subset of CSS: an
  optional `:root { }`, `--gaze-*: #RRGGBB` declarations, `/* */` comments,
  and `#` comments first on a line, as in the old format. Its grammar is in
  the function's documentation. It refuses `@`-rules, other selectors,
  `url()`, `!important`, nested blocks, any other value and files over
  64 KiB, naming the line.
- **Classification.** `scheme_of` calls a palette light when its background
  (`--gaze-bg`, else `--gaze-surface`) has relative luminance above
  $`L^* = \sqrt{0.0525} - 0.05 \approx 0.1791`$: the point where black and
  white text reach the same contrast on it. That is, black text reaches
  $`(L + 0.05) / 0.05`$ and white text $`1.05 / (L + 0.05)`$; the two are equal
  when $`(L + 0.05)^2 = 0.0525`$.
- **Missing tokens.** A theme's missing tokens come from the scheme it
  resembles (`palette_with`). The contrast checks use the same.
- **The catalogue.** `list_themes`, `find_theme` and `resolve` find and
  choose themes. A user's file hides an installed theme of the same name.
  A theme that cannot be used falls back to the built-in scheme the system
  prefers, and says why.

**The defect the old reader had** (now commented out in `theme.rs`, with
the reason). It checked contrast against the dark scheme whatever the
palette. A light palette that left out `--gaze-surface` was therefore
judged against a dark surface.
- *Prediction:* `--gaze-bg: #ffffff; --gaze-text: #111111` is refused under
  the old rule and accepted under the new one.
- *Raw result:* mutation M12j restores the old rule, and
  `a_light_theme_takes_what_it_leaves_out_from_the_light_scheme` goes red
  on exactly that file.
- *Verdict:* confirmed.

**Classification by the threshold, checked exhaustively.** For each of the
256 greys `#gggggg`, `scheme_of` says light exactly when black text has more
contrast on it than white (`palettes_are_classified_by_their_background`).
Moving the threshold to 0.5 (M12e) breaks it.

**Tests.** 17 theme tests, all passing on the first run
(`target/scratch/storage/s12-theme-tests-1.log`):
- an accept corpus of 8 files and a refuse corpus of 17 cases, each with its
  line;
- the two examples read back as exactly the built-in schemes;
- the classification, the catalogue, the fallbacks, and new theme names.

**Mutation checks** (`target/scratch/storage/s12/theme-mutations.tsv`). All
ten red, every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M12a | an eight-digit colour accepted | `theme_files_refuse_everything_else` |
| M12b | `!important` accepted | `theme_files_refuse_everything_else` |
| M12c | `url()` not refused by name | `theme_files_refuse_everything_else` |
| M12d | `#` comments anywhere on a line | `theme_files_refuse_everything_else` |
| M12e | the threshold at luminance 0.5 | `palettes_are_classified_by_their_background` |
| M12f | missing tokens always from the dark scheme | `a_light_theme_takes_what_it_leaves_out_from_the_light_scheme` |
| M12g | an installed theme replaces the user's file of one name | `user_themes_hide_installed_ones_of_the_same_name` |
| M12h | a broken theme falls back to dark whatever the system prefers | `a_theme_that_cannot_be_used_falls_back_and_says_why` |
| M12i | new theme names compared with letter case | `new_theme_names_never_clash` |
| M12j | contrast checked against the dark scheme (the old defect) | `a_light_theme_takes_what_it_leaves_out_from_the_light_scheme` |

The chrome still uses the old entry points (`palette`, `variables`,
`custom_palette`) until the Appearance redesign (plan step 10).

---

## S13 (part 1) — State files and the window's geometry

**Goal.**
- Split `workspace.json` into `state/session.json` and `state/history.json`.
- Keep the window's size and place in `state/window.json`.
- A file written by a newer F1R3Gaze is never taken for damage.

**Design.**
- **`ui_state.rs`.**
  - `SessionState` and `History` are the split of the legacy `UiState`
    (`UiState::split` returns the theme separately, for `settings.toml`).
  - Recording visits, forgetting them and suggesting them moved into
    shared functions, so `UiState` and `History` cannot drift apart.
  - Each state file's `version` is read on its own, before the rest. So a
    newer format is `StateError::Newer` even when its other fields have
    changed shape, never `Corrupt`.
- **`window_state.rs`.** It is pure and not feature-gated, so start-up can
  check `window.json` in headless builds. It holds:
  - the persisted `WindowState`;
  - `plan_restore`, with the visibility rule;
  - `Tracker`, which ignores minimized and degenerate samples, and keeps the
    normal geometry while maximized or full screen;
  - `SaveClock`: 500 ms after the last change, and at least every 3 s.

**Positions are kept in each platform's own units.** Points on macOS,
because winit converts them with the window's scale, which differs between
monitors; physical pixels on Windows and X11; none on Wayland. The
visibility rule's thresholds scale with `σ(M)`, the desktop units per
logical pixel.

**Predictions and results.** The first run: 25 passed and 1 failed
(`target/scratch/storage/s13-window-state-tests-1.log`).

| ID | Prediction | Raw result | Verdict |
|---|---|---|---|
| S13-P1 | Full screen saved on a monitor that is gone returns as "the current monitor" | it returned on the remaining monitor (`On(0)`): that monitor has the saved one's size, and `find_monitor`'s last rule (the plan's "then by origin or size") identifies it as the saved one | the test's premise was wrong. The behaviour matches the plan, and placement uses the same rule. Both cases are now pinned: the same size gives `On(0)`, another size gives `Current` |

The second run: 26 of 26 pass (`s13-window-state-tests-2.log`).

**Mutation checks** (`target/scratch/storage/s13/mutations.tsv`). All twelve
red, every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M13a | any overlap counts as visible | `a_mostly_off_screen_window_is_brought_back` |
| M13b | no lower bound on the title bar | `the_title_bar_must_be_reachable` |
| M13c | thresholds without the scale | `the_title_bar_must_be_reachable` |
| M13d | macOS given the frame's corner | `on_macos_the_surface_is_placed` |
| M13e | Wayland positions used | `wayland_never_positions_a_window` |
| M13f | the normal size followed while maximized | `maximizing_keeps_the_normal_size_and_place` |
| M13g | Windows' minimized position taken for a place | `minimized_and_degenerate_samples_change_nothing` |
| M13h | no three-second bound during a drag | `the_save_clock_waits_for_quiet_but_not_forever` |
| M13i | a newer file read whole first | `a_newer_state_file_is_recognised_whatever_its_shape` |
| M13j | monitors of one name told apart by order | `monitors_are_found_by_uuid_then_name_then_origin_then_size` |
| M13k | positions in another platform's units used | `positions_in_other_units_are_not_used` |
| M13l | a window larger than its monitor kept at its size | `a_window_larger_than_its_monitor_shrinks_to_fit` |

The window itself (`application.rs`, `launch()`) uses these since plan step 11
(S13, part 2).

---

## S12 (part 2) — The system's light or dark preference

**Goal.** Know whether the operating system prefers light or dark, so the
default `theme = "system"` can follow it.
- macOS and Windows report it through winit.
- On Linux winit reports nothing, so F1R3Gaze asks the XDG desktop portal.

**Design.** `crates/gaze-shell/src/system_theme.rs` (feature `window`).
- **The query.** `dbus-send` with a fixed argument list and no shell, killed
  and reaped after 1 s.
- **The protocol.**
  - `ReadOne` first; on `UnknownMethod`, `Read`, whose answer has one more
    variant.
  - `NotFound` means no preference.
  - No bus, no portal or no answer means "unknown, ask again".
  - A missing `dbus-send` stops the queries for the session.
- **Threading.** Queries run one at a time on the `gaze-system-theme`
  thread. A re-query comes at most every 2 s, and start-up waits at most
  100 ms. A missed reply keeps the preference already known.
- **Testing hook.** `F1R3GAZE_SYSTEM_THEME=dark|light|none` fixes the
  preference, for tests and the harness.

**Observation: the portal on this machine** (KDE Plasma, 2026-10-05). The raw
captures and their sha256 sums are in `target/scratch/storage/s12/portal/`;
the tests embed them byte for byte.

| Query | Exit | Output |
|---|---|---|
| `ReadOne` | 0 | `   variant       uint32 1` (dark) |
| `Read` | 0 | `   variant       variant          uint32 1` |
| an unknown key | 1 | `Error org.freedesktop.portal.Error.NotFound: Requested setting not found` |
| an unknown method | 1 | `Error org.freedesktop.DBus.Error.UnknownMethod: No such method “ReadNothing”` |
| an unknown service | 1 | `Error org.freedesktop.DBus.Error.ServiceUnknown: The name is not activatable` |
| a missing socket | 1 | `Failed to open connection to "session" message bus: Failed to connect to socket …: No such file or directory` |
| a refused socket | 1 | `… Connection refused` |
| a socket path too long | 1 | `… Socket name too long` |

**A slip in the capture, recorded.** The first attempt to capture the two
no-bus cases split a `for` loop's words as bash would. zsh does not
(`set -- $v`), so the second query ran against the real bus and wrote three
misnamed files. They were removed, and each case was captured on its own.

**Robustness found while testing.** Executing a script that was just written
can fail with `ETXTBSY` when another thread forks at that moment; a package
upgrade replacing `dbus-send` can cause the same. `run()` tries a busy
program again up to five times, 10 ms apart.

**Tests.** 13, all passing on the first run
(`target/scratch/storage/s12-system-theme-tests-1.log`):
- the captured replies and errors;
- fake `dbus-send` scripts through the real process runner: an answer, an
  old portal, a hung one killed at 200 ms, a missing one;
- the throttle, the start-up budget, a missed reply, and the override.

No process was left behind (`pgrep`, before and after).

**Mutation checks** (`target/scratch/storage/s12/system-theme-mutations.tsv`).
All seven red, every file restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M12k | no second query with `Read` | `an_old_portal_is_asked_again_with_read` |
| M12l | a hung `dbus-send` never killed | `a_hung_dbus_send_is_killed_at_its_deadline` |
| M12m | a missing `dbus-send` asked for again | `a_missing_dbus_send_stops_the_queries` |
| M12n | no interval between queries | `asking_again_is_throttled_and_one_at_a_time` |
| M12o | one missed reply forgets the preference | `an_unknown_answer_keeps_the_preference_already_known` |
| M12p | `NotFound` taken for a failure | `errors_are_classified` |
| M12q | start-up does not wait at all | `start_up_waits_no_longer_than_its_budget` |

The window's use of it came with the switch-over (winit's reports, and
asking again on focus; S14) and with plan step 10 (the Appearance panel,
the window's decorations, and pages following the chrome's scheme, L13;
S12, part 3).

---

## S4 (part 2) — The model in CI

**Change.** CI has a new `model` job (`.github/workflows/ci.yml`). It
downloads `tla2tools.jar` 1.7.4 and checks its sha256 (`936a2620…0e88`).
That is byte-identical to the jar every ledger run used: TLC 2.19 of
2024-08-08. The job then runs `scripts/storage-model.sh --quick --no-scope
--workers 4 --heap 6g`, the size of a CI runner.

**Defect 1: a relative `--jar` was not found.** The runner starts Java inside
its output folder, so a relative jar path no longer resolved
(`ClassNotFoundException: tla2sany.SANY`). It now resolves the path once, at
start.

**Defect 2: one expectation depended on scheduling.**
- *Hypothesis:* `settle-only` breaks `ReadyIsDurable` and
  `MarkerImpliesComplete` at the same depth (both traces are 13 states).
  Which one TLC reports first depends on how its workers are scheduled, so
  the runner's single expected invariant was flaky.
- *Experiment:* the same fault, ten runs, at 1 to 12 workers
  (`target/scratch/storage/ci/settle/settle-only-workers.txt`).
- *Result:* 1, 2 and 4 workers gave `MarkerImpliesComplete` each time; 8
  and 12 workers gave `ReadyIsDurable` three times and
  `MarkerImpliesComplete` once.
- *Verdict:* confirmed. Each invariant is now checked on its own: the run
  `fault-settle-only` leaves `MarkerImpliesComplete` out of its
  configuration, and the existing hazard run leaves `ReadyIsDurable` out.

**Result** (`target/scratch/storage/ci/model-quick-3.log`): all 15 quick
runs as predicted, in 29 s with 4 workers.

---

## Lost evidence, and how it was regenerated

**Observation.** On 2026-10-05, between 15:48 (the last write to
`target/scratch/storage/ci/`) and 17:03:44, the whole `target/` folder was
removed, with it `target/scratch/`.
- **When it was made again.** The folder's birth time is 17:03:44. That is
  the first build after the gap, whose stylo build script writes
  `target/doc/stylo/css-properties.*`.
- **Not this session's tools.** None of this session's commands removed
  it. The transcripts of the four agents started that day contain no
  `cargo clean`, no `cargo doc` and no `rm -rf`. The removal came from
  outside the session.

**What was lost.** The raw evidence under `target/scratch/storage/`:
- the S1 baseline binary and its harness images;
- the TLC logs of S2 and S4;
- the step-5 binary and its harness run;
- the portal captures of S12 (the tests embed them byte for byte, with the
  sha256 sums the capture had);
- the first mutation runner and its spec files.

Every number this ledger quotes was copied from that evidence before it was
lost.

**What was done.**
1. **Versioned what reproduces each experiment.** The mutation runner is
   now `scripts/mutation-check.py`. Every entry's mutations are in
   `docs/storage/mutations/`:
   - S5, S9, S12 and S13 exactly as they were run;
   - S3 and S4 rebuilt from their tables in this ledger, each text checked
     to occur once in the code.
2. **Ran every mutation again.** The new raw evidence is in
   `target/scratch/storage/mutations/`.
3. **The rest is regenerated where it is used next.**
   - The S1 baseline is rebuilt from `3d5ba26` in a separate worktree before
     the switch-over's harness comparison (S14).
   - The TLC runs are repeated with the model change of S6.

**Regenerated** (`target/scratch/storage/checks/`, `mutations-*.tsv`):
- 50 mutations of S5, S6, S9, S12 and S13, all red, every file restored
  byte for byte.
- The 14 rebuilt mutations of S3 and S4, all red: the rebuilt specs
  reproduce the results this ledger recorded.
- The whole workspace: 326 tests pass on stable and on Rust 1.95.
- clippy is clean with `-D warnings`: the workspace on stable, `gaze-shell`
  with `frame-times`, and the workspace on 1.95.
- The S1 baseline was rebuilt (S1, part 2).

---

## S6 (part 1) — Backups that last the whole session

**Why.** A site's store opens when a page first uses it, at any time in the
session. Its salvage must back up into the same session folder start-up
uses. So `Backups` can no longer borrow the layout and the file system for
the length of start-up.

**Change.** `Backups` now owns an `Arc<Layout>` and an
`Arc<dyn Fs + Send + Sync>`, and its session folders sit in an
`Arc<Mutex<…>>`, so clones share them.

**`StoreSalvage`** implements `gaze_store::Salvage`. Before gaze-store cuts a
damaged store back to its readable records, it does two things:
- it copies the whole damaged file into `<data>/backups/<session>/site-data/`
  (`copy_into`: owner-only, synced, never replacing);
- it reports `Salvaged`. A record cut short by a crash is a Notice; records
  that cannot be read are a Warning.

If the copy fails, the store is not opened, so it is not cut.

**Tests.** All 11 backup tests pass (`s6-backup-owned-tests.log`). Three are
new:
- the copy and the report, for a torn tail and for unreadable records;
- a backup that cannot be made leaves the store alone;
- a real `OriginStore` with three bytes of damage appended. It opens with
  its record, and the backup holds the damaged file byte for byte.

**A test defect avoided.** In the crash test, `fs.clone()` on an
`Arc<MemFs>` would share one file system between all the power cuts,
instead of forking one per cut as `MemFs::clone` does. The test now forks
explicitly (`MemFs::clone(&fs)`).
- That fork cannot be mutation-checked. With a shared file system the cuts
  compound: later cuts find nothing pending and test nothing, so no test
  goes red, and coverage is lost silently.
- The comment at the fork says why it is there.

**Mutation checks** (`docs/storage/mutations/s6-salvage.json`):

| ID | Mutation | Test that must go red |
|---|---|---|
| M6s1 | the store cut back without a backup | `a_real_store_keeps_its_records_and_its_damage_is_backed_up` |
| M6s2 | unreadable records reported as routine | `a_damaged_store_is_copied_and_reported_before_it_is_cut` |
| M6s3 | a failed backup ignored | `a_store_that_cannot_be_backed_up_is_left_alone` |

---

## S5 (part 2) — Two races in the tests, and a lock that frees late

**Observation.** The Rust 1.95 run of the whole workspace failed one test,
`the_runtime_lock_is_released_first`. After `release()`, taking the lock
again found the anchor still held, by this very process
(`target/scratch/storage/checks/r195-test.log`).

**Hypothesis S5-H5.** A `flock` lock belongs to the open file description,
and it lasts while any descriptor of that description is open. Rust's
`File::lock` documentation says the lock is released when the file "along
with any other file descriptors/handles duplicated or inherited from it" is
closed.
- A process being started holds a copy of every descriptor until it runs
  its program; close-on-exec takes effect only then.
- The library's test binary starts processes from several tests at once:
  the fake `dbus-send` scripts, and the lock test's child.
- So a lock released while another test starts a process can still be held
  a moment later.

*Prediction.* Repeated runs of the lock tests beside the tests that start
processes fail now and then; the lock tests alone never do.

*Experiment and result* (`target/scratch/storage/s5-race/summary.txt`):

| Runs | Failures |
|---|---|
| 60, lock tests beside the process-starting tests | 6, in all three tests that release a lock and take it again |
| 60, lock tests alone | 0 |

*Verdict.* Confirmed.

**Fix in the tests.** `test_support::process_starts()` is a guard. It is
held by every test that starts a process, while the process starts, and by
each test that releases a lock and takes it again. Then no process can start
in between.

*Result.* 200 runs: 1 failure, in a different test.

**Hypothesis S5-H6.** `asking_again_is_throttled_and_one_at_a_time` read a
counter of window wakes the moment `wait()` returned. But `finish()` records
the answer and wakes the waiters before it calls the wake callback, so the
counter could still be short by one.
- *Fix:* the test now receives one wake per answer from a channel, with a
  timeout, instead of reading a counter at an instant the code does not
  define.
- *Result:* 300 runs, 0 failures.

**The same effect in the product: a lock that frees late.** The citation
check of the design document found two reasons a lock may outlive its
holder for a moment:
- the same descriptor inheritance as above;
- on Windows, LockFileEx: "If a process terminates with a portion of a file
  locked or closes a file that has outstanding locks, the locks are
  unlocked by the operating system. However, the time it takes for the
  operating system to unlock these locks depends upon available system
  resources."

A quick restart after a crash could then be told that another F1R3Gaze holds
the profile. So `InstanceLock::acquire` tries a held lock again every 50 ms
for up to 1 s (`HELD_RETRY`) before it reports it held. A second F1R3Gaze
started while the first runs waits that second, then says who holds the
profile.

**Tests.** Two are new; all 11 lock tests pass
(`s5-lock-tests-retry.log`):
- `a_lock_freed_while_waiting_is_taken`: the holder lets go after 200 ms,
  and the lock is taken after it.
- `a_lock_still_held_after_the_retry_is_reported`: with 300 ms of patience,
  a lock still held is reported after 300 ms and before 800 ms.

**Mutation checks** (`target/scratch/storage/mutations-s5-lock-2.tsv`). All
nine red, every file restored byte for byte. This includes the new M5i: a
held lock reported at once, with no retry. It turns
`a_lock_freed_while_waiting_is_taken` red.

---

## S1 (part 2) — The baseline, rebuilt

**Why.** The switch-over's harness run (S14) compares its images with the
baseline's. The baseline's images were lost with `target/`.

**Rebuild.** `git archive 3d5ba26` exported the baseline's tree into
`target/scratch/storage/s1/src/`, leaving the repository's git state as it
was. `cargo build --release --locked --offline` built it in 1 min 31 s.

**Hypothesis S1-H6.** The rebuilt binary has a different sha256 from the
recorded one only because it was built in another folder.
- *Observation:* the rebuilt binary hashes to `8997869f…a696`; the recorded
  one was `d4d10cae…0e638a81`.
- *Prediction:* the binary contains an absolute path from its build folder.
- *Experiment:* search the binary for the build folder's path.
- *Result:* one occurrence: the path of stylo's generated
  `target/release/build/stylo-…/out/properties.rs` (a panic location in
  generated code).
- *Verdict:* confirmed. Any build in another folder differs in its bytes,
  whatever its code. The baseline is therefore judged by its images, not by
  its hash.

**The rebuilt baseline's harness run** (`target/scratch/storage/s1/harness.log`):
71 scenes, 0 failed; 72 images; 57 of 57 checks pass. Those are the counts of
the original S1 run and of the step-5 run. Their sha256 sums are in
`harness/SHA256SUMS`, the reference for S14.

**The committed captures are no reference here.**
- *Observation:* 9 of 69 images match `docs/screenshots/ui/after/` byte for
  byte.
- *Explanation:* those captures were last regenerated at `90ad693` (L8).
  The commits since then change what many scenes show: every window
  starting with its sidebar collapsed, the font fix of L10, the lamp of L11
  and its new scenes 69–71.
- *Where the evidence for reproducibility lies:* S4's 72 identical images
  from two different binaries.

---

## S4 (part 3) — The Windows code, compiled for Windows

**Why.** Code under `#[cfg(windows)]` or `#[cfg(not(unix))]` is never
compiled on Linux, so no Linux check can find a defect in it. Examples are
gaze-fs's `commit_name`, `rename_retrying` and folder creation.

**Experiment.** `cargo check --target x86_64-pc-windows-msvc` on this Linux
machine (`target/scratch/storage/windows-check/`):
1. **The Windows-only crates.** `cargo fetch --locked --target
   x86_64-pc-windows-msvc` downloaded them at their locked versions.
   `Cargo.lock` is byte-identical afterwards (`cmp`).
2. **The whole workspace** stops at `ring`'s build script ("GNU compiler is
   not supported for this target"): it needs a Windows C compiler. So the
   crates that use TLS (gaze-net and everything above it, gaze-shell
   included) are checked only by CI's Windows runner.
3. **The crates that do not need `ring`** (gaze-fs, gaze-store, gaze-broker,
   with every feature, tests included) compiled, with one warning:
   `variable does not need to be mutable` in `StdFs::create_dir`. The
   builder was `mut` only for the Unix `mode` call.

**Fix.** `create_dir` builds the folder separately per platform: with mode
0700 on Unix; elsewhere the folder inherits its parent's access list.

**Result.** `cargo check` and `cargo clippy -- -D warnings` for the Windows
target, on those three crates with every feature, are clean.

---

## S4 (part 4) — Creating a file that must not exist, tracing, and crash enumeration for every crate

**Why.** Start-up repair and migration need three things gaze-fs lacked
(design of S6 and S7):
- a way to create a missing file without ever replacing one that appears
  meanwhile;
- a record of every operation, so a test can prove that a second start only
  reads, and so S8 can check real runs against the model;
- crash enumeration that any crate can use.

**Added.**
- **`write_new(fs, path, bytes, perm)`:**
  1. a temporary file, created with the final mode and synced;
  2. read back and compared;
  3. published with `publish_no_replace`.

  If something is at `path`, it fails with `AlreadyExists`. A crash leaves
  either no file or the whole file.
- **`Fs::note`:** a hook with nothing to do on real file systems, for
  marking points in a trace, such as the barrier's start and end.
- **`TraceFs` (feature `trace`, part of `testing`).** It forwards every call
  and records it: the path, the second path, the length, an FNV-1a
  fingerprint and the mode of written bytes, the answer of `same_contents`,
  and the kind of any error. It can also append the records to a file as
  JSON lines.
- **`every_crash` and `every_two_crashes` (feature `testing`).** They moved
  from gaze-fs's own tests. Each state is a `MemFs::clone` fork, and each
  is described by a typed `Crash`: the process crashes, and the power cut
  if any.

**A finding while moving the helper.** One test told power cuts from
process crashes by the wording of the description
(`when.starts_with("power cut")`). The new wording ("a power cut after…")
silently made every power cut count as a process crash, and the test failed
on its count. Descriptions are now a typed `Crash`, and the test reads
`when.power`.

**A wrong expectation.** `write_new_survives_every_crash` first asserted
that nothing is pending after a finished `write_new`. Under NTFS semantics
the temporary file's final unlink stays pending, because there is no
directory sync. That is harmless: a power cut can bring back only the
temporary file, which the next start sweeps. It is also why the model's
NTFS configuration leaves out `ReadyIsDurable`. The test now asserts the
property that matters on both systems: a power cut that keeps nothing
pending keeps the new file whole. It also asserts that nothing is pending
under POSIX.

**Tests.** 32 gaze-fs tests pass (`s4-gaze-fs-part4-tests.log`).

**Mutation checks** (`docs/storage/mutations/s4-gaze-fs.json`, M4i–M4k; all
red, every file restored byte for byte):

| ID | Mutation | Test that went red |
|---|---|---|
| M4i | `write_new` without the file sync | `write_new_survives_every_crash` |
| M4j | `write_new` renaming over the name | `write_new_never_replaces` |
| M4k | `TraceFs` recording a rename without forwarding it | `trace_fs_is_transparent` |

---

## S2 (part 3) — The model, revised for repairs that copy, the busy flag, and the plan's actions

**Why.** Three findings changed the protocol before its Rust code was
written (the S6 and S7 design):
1. **Repairs.** A damaged managed file was moved into the backups and then
   its repair written. A crash in between leaves the file absent, and for a
   file whose usable lines are kept, those lines exist only in the backup;
   the next start then writes the default instead. Now the file's bytes are
   copied into the backups (made durable), then the repair is renamed over
   the file.
2. **When the barrier runs.** It ran at every start, while S15 predicts that
   a steady start does no fsync. S2 had already shown why the barrier can
   be skipped after a start that finished (`ReadyIsDurable`); what was
   missing was a way to know. Now `data/.startup-busy` is created, unsynced,
   before a start writes anything, and removed (and synced) once start-up is
   done. The barrier runs only if the flag is there:
   - a process crash keeps the flag, so the next start runs the barrier;
   - a power cut leaves nothing pending, so the barrier is not needed after
     one, whether or not the flag survived it.
3. **The migration** is now the plan's list of actions: `write` (a converted
   output), `move` (an item), `preserve` (an original moved into the
   backups). Their folders are constants. A ghost variable, `act`, records
   each file operation when `TraceMode` is on (S8).

The previous model and runner are kept in `docs/storage/tla/history/s2-s4/`.

**New invariants.**
- `NeverAbsent`: a managed file that existed is never absent, after any
  crash.
- `RepairInPlace`: a damaged file holds its original or its repair, never
  only a backup.
- `ReadyIsValid` also requires the repair itself (not the default) and no
  busy flag.

**New faults:**
- `move-then-write` (the old repair order);
- `no-backup-fsync`;
- `no-backup-dir-fsync`;
- `no-verify` (a copy published without reading it back);
- `no-busy-flag`.

**New configurations:** `order` (the same actions in another order) and
`badcopy` (copies can come out wrong).

**Predictions.**
1. Every configuration passes.
2. The deep one has $`10^5`$ to $`5 \cdot 10^5`$ distinct states (the design's
   estimate).
3. Each fault breaks the invariant in the runner's table.
4. The old repair order breaks nothing without crashes.

**Result** (`target/scratch/storage/s2-part3/run1.log`): all 28 runs as
predicted on the first run.

| Run | Result |
|---|---|
| deep, two crashes | no error, 238 393 distinct states (3 s) |
| wide, seven actions | no error, 189 042 |
| order | no error, 231 175 |
| badcopy | no error, 23 583 |
| ntfs, with `Termination` | no error, 404 931 |
| live, with `Termination` | no error, 22 041 |
| 9 faults on the deep config | each on its invariant: `regenerate-before-backup` → `CorruptKept`, `replace-destination` → `NoOverwrite`, `marker-first` → `MarkerImpliesComplete`, `no-fsync-before-publish` → `PublishedComplete`, `no-dir-fsync-before-unlink` → `NoLoss`, `resume-stale-temp` → `PublishedComplete`, `no-start-barrier` → `NoLoss`, `no-busy-flag` → `NoLoss`, `no-backup-fsync` → `CorruptKept` |
| `settle-only` and its hazard | `ReadyIsDurable`; and without it `MarkerImpliesComplete` |
| `hazard-no-barrier-one-crash-pending` | `ReadyIsDurable` |
| `move-then-write` | `NeverAbsent`; without it `RepairInPlace`; without both, after a crash, `ReadyIsValid` |
| control: `move-then-write`, no crash | no error: only crash tests can see it |
| `no-backup-dir-fsync`, without `ReadyIsDurable` | `CorruptKept` |
| `ntfs-no-commit-name` | `NoLoss` |
| control: `no-backup-dir-fsync` under NTFS | no error, the same 404 931 states as ntfs: a folder sync does nothing there, and syncing the copy commits its name (W2) |
| `no-verify` on badcopy | `PublishedComplete` |
| controls: no fsync without power loss; no barrier, one crash, data only | no error |

**What this establishes.**
1. A damaged file is never absent and never loses its repairable content,
   whatever crashes when.
2. The barrier is needed exactly after a start that did not finish, and the
   busy flag finds those starts. Without the flag, two crashes lose an item,
   exactly as without the barrier.
3. The order of the plan's actions does not change any verdict.

---

## S6 (part 2) — Start-up's repair, and three gaps it found in durability

**Goal.** Implement start-up's steps 3 to 7 (design A.1–A.8; README section
9) and verify them:
- the busy flag and the barrier (`migrate::begin`, `barrier`, `finish`);
- the marker of a profile with nothing to migrate (`migrate::mark_fresh`);
- the folders and the sweep (`reconcile::prepare`);
- the repair of the 14 managed files and the report of the wallet keys
  (`reconcile::reconcile`).

**Built.**
- **`profile/reconcile.rs`.**
  - The manifest is `Managed` with `path`, `class`, `name`, `perm`,
    `unreadable`, `check` and `missing`, each a `match`.
  - What a file's content says: `Check` (`Valid`, `Newer`, `Damaged`). How
    a damaged file is fixed: `Fix` (`Replace`, `Remove`). What a missing one
    gets: `Missing` (`Leave`, `Migration`, `Create`). Each repair carries
    its report as `Note`s.
  - `Blocked` (all files, or per file) and `Reconciled` (also the user id,
    and the wallets listed again) are what the session receives.
  - The pure checks are separate from the I/O in `prepare` and
    `reconcile`.
- **`profile/migrate.rs`.**
  - The plan's schema (`Plan`, `Action`, `Role`, `ThemeNote`,
    `SettingNote`), which the barrier reads.
  - `barrier_folders`, `barrier`, `begin` with the result `Begun`,
    `mark_fresh` and `finish`.
  - Its tests moved to `migrate/tests.rs`.
- **`gaze_fs::create_dir_durably`.** This is reconcile's `create_missing`,
  moved so the lock can use it too.
- **`report::EventKind::Unrepaired`.** "Cannot be backed up" and "cannot be
  created" were first reported as `Unreadable`, which says something else.
- **`Layout::busy_file`**, which is `data/.startup-busy`.

### Experiment 1: the suite, first run

**Prediction.** Every test passes.

**Result** (`s6-reconcile-test-1.txt`): 85 pass and 2 fail, both in
`check_recovery` (a start after a crash leaves something pending):
- `first_launch_survives_every_crash` fails at "posix: a process crash
  after 4 of 160 operations";
- `a_repaired_file_is_never_absent_at_any_crash` fails at "posix: a process
  crash after 44 of 212 operations".

`MemFs::pending()` was added to name pending operations in failure messages
(`s6-reconcile-test-2.txt`):
- the first failure leaves `+/p (sync /)`;
- the second leaves
  `+/p/config/backups/2026-10-05T18-21-02Z/settings.toml (sync …)`.

**Finding 1: the data root's creation was never synced after a crash.**
- **What happens.** `begin` creates the data root, and only then the flag
  inside it. A start that dies after `create_dir_all(/p/data)` and before
  the syncs of its parents leaves no flag. The next start finds the folder,
  so `create_dir_durably` syncs nothing, and with no flag no barrier runs.
- **Why it matters.** The root's name stays pending however long the
  profile is used. A power cut can then take the data root, with
  everything inside it, even files that were synced.
- **Where it comes from in practice.** The instance lock creates the data
  root first, for its anchor. `take` used `StdFs.create_dir_all`, with no
  syncs at all, so every first start had this window, crash or not. This
  was found while writing `begin`, before the test failed.

**Finding 2: the barrier synced fewer folders than the model's.** The
model's `Barrier` makes every pending operation durable. The code synced
only the skeleton (without `runtime`), `.migration` and the plan's folders.
- A repair that dies after creating its copy, but before syncing the copy's
  folder, leaves the copy's name pending.
- The next start makes a new copy, in a new session `…Z.1`, and syncs only
  that folder.
- The first name stays pending at ready, against `ReadyIsDurable`. The copy
  is a duplicate, so no data depends on it. The design's sentence "Reconcile's
  backups don't need to be covered" held for the data, not for
  `ReadyIsDurable`.

By the same reasoning, three more places were found that a dead start could
leave pending, though no test had failed there yet:
- a linked `settings.toml`'s target folder, where `write_atomic` renames;
- `runtime/`, which the sweep writes into;
- the roots' ancestors.

### Experiment 2: the fixes

**Hypothesis.** All four gaps close if three things change:
- the barrier also runs when the marker is missing: $`F \lor \lnot M`$
  instead of $`F`$;
- the barrier also syncs the roots' ancestors, every folder under
  `backups/`, link targets' folders, and `runtime/`;
- the lock creates its folders with `create_dir_durably`.

Steady starts are unaffected: their marker exists and their flag is gone.

**Prediction.** Both failures pass. Every other test still passes.

**Result** (`s6-reconcile-test-3.txt`): both crash tests pass, but two
tests now fail on the second start's writes. That start runs the barrier,
because the tests' start sequence skipped step 4, which creates the marker
of a profile with nothing to migrate.

That is the new rule working as designed. The tests did not yet start the
way `main.rs` will.

**Fix to the tests.** `migrate::mark_fresh` (design B.2, `Fresh`) was added
and called in the tests' `start_with`, as step 4.

**Result** (`s6-reconcile-test-4.txt`): all 89 profile tests pass.
**Verdict:** the hypothesis holds.

### Experiment 3: the model, given the marker's condition

**Change.** In `StartStep`, the barrier is skipped only when there is no flag
**and** the marker exists. The fault `no-marker-barrier` restores the old
condition.

**Hypotheses** (run 1, `s6-part2-model/run1.log`):
- **H1:** every configuration still passes, because a barrier only makes
  pending operations durable;
- **H2:** `no-busy-flag` no longer breaks `NoLoss`, since every restart
  during a migration (no marker yet) now runs the barrier, but breaks
  `ReadyIsDurable` instead;
- **H3:** `control-no-marker-barrier` passes, since the model's folders
  always exist.

**Results.**
- **H1 confirmed.** Every configuration passes with exactly the state counts
  of S2 part 3. In the model, a start without the flag never has anything
  pending, so the extra barrier never changes a state.
- **H3 confirmed.**
- **H2 refuted:** `no-busy-flag` broke nothing at all.

**Diagnosis.** `MCProfileStartup.tla` placed the managed file `m`
(session.json) in `"new"`, the marker's folder. The sync of the marker's
folder that ends start-up therefore also synced `m`'s, and covered for the
missing flag. In the real layout, `session.json` is in `state/` and the
marker in `data/`.

**H4:** with each file in its class's folder, every configuration passes,
and `no-busy-flag` breaks `ReadyIsDurable`. The folders:

| File | Folder |
|---|---|
| `so`, settings.toml | `config` |
| `u`, user-id, and the marker | `data` |
| `ss`, `sh` and `m` | `state` |
| `k`, the key | `far`, another file system |
| `s`'s original | `config-backups` |
| `w`'s original | `far` |
| `m`'s copy | `state-backups` |

**Result** (run 2, `run2.log`):
- every configuration passes;
- `no-busy-flag` gives "Invariant ReadyIsDurable is violated" (a trace of
  24 states). Run 1 crashes during the repair, with an operation in
  `state` pending. Run 2 starts with a marker and no flag, so it runs no
  barrier. `m` already checks valid, and the last step syncs only `data`.

**Verdict:** H4 confirmed. The runner now expects `ReadyIsDurable` for
`no-busy-flag`. Run 3 (`run3.log`, 2 min 19 s) gives 29 of 29 runs as
predicted:

| Run | Result |
|---|---|
| deep | no error, 260 146 distinct states (238 393 before the folders) |
| wide | no error, 216 196 |
| order | no error, 251 659 |
| badcopy | no error, 24 571 |
| ntfs | no error, 404 931 (unchanged: no folder syncs there) |
| live | no error, 22 999 |
| 14 faults, 4 hazards | each on its invariant, as before but `no-busy-flag` → `ReadyIsDurable` |
| 5 controls | no error, including `control-no-marker-barrier` (260 146 states) |

**What this establishes.**
- The busy flag guards `ReadyIsDurable` after a crash in the repair.
- The marker's condition guards the migration's items, and the data root's
  creation. The second is outside the model's scope, since its folders are
  constants, and is checked by `first_launch_survives_every_crash` and
  mutation M6r18.

### A limit in the crash enumerator

`MemFs::power_cuts` refused more than 16 pending operations, to avoid
enumerating $`2^n`$ subsets. Under NTFS it enumerates per-volume prefixes,
which is linear in $`n`$, and a first launch there has 17 pending
operations before its first file sync (no folder syncs). The guard now
applies to POSIX only.

### The suite

All tests use MemFs and a fixed clock and id.

| Test | What it checks |
|---|---|
| `every_state_of_every_file_in_each_root` | 5 states (missing, valid, damaged, empty, unreadable) of every file, exhaustively within each root: 625 + 15 625 + 125 profiles. Each profile is checked against the rule of the design's table A.2, written out on its own in the test; for no loss; for nothing pending; and for a second start that writes only `create_new(flag)`, `remove_file(flag)`, `sync_dir(data)` |
| `every_pair_of_files_across_roots` | the same, for 61 cross-root pairs × 25 = 1 525 profiles, the marker included |
| `a_second_start_writes_nothing` | each state of each file on its own, a first launch, and a recovery of wallets |
| `first_launch_creates_the_skeleton` | folders 0700, files 0600, exactly the default files and the marker, only Quiet events |
| `every_fix_passes_its_own_check` | over the damaged, empty, non-UTF-8 and `GARBAGE` fixtures, with and without keys to list again |
| `a_repaired_file_is_never_absent_at_any_crash` | every file damaged plus keys to list again: 659 POSIX and 565 NTFS crash states; at each, `NeverAbsent`, `RepairInPlace`, `PublishedComplete` and `CorruptKept`, then a start after it that ends where a start without crashes ends, with nothing lost or pending |
| `first_launch_survives_every_crash` | the same from an empty disk: 511 POSIX and 1 197 NTFS states |
| `two_crashes_keep_every_byte` | a crash, a restart, a second crash and every power cut: 27 753 states |
| `a_finished_start_leaves_nothing_pending` | first launch, steady, everything damaged, everything missing |
| `corrupt_keys_are_reported_and_never_touched`, `wallets_are_listed_again_from_their_keys` (cases a–g), `recovery_never_chooses_a_payer` | section 9.6 |
| `examples_are_refreshed_with_their_old_content_kept`, `a_newer_state_file_is_left_alone`, `a_link_to_nothing_is_left_alone`, `a_file_that_cannot_be_backed_up_is_left_alone`, `the_site_index_keeps_its_names`, `settings_rewrite_keeps_its_mode` | one rule each |
| `a_read_only_run_changes_nothing` | no writing operation in the trace, with the profile half damaged and half missing, and with no folders at all |
| `the_sweep_removes_only_dead_writers_temps` | 12 dead temporary files removed, including one next to a linked settings.toml's target; ours, the keystore's, the cache's, the backups' and the migration's kept |
| migrate: `only_a_start_after_an_unfinished_one_runs_the_barrier`, `the_first_start_makes_the_data_root_durably`, `a_start_that_died_making_the_data_root_is_followed_by_the_barrier`, `the_barrier_covers_the_folders_of_a_plan`, `the_barrier_covers_backups_link_targets_and_the_roots_ancestors`, `plans_read_back_with_their_kinds_named` | section 9.2 |
| gaze-fs: `new_folders_are_durable_and_existing_ones_cost_nothing`, `a_linked_folder_is_used_as_the_folder` | ⟨create folders durably⟩, the second on the real file system, because MemFs follows a link only as a path's last name |
| lock: `the_lock_creates_its_folders_durably` | the anchor's folders are synced in their parents, through `TraceFs` over `StdFs` |

The whole matrix runs in 1.5 s on all cores (`s6-matrix-timing.txt`).

**Workspace** (`s6-workspace-test-2.txt`, `s6-workspace-test-195.txt`):
365 tests pass on stable and on 1.95.

**Clippy** (`s6-clippy-{stable,195,frame-times}.txt`) is clean in all three
configurations. Two lints from earlier work were fixed:
- a doc comment left behind when the crash helper moved to `crash.rs` (it
  is now a plain comment);
- `names.iter().any(|n| *n == file)`, now `names.contains(&file)`, in
  gaze-wallet.

**The cost of a steady start** is one directory fsync: the flag is created
(not synced), then removed, and its removal synced. The design predicted
zero fsyncs (S15) before the busy flag existed. S15 measures this.

### Mutation checks

The checks are in `docs/storage/mutations/s6-reconcile.json`
(`mutations-s6-reconcile.tsv`, 1 min 33 s). All 25 went red, and every file
was restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M6r1 | a damaged file repaired with no copy kept | `every_state_of_every_file_in_each_root` |
| M6r2 | the old order: moved into the backups, then repaired | `a_repaired_file_is_never_absent_at_any_crash` |
| M6r3 | a failed backup ignored | `a_file_that_cannot_be_backed_up_is_left_alone` |
| M6r4 | a file that cannot be read treated as missing | `every_state_of_every_file_in_each_root` |
| M6r5 | a newer format treated as damaged | `a_newer_state_file_is_left_alone` |
| M6r6 | a read-only session repairs anyway | `a_read_only_run_changes_nothing` |
| M6r7 | the keystore swept | `the_sweep_removes_only_dead_writers_temps` |
| M6r8 | no wallets listed again when the list is damaged | `wallets_are_listed_again_from_their_keys` |
| M6r9 | a recovered wallet made the payer | `recovery_never_chooses_a_payer` |
| M6r10 | a damaged key file removed | `corrupt_keys_are_reported_and_never_touched` |
| M6r11 | lost freshness records only a Warning | `every_state_of_every_file_in_each_root` |
| M6r12 | a new user id only a Warning | `every_state_of_every_file_in_each_root` |
| M6r13 | an example counted current whatever it holds | `examples_are_refreshed_with_their_old_content_kept` |
| M6r14 | a valid file written again | `a_second_start_writes_nothing` |
| M6r15 | skeleton folders not synced in their parents | `a_finished_start_leaves_nothing_pending` |
| M6r16 | a link to nothing treated as missing | `a_link_to_nothing_is_left_alone` |
| M6r17 | a damaged site index keeps no names | `the_site_index_keeps_its_names` |
| M6r18 | the barrier only after a flag (finding 1) | `first_launch_survives_every_crash` |
| M6r19 | the barrier without the backups (finding 2) | `a_repaired_file_is_never_absent_at_any_crash` |
| M6r20 | the barrier without the roots' ancestors | `first_launch_survives_every_crash` |
| M6r21 | the barrier without a link target's folder | `the_barrier_covers_backups_link_targets_and_the_roots_ancestors` |
| M6r22 | no busy flag | `a_repaired_file_is_never_absent_at_any_crash` |
| M6r23 | the flag's removal not synced | `a_finished_start_leaves_nothing_pending` |
| M6r24 | the lock's folders not synced (finding 1) | `the_lock_creates_its_folders_durably` |
| M6r25 | `create_dir_durably` without its syncs | `new_folders_are_durable_and_existing_ones_cost_nothing` |

**Documents.**
- README section 9 (Start-up) is new, with two diagrams:
  `startup-activity` (activity) and `managed-file-states` (state).
- Section 7.3 lists the new invariants and the 29 runs.
- Section 10 now says that start-up's repairs copy.
- `tla/README.md` describes the class folders, the marker's condition, the
  new expectation for `no-busy-flag`, and `control-no-marker-barrier`.

---

## S7 — Moving an old profile

**Goal.** Implement start-up's step 4 (design B; README section 10):
detection, a durable plan, its replay (M2 to M9), the conversion of
`settings.conf` and `workspace.json`, and `MIGRATED.txt`. A crash at any
step must lose nothing, and the next start must finish the move.

**Built.**
- **`profile/migrate/convert.rs`.** The old parser and template moved here
  unchanged from `profile.rs`, which re-exports them under their old names
  until the switch-over. It also has `keys_set`, `ThemeFold` and `convert`.
- **`profile/migrate.rs`.**
  - `detect`, `plan`, `run` and the replay;
  - ⟨write a converted file⟩ (`write_converted`), ⟨move a file⟩
    (`move_file`) and ⟨move a cache shard⟩ (`move_shard`);
  - `MIGRATED.txt` and the clean-up.
- **The test helpers.**
  - `TraceFs::with_hook` runs a test's code before each operation is
    forwarded.
  - `MemFs` drops the names a power cut leaves without their folder.
  - `every_crash` and `every_two_crashes` spread their crash points over
    every core.
- **Folder durability everywhere.** The lint `disallowed-methods` now bans
  plain folder creation, and the mutation runner can check a lint (`check`:
  `clippy`).

### Experiment 1: the conversion

**Prediction.** The old templates convert byte for byte, and on 2,000
generated files the converted settings equal the old parser's whenever the
new schema accepts the value.

**Result** (`s7-convert-test-1.txt`). All 13 tests pass on the first run,
and the property's two non-trivial cases each occur more than 1,000 times.

### Experiment 2: the migration, first run

**Result** (`s7-migrate-test-1.txt`): 30 pass and 4 fail.
1. `conflicts_keep_both` shows two defects in `MIGRATED.txt`:
   - when only `session.json` conflicted, the note said "workspace.json:
     not converted", although `history.json` was written;
   - the old `user-id`, kept because of a conflict, was listed under "Left
     here" as "not part of a F1R3Gaze profile".

   **Fixed:** each output is reported on its own, and a conflict's source
   gets "not moved: … already exists; both are kept". The test's expected
   line order was wrong too: write conflicts precede move conflicts, in the
   plan's order.
2. `a_portable_root_is_migrated_in_place`: **a test defect.** Looking for
   `"  config/"` matched `config/settings.toml` in the "Converted" part. The
   portable root's own folders were correctly not listed: there is no "Left
   here" section at all, which is what the test now checks.
3. `a_resumed_migration_keeps_its_backup_folder`: **a test defect.** A crash
   between M8 (the marker) and M9 (the clean-up) is correctly followed by a
   start that finds the profile `Current` and only removes the plan.
4. `a_crash_at_any_step_resumes_and_loses_nothing` failed at "a process
   crash after 4 of 363 operations, then a power cut keeping
   [false, true]": the next start left
   `+/home/u/.local/share/f1r3fly-io (sync /home/u/.local/share)` pending.

**Diagnosis of 4.** `create_dir_all(data)` made two folders, `f1r3fly-io`
and `f1r3fly-io/f1r3gaze`, as two pending operations. The cut kept the
second and not the first. MemFs's names are a flat map, so it showed a
folder whose parent did not exist, and the next start found it, created
nothing, and made `f1r3fly-io` later, as a side effect, without syncing it.
No file system can be in that state: an entry lives inside its folder, so a
folder whose own creation was lost takes its entries with it.

**Hypothesis.** If a power cut also drops every name whose folder did not
survive, the failure goes, and a test of that rule passes.

**Fix.** `MemFs::power_cut` keeps only reachable names (`reachable`: one
pass, since paths order by their components, so each folder comes before
what is in it). New test:
`a_power_cut_never_keeps_a_folders_entries_without_the_folder`.

**Result.** The crash test passes that state. **The fix then found a real
defect elsewhere.** The workspace's tests (`s7-workspace-test-1.txt`) failed
`saving_a_theme_writes_only_when_it_changes`.
`settings::write_setting` created a missing settings folder with plain
`create_dir_all`. After a power cut the folder, and the theme just saved in
it, were gone; the flat map had hidden that. `write_setting` now uses
`create_dir_durably`.

**An audit of every folder creation** found ten more writers that make a
folder without syncing its parent:
- grants, the wallet list, the keystore and the freshness records;
- site stores and the store index;
- replay logs and wallet exports;
- the old workspace and the old settings and user-id helpers.

In the new layout, start-up creates these folders durably beforehand, so
the calls do nothing in practice. Each writer is now correct on its own:
all use `create_dir_durably`. The lint `disallowed-methods` bans
`std::fs::create_dir_all`, `std::fs::create_dir` and `Fs::create_dir_all`;
the trait method is matched through gaze-fs's re-export. Five places are
exempt, each with a reason:
- `create_dir_durably` itself;
- `backup::create_durably`, which syncs every folder up to its root right
  after;
- `scratch_dir`, which makes tests' folders;
- `TraceFs`, which forwards what it records;
- `f1r3c`'s site output, which is made to be published with the umask's
  mode, like its files.

The content cache was already exempt: it is never synced, because it can
be fetched again.

### Experiment 3: the clean-up

**Result** (`s7-migrate-test-3.txt`, after the fixes above). One state
failed: "a process crash after 90 of 363 operations". The next start left
the removal of `.migration/.plan.json.tmp-…` pending.
- The dead start's temporary file was this process's (tests run in one
  process), so the plan's sweep rightly skipped it.
- M9 then removed it, and removed `.migration`, without syncing the first
  removal.
- In real use the same happens when a migration is resumed (no new plan
  written) after a start that died while publishing its plan.

**Fix.** M9 syncs the folder after removing its temporary files and before
removing the folder. **Result** (`s7-migrate-test-5.txt`): all 35 tests
pass.

### Experiment 4: the two-crash test's time

**Observation.** `two_crashes_never_lose_an_item` checks 238 314 states in
90.07 s (`s7-two-crash.txt`). That is too slow for every test run.

**Profile** (`s7-perf/`).
- `perf record --call-graph lbr` cannot open its events: this machine's
  Threadripper PRO 5975WX (Zen 3) has no Intel LBR.
- With DWARF call graphs the flat profile has no hot spot. The largest
  shares are path comparisons (`compare_components` and its iterators,
  about 14 %), `libc` copies (about 9 %), elliptic-curve arithmetic from
  deriving each wallet key's address (`k256`, about 7 %), and TOML parsing
  (about 4 %).
- The work is inherently $`O(n_1 n_2^2)`$, because every second-crash
  prefix replays a whole start, and it runs on one core.

**Hypothesis.** Running the first crash points in parallel divides the time
by about the number of cores, and checks the same states.

**Change.** `every_crash` and `every_two_crashes` give each core every
$`c`$-th crash point. Their checks become `Fn + Sync`; the one test that
counted inside its check now uses atomics.

**Result** (`s7-two-crash-parallel.txt`): the same 238 314 states in
**3.21 s**, 28 times faster (2 142 % CPU). **Confirmed.** A new test,
`every_crash_point_is_checked_once_on_every_core`, guards the split.

### The suite

35 migration tests and 6 conversion tests (README section 10.7). The crash
test checks 929, 2 076, 1 008 and 2 560 states (one file system and two,
each under POSIX and NTFS rules; `s7-crash-counts.txt`), and the two-crash
test 238 314.

**Workspace:** 395 tests pass on stable and on 1.95
(`s7-workspace-test-2.txt`, `s7-workspace-test-195.txt`). **Clippy** is
clean in all three configurations (`s7-clippy-{stable,195,frame-times}.txt`).

### Mutation checks

The checks are in `docs/storage/mutations/s7-migrate.json`.

**Run 1** found four mutations that stayed green. Its table was overwritten
by run 2's output; the verdicts below are as recorded in the session.

| ID | Why it stayed green | What was done |
|---|---|---|
| M7a (a converted file written with `write_atomic`) | `write_converted` looks at the destination first, so a replacing write differs only when something appears in between, and no test made anything appear | `TraceFs::with_hook` lets a test create the destination just before F1R3Gaze publishes it: `a_file_that_appears_meanwhile_is_never_replaced`. `move_file` now looks a second time when its destination appears, and calls it a conflict instead of failing the migration |
| M7b (a file moved with `rename`) | the same | the same test |
| M7i (`quorum` dropped from `keys_set`) | the property decided which settings were set with `keys_set` itself, which is circular | the test decides it on its own (`set_by`, from the old parser's rules) and checks `keys_set` against it |
| M7w (every second crash point skipped) | the test had 41 points and this machine 64 cores, so each core got one point whatever the step | the test now has more than twice as many points as cores |

**Run 2** (`mutations-s7-migrate-run2.tsv`, 1 min 51 s): all **34** red,
every file restored byte for byte. M7ai, added afterwards
(`mutations-s7-m7ai.tsv`), is red too.

| ID | Mutation | Test that went red |
|---|---|---|
| M7a | a converted file written with `write_atomic` | `a_file_that_appears_meanwhile_is_never_replaced` |
| M7b | a file moved with `rename` | `a_file_that_appears_meanwhile_is_never_replaced` |
| M7c | the marker written before the actions | `a_crash_at_any_step_resumes_and_loses_nothing` |
| M7d | the plan published without syncing its data | `a_crash_at_any_step_resumes_and_loses_nothing` |
| M7e | the start barrier skipped | `two_crashes_never_lose_an_item` |
| M7f | a stale temporary file published as the moved file | `a_crash_at_any_step_resumes_and_loses_nothing` |
| M7g | dark kept as dark | `workspace_splits_for_each_theme` |
| M7h | every old key written | `every_old_template_converts_byte_for_byte` |
| M7i | a set key dropped | `conversion_matches_the_old_parser` |
| M7j | the cache copied to another file system | `the_cache_is_left_behind_across_devices` |
| M7k | links moved like files | `symlinks_are_moved_not_followed` |
| M7l | any folder counts as an old profile | `detection_needs_a_signature` |
| M7m | `MIGRATED.txt` ignored by detection | `a_profile_is_migrated_once` |
| M7n | a failed step leaves the session writable | `a_failed_migration_opens_read_only_and_resumes` |
| M7o | moved files keep their mode | `moved_files_are_private` |
| M7p | the settings file's text in `MIGRATED.txt` | `migrated_txt_never_holds_urls` |
| M7q | a resumed migration planned again | `a_resumed_migration_keeps_its_backup_folder` |
| M7r | a power cut keeps the entries of a lost folder | `a_power_cut_never_keeps_a_folders_entries_without_the_folder` |
| M7s | a new settings folder not synced | `saving_a_theme_writes_only_when_it_changes` |
| M7t | the plan folder removed before its removals are synced | `a_crash_at_any_step_resumes_and_loses_nothing` |
| M7u | a conflict listed as not part of a profile | `conflicts_keep_both` |
| M7v | a half-converted workspace reported as not converted | `conflicts_keep_both` |
| M7w | every second crash point skipped | `every_crash_point_is_checked_once_on_every_core` |
| M7x–M7ah | each of the eleven writers' folders made without the durable call (grants, wallets, keystore, freshness, site stores, store index, replay logs, exports, the old workspace, settings and user id) | the lint `disallowed_methods` (`check`: `clippy`) |
| M7ai | sweeping one name without syncing its folder | `sweeping_one_name_leaves_every_other_file` |

**Every earlier entry, checked again** (`mutations/recheck-s7-*.tsv`),
because MemFs, the crash enumerator, `trace.rs`, the lock and the settings
changed.
- S3, S5, S6, S9, S12 and S13 are all red.
- S4 had three texts that no longer matched:
  - M4d's now occurred twice, because `sweep_temps_of` ends like
    `sweep_temps`;
  - M4f's had been stale since S4 part 4, which put `write_new` after
    `write_atomic`;
  - M4k's needed `TraceFs`'s new hook line.

  Refreshed, all 11 are red.

**Documents.**
- README section 10, "Moving an old profile", is new, with the diagram
  `migration-activity`.
- Section 7.1 now states the folder rule, and section 7.2 the wider lint.
- The reference \[Steele-2014\] was added after checking its DOI with
  Crossref.

---

## S8 (part 1) — Real start-ups checked against the model

**Goal.** Show that the code does what `ProfileStartup.tla` does: record
real start-ups and check that each is a behaviour of the model (trace
validation, \[Cirstea-2024\]; README section 15.2). Part 2, the binary
killed at real storage operations (`crash-points`), needs the binary to run
the new start-up, and lands with the switch-over.

**Built.**
- **The trace module.** `docs/storage/tla/ProfileStartupTrace.tla`:
  `TraceInit`, and `TraceNext`, which matches the next event with a model
  step that does the same. It allows *eager* folder syncs anywhere. A
  barrier's synced folders must cover every pending operation, and crashes
  are the test's own. `TraceEnd` is violated exactly when every event was
  matched.
- **The converter.** `crates/gaze-shell/src/profile/migrate/trace.rs`
  (feature `storage-trace`) maps names and operations into the model's
  terms. A power cut's kept operations become indices into the model's
  queue, which holds the modelled operations only.
- **The harness.** `crates/gaze-shell/tests/storage_trace.rs` runs the
  cases, writes each trace's module, configuration and events, and asks
  TLC. A rejected trace is reported with how far the model could follow
  it, found by a binary search with probe invariants.
- **The script and CI.** `scripts/storage-trace.sh [--quick]` checks the
  jar's sha256. CI's `model` job runs it after `storage-model.sh`, which
  now also runs SANY on the trace module.
- **gaze-fs.**
  - `process_id`, with `with_process_id` for tests: a simulated restart is
    another process.
  - `MemFs::pending_changes`.
  - `TraceFs::with_hook`, from S7.
- **gaze-shell features.** `storage-trace` and `crash-points`.

### Comparing the code with the model, before any run

Reading the code's operation sequences against the model's steps showed
that they match, step for step:
- a move: link, folder sync, unlink, folder sync;
- a copy to another volume: create, write, sync, compare, link, sync,
  unlink, sync, then the old name's unlink and sync;
- a converted file (`write_new`), and the marker.

There were two divergences:
1. **The marker.** The model's `MarkerStep` removes a dead start's
   temporary marker before making its own; the code did not, and left it
   to step 5's sweep. Run on a trace with a crash after the temporary
   marker was made, the model must reject that order. **Fixed:** M8 sweeps
   `.layout.json.tmp-*` of other processes first. The sweep before every
   write and move already did this.
2. **The process id.** Tests run every simulated restart in one process,
   so the code never swept the dead start's temporary files, which belong
   to "this" process, while the model, rightly for a real restart, does.
   **Fixed for the tests:** `gaze_fs::with_process_id` gives each
   simulated process its own id (feature `testing` only). Release builds
   use the real id.

### Experiment 1: the quick set

**Prediction.** Every trace is accepted, and both mutants (a move without
its folder sync; a move with its link and unlink swapped) are rejected.

**Results.**
- **Run 1** (`s8/run1.txt`): the mutants were rejected and 44 of 46 traces
  accepted.
  - `killed_two_25`: TLC failed while parsing, with a
    `NullPointerException`. **Cause:** TLC unpacks its standard modules
    (`TLC.tla`, `Naturals.tla`) into `java.io.tmpdir`, which defaulted to
    `/tmp`, and 16 TLCs at once raced on the same files. **Fixed:** each
    case's TLC gets its own `java.io.tmpdir`, in its folder (and so never
    `/tmp`).
  - `killed_two_250` (then reported unnamed): "the copy … was never
    compared". **Cause:** the converter looked ahead for the comparison
    that verifies a copy and found the crashed process's comparison, which
    had failed after the crash point and so carried no answer. **Fixed:**
    failed records are skipped, and the look-ahead stops at a crash: a copy
    left uncompared holds what was copied.
- **Run 4:** all 46 accepted.

### Experiment 2: the full set, and a gap in the model

**Result** (`s8/run5-full.txt`): 167 of 168 accepted. `killed_one_260` was
rejected. The new diagnosis (`s8/run6-full.txt`): "62 of 74 events matched;
the next: `unlink(MarkerTmp)`".

**Analysis.**
- Run 1 died after linking the marker and syncing the data folder, but
  before unlinking the marker's temporary file.
- Run 2 found the marker, so the profile was current. Its step 5 sweep
  removed the dead process's temporary marker: correct, and needed, or such
  files would build up.
- The model has no step 5. Once the marker exists, `FirstPhase` goes to the
  repair, and `MarkerStep` (the only step that removes `MarkerTmp`) never
  runs. The model could not follow.

**Hypothesis.** Adding a `prepare` phase that removes a leftover
`MarkerTmp` and syncs the data folder makes the trace a behaviour of the
model, and changes no verdict of the 29 runs. The phase sits after the
marker, or first when the marker exists, before the repair.

It is kept minimal on purpose. `MarkerTmp` is the only modelled temporary
file that can outlive the migration, because every action's temporary file
is swept when the action is replayed. If the code's sweep ever removed
another, the model would reject that trace, and that would be worth knowing.

**Results.**
- The model run (`s8/model-run1.log`): **29 of 29 as predicted.** The deep
  configuration has 262 067 distinct states (260 146 before): the new
  states are those where a leftover `MarkerTmp` is swept.
- The traces: **all 168 accepted** (`s8/run7-full.txt`).

**Verdict:** confirmed.

### Named crash points

A mutation of the marker's sweep, the simulated process id, or the copy's
check goes red only if some trace crashes in the window it guards, which
the spacing of every 5th (or 25th) operation leaves to chance. Three cases
now crash exactly there:
- just after the temporary marker is made;
- just after the marker is linked;
- just after a copy to another volume, before its comparison.

**The script's full run** (`s8/script-full.log`): **171 traces accepted**
and both mutants rejected, in 35 s. Its quick run checks 49 traces in 14 s.

### Mutation checks

The checks are in `docs/storage/mutations/s8-trace.json`, run with
`F1R3GAZE_TRACE_QUICK=1` (`mutations-s8-trace.tsv`, 4 min). All 6 went red,
and every file was restored byte for byte.

| ID | Mutation | Test that went red |
|---|---|---|
| M8a | a published name's folder not synced before the old name goes | `real_migrations_are_behaviours_of_the_model` |
| M8b | the busy flag never set | the same |
| M8c | the marker written before the plan's actions | the same |
| M8d | a dead start's temporary marker not swept before the marker | the same |
| M8e | a simulated restart keeps the dead process's id | the same |
| M8f | a copy never compared taken for garbage | the same |

**The design's M8b** was a converted file published with `write_atomic`.
The prediction was that it would stay green, and it did
(`mutations-s8-m8x.tsv`). The model allows publishing into a free name by a
rename (its path for file systems without hard links), and that is safe
there. A rename replaces a file only when one appears between the look and
the publish, a race that only a test making it happen can show. M7a's race
test, `a_file_that_appears_meanwhile_is_never_replaced`, guards it. So it
is not a trace mutation.

### Checks

- **Workspace:** 396 tests pass on stable and on 1.95
  (`s8/workspace-test*.txt`).
- **Clippy** is clean in four configurations: the three usual ones
  (`s8/clippy-{stable,195,frame-times}.txt`) and `--no-default-features
  --features storage-trace`, which compiles the trace code
  (`s8/clippy-trace.txt`).
- That fourth configuration lints the library's tests without the window
  for the first time. It found `test_support::ScratchProfile` unused there:
  only the chrome's tests, which need the window, use it, so it is now
  gated the same way.

**Documents.**
- README section 15, "Verifying storage", is new, with the diagram
  `trace-validation`. Section 7.3 points to it.
- `tla/README.md` describes the trace module, the cases, the `prepare`
  phase and the runs.

---

## S8 (part 2) — Real kills checked against the model

**Goal.** The last layer of section 15.1: the real binary, killed at a real
storage operation during start-up, on the real file system and kernel. What
it did, both processes' operations with the crash between them, must be a
behaviour of `ProfileStartup.tla`, and nothing may be lost. Part 1 checked
start-ups run on `MemFs`; this part needed the binary to run the new
start-up, so it came with the switch-over (S14).

**Built.**
- **`crates/gaze-shell/tests/crash_points.rs`** (features `crash-points` and
  `storage-trace`, no window). For each shape, the profile this machine's
  F1R3Gaze made and a full one (keys, an export, a site store, a replay log,
  a cache shard), laid flat at a portable root:
  1. a reference run of `f1r3gaze wallet list`, the shortest command that
     runs the whole start-up, counts the storage operations before the note
     `profile opened`, and finds four windows: the temporary marker made,
     the marker linked, the plan's temporary file made, the first item
     linked;
  2. for every 5th operation (25th with `F1R3GAZE_TRACE_QUICK`) and each
     window, a run with `F1R3GAZE_CRASH_AT=k` must abort (SIGABRT on Unix),
     having recorded exactly k − 1 operations; the test appends the crash;
  3. a clean run finishes, appending to the same trace;
  4. every old file's bytes are found in the profile, the user id at its
     place; the marker and `MIGRATED.txt` exist, and the plan is gone; for
     three kills, a third run changes nothing but the lock files;
  5. TLC must accept every trace (`common::check_all`).
  Each run has 60 s; `HOME`, every XDG variable and `F1R3GAZE_PROFILE` are
  pointed away from the user's folders.
- **`scripts/storage-kill.sh [--quick]`**, modelled on `storage-trace.sh`:
  the pinned jar (sha256 checked with `sha256sum` or `shasum`), `TMPDIR` and
  every folder under its output, no core files.
- **`tests/common/mod.rs`**: the TLC helpers (`accepted`, `diagnose`,
  `check_all`, and the probe search) moved out of `storage_trace.rs`, which
  both tests now share.
- **In the binary.** `main.rs` runs start-up on `TraceFs::appending_to` when
  `F1R3GAZE_STORAGE_TRACE` names a file (`storage-trace` builds only), and
  notes `profile opened` when start-up is over. The migration notes the plan
  it replays (`plan <json>`), which `migrate::trace::plan_of` reads back with
  the records (`events_of_ndjson`, the inverse of `Traced::json`).
- **The alignment.** The instance lock made its folders and wrote its
  `instance.pid` through `StdFs` whatever file system start-up ran on, so a
  traced start missed them while each was still a crash point.
  `InstanceLock::acquire_in(fs, …)` now uses start-up's (S14, B2): every
  storage operation before `profile opened` is a traced one. The engine's
  own writers (stores, wallets, grants) use `StdFs`, untraced, so kills are
  chosen before that note.

**Hypothesis H-kill.** Every kill's trace is accepted, the alignment holds
for every kill, and nothing is lost.

**Results.**
- **Quick** (`s14/kill-quick-1/`): 40 kills, all accepted, every check
  passed, in 11.1 s.
- **Full** (`s14/kill-full-1/`): 165 kills, all accepted, in 46.8 s.
  Start-up made 341 storage operations on the user's shape and 436 on the
  full one.
- **After the S14 fix of the emptied folders** (S14-D1,
  `s14/kill-full-d1/`): 163 kills, all accepted, in 58.4 s; the full shape's
  start-up made 434 operations, its six folder removals among them.
- **Final** (`s14/kill-full-final/`, after the fix's redundant sync went):
  164 kills, all accepted, in 34.4 s; 341 and 433 operations.

**Verdict.** Confirmed: the protocol the model checks is what the binary
does on a real file system, under real crashes.

**CI.** A new `kill` job runs `storage-kill.sh --quick` on Linux, macOS and
Windows (Windows Error Reporting's dialog turned off first). These are the
first Windows traces through the converter and TLC; they use the model's
NTFS semantics (`ntfs: cfg!(windows)`).

### Mutation checks

`docs/storage/mutations/s8-kills.json`, run with `F1R3GAZE_TRACE_QUICK=1`
and the pinned jar:

All four went red, every file restored byte for byte
(`s14/mutations/s8-kills.tsv`, 55 s):

| ID | Mutation | Test that went red |
|---|---|---|
| M8g | the binary starts without its busy flag (`begin` skipped) | `real_kills_are_behaviours_of_the_model` |
| M8h | the migration does not note the plan it replays | the same |
| M8i | a crash point never aborts | `every_crash_point_is_a_traced_operation` |
| M8j | the lock's operations bypass start-up's file system | the same |

---

## S14 (part 1) — The switch-over to the five roots

**Goal.** The browser opens its profile as start-up was designed (README
section 9), in the five roots, and the single-folder profile leaves every
code path: the lock taken as each command needs it, start-up in one
function, the session and the history in their own files, the theme a
setting that follows the system, start-up's notices shown, and the commands
`paths`, `profile check`, `profile backups [prune]` and `trust list|forget`.
Then the harness, the scripts and CI on the new layout, with an isolation
that cannot reach the user's own folders.

**Design.** A Plan agent's report (2026-10-05; kept as
`s14/design-report.md`, sha256 `fa61ecdf…`), checked against the code
before any of it was built. Its five findings corrected the approved plan:
1. **The predicted screenshot changes were wrong.** Scenes 09–10 do not
   change (no wallet, so no Embers notice). Scenes 69–71 do (their Appearance
   panel shows the palette's path). The corrected prediction: 15–18, 52–59
   and 69–71.
2. **Isolating the XDG variables does not protect the old profile.** It is
   also found through `HOME` (`layout.rs`), and a start without `--profile`
   moves it. Every run without `--profile` must isolate `HOME`, the S16 dry
   run included, and the `paths` check must cover the `legacy` lines.
3. **Isolating `$XDG_DATA_DIRS` hides the GPU driver**, which the Vulkan
   loader finds there: name it in `VK_DRIVER_FILES`. Fonts might change too,
   so a control run with the old binary (H0) comes before any churn is
   blamed on code.
4. **A chrome defect, L14**: Dark and Light were drawn in a valid custom
   palette's colours (chrome ledger L14).
5. **The S1 baseline binary** is at
   `s1/src/f1r3-work/f1r3gaze/target/release/f1r3gaze` (sha256
   `8997869f…a696`); `s1/f1r3gaze-baseline` was never kept.

**The design's open questions**, answered with the defaults below (the
user was told, and can change any of them):

| | Question | Default taken | Why |
|---|---|---|---|
| Q1 | A way back to System before step 10's control? | No fourth segment now | Step 10 brings the System/Dark/Light control; until then `theme = "system"` in `settings.toml`, which its template documents. The Appearance markup, and so the harness's coordinates, stay as they are |
| Q2 | Start-up's notices: status-bar messages, or a lasting area? | Status-bar messages, most severe first, 8 s each, and stderr | The chrome's existing messages, no new markup |
| Q3 | Freshness in a session that only reads | Enforced from the file, never written (`FileFreshness::read_only`) | An empty in-memory log would reopen, for that session, the stale-binding window the records close |
| Q4 | Does a read-only session fill the content cache? | No: read and verified, never stored, removed or evicted (`ContentCache::read_only`) | Read-only means nothing written (README §9.7) |
| Q5 | Record L14, red first? | Yes | Every defect gets an entry |
| Q6 | Ledger names | S14 (part 1) and S8 (part 2) | As the earlier parts |
| Q7 | Do `trust list` and `profile backups` run the full start-up? | Yes, like every command but `paths` and `profile check` | One start-up for every command that opens the profile |
| Q8 | `profile check`'s exit status | 1 when a start would change something | As `fsck -n` and `--check` options do: scripts can act on it |

**Built.**
- **gaze-shard.** `FileFreshness::read_only`. The bridge reports a failure to
  save the records once, and again only after a save has succeeded in
  between (`SaveFailure`): a session that only reads failed every save the
  same way, and would have shown the same alert after every new block.
- **The lock** (`InstanceLock::acquire_in`): its folders, its `instance.pid`
  and the reading of another holder's go through start-up's file system, so
  every crash point before the engine is a traced operation (S8, part 2).
- **The migration** notes the plan it replays (`plan <json>`); `trace.rs`
  reads JSON-lines traces back (`events_of_ndjson`, `plan_of`);
  `migrate::newer_layout` reads the marker alone.
- **`profile.rs`.**
  - `start_up`: steps 3 to 7, the one copy of their order, which also
    decides when a writing session becomes read-only on the way (README
    section 9.1);
  - `Profile::open(layout, Locking, StartEnv)`: the lock (`Exclusive`,
    `IfFree`, `ReadOnly`, and `Unlocked` for tests), `start_up`, and the
    settings, each diagnostic reported with its place;
  - `Profile`: the theme (the session's first, then `settings.toml`),
    `read_state`/`write_state` for `session.json`, `history.json` and
    `window.json` (refused when blocked), `store_salvage`,
    `forget_history_backups`;
  - `check` for `profile check`, and `Checked::changes`.
  The old `default_dir`, `Settings::load` and `user_id` are commented out
  with their reasons.
- **The engine** (`Engine::open(profile)`; `Engine::new` commented out):
  every file at its new place; the wallets, the grants and the freshness
  records read-only when blocked; the freshness records in memory for a
  shard whose observers are all on this machine (`observers_are_loopback`,
  `freshness_log`); a damaged site store backed up before it is cut back;
  no site store in a session that only reads; the content cache read-only
  then.
- **`main.rs`**: the command table of README section 8, the usage, and the
  exit statuses; with `storage-trace`, start-up recorded into
  `F1R3GAZE_STORAGE_TRACE`.
- **The chrome**: the session and the history in their own files, the
  history written at most every 2 s and on closing (`save_state`,
  `history_changed`, `Drop`); the theme resolved by `theme::resolve` with the
  system's preference (`system_theme.rs`: the portal on Linux, winit on macOS
  and Windows), and repainted only when the colours change; the Appearance
  panel's controls rewired with their markup unchanged (Custom is
  `themes/custom.css`; Create never replaces it); start-up's notices; replay
  logs and exports in the data folder, refused when read-only.
- **The tests' profile** (`ScratchProfile`): a portable root whose
  `config/settings.toml` empties the observers. Never a `settings.conf`:
  start-up would take it for an old profile and move it.
- **`profile check` reports everything a start would do.** A session that
  only reads skipped the temporary-file sweep and said nothing of a file
  where a folder must be. It now lists both (`gaze_fs::stale_temps`,
  `stale_temps_of`).

### S14-D1: the folders a move emptied were left behind

**Found by** the CI's migration check, run in its new `--same-device` mode
on this machine: the old folder still held `keys/`, `exports/`, `store/`,
`logs/` and `cache/`, empty, though `MIGRATED.txt` says an older F1R3Gaze
"finds this folder empty".
- **H:** the migration moves a folder's items one by one and never removes
  the folder. **Prediction:** a test that moves every kind of item finds
  five empty folders. **Raw result** (`s14/d1-red.log`): exactly those five.
  **Verdict:** confirmed.
- **Fix (M6b):** after the moves, each old folder the plan moved items out
  of is removed if it is empty, deepest first. Only an empty folder can go,
  so nothing that was not moved goes with it; one that still holds
  something stays, and the note lists what is in it. README section 10.3
  has the step and why it loses nothing.
- **A sync too many.** The fix first synced the old folder after the
  removals. Its mutation check (M14ad) stayed green: M7 writes the note
  into that folder atomically right after, which syncs it, and a start that
  stops first is followed by a barrier over the plan's folders, this one
  among them (every plan moves a file out of it). The sync was removed, and
  M14ad is kept in the spec, retired, with that reason.
- **Green:** the 39 migration tests, with every crash point and two crashes
  in a row; 171 of 171 traces; 164 of 164 real kills; the CI check
  (`ok: the old profile moved (linux, on one file system)`).

### Experiments

**H-trace: with the real `start_up`, every trace is still accepted.**
`storage_trace.rs` ran a copy of start-up's order; it now calls
`profile::start_up`. The newer-marker check adds a read, so the crash
points move. **Prediction:** 171 accepted and both mutants rejected, as in
S8; and every start after a crash leaves the profile moved and writable (a
stronger check than before, which asked only that the start finished).
**Result** (`s14/trace-full-b13/`): as predicted. **Verdict:** confirmed.

**H0: the new isolation changes no capture** (a control: the S1 baseline,
seeded the old way, `--seed-format legacy`).
- **Prediction:** every capture byte-identical to S1's (`s1/harness/`), and
  57 of 57 checks.
- **Run 1** (`s14/h0/`): 67 of 71 identical. 50 of 57 checks: the seven
  cursor scenes' `cursor_refs_distinct` failed.
  - **The four captures** (03, 04, 49, 62) differ in one 6×8 px spot, the
    date of the visit seeded 45 days ago: "2026-08-21" in S1, "2026-08-22"
    now. Dates are shown in UTC, and S1 ran before UTC midnight. The
    harness seeds visits relative to now: a calendar effect, not the
    isolation's.
  - **The cursor checks.** H: libXcursor's search path (the `xcursor`
    crate, under winit's X11 backend) runs through `$XDG_DATA_HOME` and
    `$XDG_DATA_DIRS/icons`, which now point into the work directory, so
    every cursor falls back to one shape. Fix: the isolation names the
    path in `XCURSOR_PATH` first, in the crate's order, as S1's
    environment had it.
- **Run 2** (`s14/h0-run2/`): **57 of 57 checks**; 67 of 71 identical to
  S1, the other four differing only at that date. **Verdict:** confirmed:
  the isolation (every XDG variable, no session bus, the fake portal,
  `VK_DRIVER_FILES`, `XCURSOR_PATH`) changes nothing, the user's
  fontconfig settings included.

**H1: the new binary changes exactly the predicted scenes** (sha256
`2f1e9d6a…fb54`, the default work directory, compared with H0's run 2 of
the same UTC day).
- **Prediction:** scenes 15–18, 52–59 and 69–71 change (their Appearance
  panel shows the custom theme's path, and 17–18 the new parser's message);
  the other 56 are byte-identical; 57 of 57 checks.
- **Result** (`s14/harness/`, `s14/h1-changed-regions.txt`): **exactly those
  15**; 56 identical; 57 of 57 checks. Each change is inside the card: a
  97 × 13 px run of the path's tail (`…/themes/custom.css` for
  `…alette/palette.css`), 44 px lower in 56–57 under the prompt bar; in
  17–18 the path, the message ("line 2: expected a --gaze-* colour; a
  theme file holds only these, optionally inside :root { }") and the
  Reload button it moves down.
- **Verdict:** confirmed. The committed captures in
  `docs/screenshots/ui/after/` are not regenerated (that needs the user's
  go-ahead).
- **The final binary** (sha256 `0dc5c59c…`), after the mutation campaign
  removed a redundant sync from the migration, which no harness profile
  runs: 71 of 71 captures byte-identical to H1's, and 57 of 57 checks
  (`s14/harness-final/`).

**The isolation itself.** `--check-isolation` passes in the default work
directory (every folder inside it, `dbus-send` the fake,
`VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json`).
`--work ~/.local` is refused with exit 3, before anything is written there
(`~/.local` holds a real F1R3Gaze folder). Writing the guard found it ran
after the work directory's lock file was made; it now runs first, in all
three scripts.

**Scripts.** `scripts/lib/storage-isolation.bash` (the real roots, the
guard, the isolation, the self-test), shared by `ui-snapshots.sh`,
`resize-bench.sh` and `resize-live.sh` (each with `--seed-format
current|legacy`); `storage-smoke.sh` (passes here: 9 files, a second start
wrote nothing); `storage-migration-ci.sh` (CI only, with `--same-device` to
run anywhere); `storage-kill.sh` (S8, part 2). Shellcheck finds nothing new
in the changed scripts and nothing in the new ones.

### Checks

- **Tests:** 465 pass on stable and on 1.95 (396 at the start of S14;
  `s14/final-tests-{stable,195}.log`).
- **Clippy** is clean in five configurations: the workspace on stable and
  1.95, `frame-times`, `--no-default-features --features storage-trace`,
  and `--no-default-features --features crash-points,storage-trace`
  (`s14/lint-*.log`). The last first linted the kill test, and found a
  complex tuple type, now a struct.
- **The census** of the old names (`settings.conf`, `workspace.json`,
  `palette.css`, `eng.dir`, `default_dir`, `Engine::new`, `UiState::load`):
  `s14/census-before.txt` (200 lines) and `census-after.txt` (210). In the
  chrome, the engine, `main.rs` and the pages they appear only in comments;
  everywhere else they belong to the migration, its conversion, the
  backups, the tests, the scripts' legacy seed format, and the ledgers.
- **The resize benchmark**, once (`s14/resize-bench-smoke/`): its new
  isolation and seeding work (a `frame-times` build, six seeded tabs, 49
  frames that applied a size and 1 extra).
- **`resize-live.sh`** opens a window on the user's own desktop and waits
  for a hand to drag it: it was not run. Its isolation is the shared
  library's, and its seeding the benchmark's.

### Mutation checks

**The campaign** (`s14/mutations/summary.tsv`, `s14/run-mutations.sh`):
every spec of the ledger, run again on the final code, since the
switch-over moved much of the code they name. Each red, every file
restored byte for byte:

| Spec | Red | Retired | Time |
|---|---|---|---|
| `s14-switch-over.json` | 40 | 1 (M14ad) | 131 s |
| `s8-kills.json` | 4 | | 55 s |
| `s3-layout.json` | 6 | | 19 s |
| `s4-gaze-fs.json` | 11 | | 5 s |
| `s5-lock.json` | 9 | | 33 s |
| `s6-salvage.json` | 3 | | 11 s |
| `s6-reconcile.json` | 25 | | 79 s |
| `s7-migrate.json` | 33 | 2 (M7ag, M7ah) | 107 s |
| `s8-trace.json` | 6 | | 148 s |
| `s9-settings.json` | 10 | | 31 s |
| `s12-themes.json`, `s12-system-theme.json` | 17 | | 86 s |
| `s13-window-state.json` | 12 | | 43 s |

**What the campaign found.**
1. **M14p stayed green: the history test was weak.**
   `history_is_written_once_per_delay` saved first at a time taken before
   the two visits, which a history due at once passed too. It now saves at
   a time taken just after them, which only a history waiting for its delay
   passes. Red since.
2. **M14ad stayed green: a sync too many.** The removal of the emptied
   folders was synced, and M7 syncs the same folder right after (S14-D1
   above). The sync is gone; M14ad is retired with that reason.
3. **M7ag stayed green: it mutated a comment.** Its writer, the old
   `Settings::load`, was commented out at the switch-over, and the
   mutation's text was found only in that comment. It is retired, as is
   M7ah (the old `user_id`, also disabled). `scripts/mutation-check.py`
   now refuses a mutation whose text lies only in comment lines (`IN A
   COMMENT`): commented-out code is kept, not deleted, so this would recur.
   A static check of every spec found no other.
4. **Texts the switch-over moved.** M7ac, M7ad and M7ae (the store index,
   replay logs and exports now write through the profile's file system) and
   M4d (the read-only listings now follow the sweep) were updated to the new
   code. M5g and M5i were updated after the lock's change (B2): 9 of 9 red
   then too (`s14/mutations-s5.log`).
5. **The runner** reports a retired entry (`"retired": "<reason>"`) without
   running it, and prints each verdict as it finishes.

**S14's own mutations** (`docs/storage/mutations/s14-switch-over.json`):

| ID | Mutation | Test that went red |
|---|---|---|
| M14a | a writer opens read-only while another F1R3Gaze holds the profile | `the_lock_policy_decides_access` |
| M14b | a reader is refused while another F1R3Gaze holds the profile | the same |
| M14c | `paths` opens the profile first | `cli::paths_names_the_roots_and_creates_nothing` |
| M14d | `profile check` runs a writing start | `profile_check_reports_what_start_up_would_do` |
| M14e | the busy flag removed after a move that stopped | `a_stopped_migration_leaves_the_session_read_only_and_unfinished` |
| M14f | no busy flag at start | `a_steady_open_writes_only_its_busy_flag` |
| M14g | a newer profile not seen before the busy flag | `a_newer_profile_is_opened_read_only_without_a_busy_flag` |
| M14h | blocked wallets opened writable | `blocked_wallets_refuse_changes` |
| M14i | blocked grants opened writable | `blocked_grants_are_never_saved` |
| M14j | a site index start-up could not read saved anyway | `an_unreadable_site_index_is_never_replaced` |
| M14k | no observer counts as this machine's | `observers_are_loopback_only_on_this_machine` |
| M14l, M14l2 | a read-only freshness log saves; the same save failure reported after every block | `a_read_only_log_enforces_and_never_writes`; `a_repeated_save_failure_is_reported_once` |
| M14m | site stores opened in a session that only reads | `site_data_is_not_opened_read_only` |
| M14n | a damaged store cut back without a backup | `a_damaged_store_is_backed_up_when_it_opens` |
| M14o | the engine makes a user id of its own | `the_engine_uses_the_reconciled_user_id` |
| M14p | the history written at every visit | `history_is_written_once_per_delay` (after the test was strengthened) |
| M14q | a tab switch rewrites the history | `a_tab_switch_never_rewrites_history` |
| M14r | a closing window saves nothing | `closing_the_window_saves_pending_history` |
| M14s | Clear history keeps the history's backups | `clearing_history_removes_its_backups` |
| M14t | a theme that cannot be saved is not used either | `set_theme_changes_the_session_first` |
| M14u | Create palette file replaces a theme file | `create_palette_never_replaces_a_theme_file` |
| M14v | System resolved without the system's preference | `system_follows_the_os` |
| M14w | a built-in choice follows the system | `an_explicit_choice_ignores_the_os` |
| M14x | the window's reports used under `F1R3GAZE_SYSTEM_THEME` | `the_override_ignores_window_reports` |
| M14y | start-up's events never shown | `start_up_notices_are_shown_once` |
| M14z | the custom palette laid over a built-in scheme again (L14) | `a_builtin_scheme_ignores_the_custom_theme` |
| M14aa–aa3 | the lock's folders, its `instance.pid`, or another holder's read through `StdFs` | `the_lock_goes_through_the_file_system_it_is_given` |
| M14ab | a test profile that is an old single-folder one | `a_scratch_profile_is_never_an_old_one` |
| M14ac | the folders a move emptied left behind (S14-D1) | `emptied_folders_of_the_old_profile_go_too` |
| M14ad | (retired) the emptied folders' removal never synced | — |
| M14ae, M14af | a session that only reads names no temporary file to sweep; nothing in the way of a folder | `the_sweep_removes_only_dead_writers_temps`; `a_read_only_run_names_a_file_in_the_way_of_a_folder` |
| M14ag, M14ah | a read-only content cache stores a blob; a read-only session's cache opened writable | `a_read_only_cache_reads_and_never_writes`; `a_read_only_engine_never_fills_the_content_cache` |
| M14ai, M14aj | `profile check` counts nothing made as a change; exits 0 when a start would change something | `profile_check_reports_what_start_up_would_do`; `cli::profile_check_writes_nothing` |
| M14ak, M14al | a blocked freshness file written anyway; a local shard's records written to the file | `freshness_records_are_kept_where_the_shard_says` |

The harness's isolation was checked by hand rather than by the runner: the
guard refuses `--work ~/.local` (exit 3, nothing written there), and the
self-test is what made H0's isolation visible (the cursor theme).

---

## S10 — Freshness records

**Goal.** The architecture's freshness requirement (proposal §8.2,
"Freshness"; specification §9.4, resolution step 4), kept across restarts:
for each binding, the highest finalized block at which the browser has seen
it. An answer read at an older block replays superseded state, and is
refused. Kept only in memory (`Bridge.seen`, before), every restart
reopened that window. The pieces were built in S4 (the format, in memory)
and S14 (on disk); this entry gathers them.

**Built.**
- **The file** `data/trust/freshness.tsv` (`gaze-shard/src/fresh.rs`): a
  header, then `shard TAB binding TAB block` per line, with `%`, tab, CR
  and LF percent-escaped. Each shard's records are separate: another
  shard's lines are kept as they are. Loading merges duplicate lines to the
  highest block. The file is written atomically, owner-only, and only when
  a block rises, under the bridge's lock, so the saved records only rise.
- **Where they are kept** (`engine::freshness_log`): in the file, unless
  every observer runs on this machine (`localhost`, `*.localhost`,
  127.0.0.0/8, `::1`; storage plan, decision 7). A development shard is
  reset often, and persisted records would then refuse every site.
- **When they cannot be written.** A file that cannot be read is never
  overwritten: the records are kept in memory, and the window shows an
  Alert. A session that only reads enforces the file's records and writes
  none (`FileFreshness::read_only`, S14 Q3). A failure to save is shown
  once, and again only after a save has succeeded in between (S14).
- **Start-up's repair** (S6): a damaged file keeps its usable lines, its
  original backed up, and an Alert, since lost records are a downgrade.
- **After a shard reset**, its new blocks look like replays:
  `f1r3gaze trust list` shows the records, and `trust forget BINDING` or
  `trust forget --all` forgets this shard's, refused when the file could
  not be read.

**Tests.**
- `fresh.rs`: records round-trip per shard; duplicates merge to the highest
  block; odd bindings round-trip; damaged lines are found; forgetting
  removes only this shard's records; unreadable records are never
  overwritten; the file is private; a read-only log enforces and never
  writes, and keeps a first reason.
- `tests/mock_node.rs`: a rollback is refused on one bridge, and across a
  restart (two bridges on one file); records are saved only when a block
  rises; a failed save is reported and the records still hold; a repeated
  failure is reported once.
- `engine/tests.rs`: `observers_are_loopback_only_on_this_machine`;
  `freshness_records_are_kept_where_the_shard_says` (memory for a local
  shard, the file otherwise, read-only when the session reads only).
- `reconcile/tests.rs`: the file in every state, with its Alerts
  (`every_state_of_every_file_in_each_root`).
- `tests/cli.rs`: `trust_forget_touches_only_this_shard`.

**Mutation checks.** M6r11 (S6); M14k, M14l, M14l2, M14ak and M14al (S14).

---

## S11 — Secrets: keys, exports and the keyring's name

**Goal.** No secret is ever readable by another user, and none is written,
copied or removed where the user did not ask for it. The pieces came with
S4 (owner-only from creation), S6 (start-up never touches a key), S7 (the
move) and S14 (the exports' and logs' folder); this entry gathers them.

**What holds.**
- **Key files** (`gaze-shard/src/keys.rs`, `FileKeystore`) are written by
  `write_atomic` with `Perm::Private`: the temporary file is created 0600
  before any byte is written, so a key is never readable by others, not
  even for a moment (it was `chmod`ed after writing, before S4).
- **Start-up never writes, copies or removes a key file** (README section
  9.6). A damaged key is reported as an Alert, naming its wallet; a listed
  wallet with no key, and an unfinished save holding a key's only copy, are
  reported too. Wallets missing from a damaged list are listed again from
  their key files, never made the payer. The sweep never enters
  `wallet/keys`.
- **The move of an old profile** moves key files with their bytes, and
  tightens every moved file to at most 0600 (a link keeps its mode).
- **Exports and replay logs** are owner-only: `wallet export ADDRESS FILE`
  writes the file named, and the Wallet panel's Export writes
  `data/wallet/exports/<address>.json`; replay logs go to
  `data/replay-logs/`. A session that only reads writes neither.
- **The OS keystore's name stays `F1R3Gaze`**
  (`OsKeystore::new("F1R3Gaze")`): renaming it would orphan the keychain
  entries of every existing wallet on macOS and Windows.

**Tests.**
- `keys::tests::keys_round_trip_by_name` (0600 on Unix).
- `reconcile/tests.rs`: `corrupt_keys_are_reported_and_never_touched`,
  `wallets_are_listed_again_from_their_keys`, `recovery_never_chooses_a_payer`,
  `the_sweep_removes_only_dead_writers_temps` (never the keystore).
- `migrate/tests.rs`: `moved_files_are_private`, and the full profile's keys
  moved byte for byte.
- `chrome/tests.rs`: `replay_logs_and_exports_go_to_the_data_folder` (0600),
  `a_read_only_window_saves_nothing`.
- `tests/cli.rs`: `a_first_start_makes_a_private_skeleton` (every file 0600,
  every folder 0700).
- CI: `storage-migration-ci.sh` checks the key moved byte for byte and every
  file 0600.

**Mutation checks.** M6r7 and M6r10 (S6); M7 entries for the keystore's
folder (S7); M14h (S14).

---

## S15 (part 1) — The start-up benchmark, as a script

**Why.** S1 timed start-up with commands typed by hand, and that evidence
was lost with `target/` ("Lost evidence"). S15 repeats the measurement
after the layout, so the method is now versioned:
`scripts/storage-bench.sh`, whose tables are made by
`scripts/lib/storage-bench-table.py`.

**What it measures.** Two builds, the S1 rebuild (`before`) and the build
under test (`after`), each on profiles of its own under the output folder,
in an environment isolated as the harness isolates it, HOME included:
- **profiles:** `fresh` (removed before every run), `steady` (made by one
  start, then reused), `old-3` (the shape of the user's profile: the
  f6ee26a settings template, a user id, a workspace with one tab and four
  visits) and `old-full` (old-3 with every other item an old profile can
  hold, a wallet key among them), both restored before every run, so the
  `after` build moves them every time; and `moved`, the `after` build's
  start on old-full after one move;
- **commands:** `--headless gaze://newtab --timeout 5` (a page),
  `wallet list` (the whole engine, no page) and, for `after`,
  `profile check`;
- **timing:** hyperfine, pinned with `taskset`, 50 runs for a page and 100
  for the others after 5 warm-ups; before against after by Welch's t-test
  (the difference of the means with its 95 % interval) and the
  Mann–Whitney U test (no assumption of normal times);
- **what a start does to its profile:** one start under `strace -f -y -tt -T`,
  every call naming a path in the profile counted by kind, and the paths
  written listed in order; and `strace -f -c` for the totals.
`machine.txt` records the CPU, the governor, EPP, boost, the maximum
frequency, the file system and its mount options, the load and the busiest
processes, before and after the runs.

**A trial run, to check the script** (`--quick`, 5 runs and 1 warm-up;
the step-9 build `0dc5c59c…`; `target/scratch/storage/s15-quick/`).
- *First attempt:* exit 3, "dbus-send is …/env/fake-bin/dbus-send, not
  the fake". The self-test was given the output folder as its work folder,
  while the isolation was made in `env/` below it. Fixed by passing `env/`.
- *Second attempt:* it ran to the end in 28 s. Every exit code was the
  expected one (0; and 1 for `profile check` on fresh, old-3 and old-full,
  where a start would change something), and the tables were made.

**Seen before any prediction.** The trial's counts were seen while the
script was checked, so S15's predictions of them cannot be blind. They will
be derived from the code and compared with the trial and the full run
alike. What the trial showed:
- The baseline's steady start wrote nothing. A steady start of the step-9
  build opened 5 files to write and made 5 fsyncs, 2 renames and 1 unlink:
  the two lock files, their two pid files (each written atomically), and
  the start-up busy flag.
- `profile check` wrote nothing on any of the five profiles.
- Timings, 5 runs each, indicative only: `wallet list` on a steady profile
  took 2.8 ± 0.3 ms before and 4.2 ± 0.4 ms after. Two runs stood out:
  `after-page-fresh` reached 494.9 ms (median 262.7 ms), and
  `after-wallet-old-full` 253.9 ms (median 15.8 ms).

**A fact about this machine that S15 must account for.** The scratch
folder is on `/dev/nvme0n1p4`, ext4 mounted `nobarrier`: an fsync there
does not ask the drive to flush its cache. fsync costs measured on this
machine are therefore lower than on a file system with barriers, the
default. (A loop-mounted image would not help: its flushes become fsyncs of
a file on this same file system.)

---

## S12 (part 3) — The window follows the system: Appearance, pages and the title bar

**Goal.** Plan step 10 (§3.5, §3.6), with the user's answers of 2026-10-05:
- Appearance offers System, Dark and Light, with a sentence on what System
  found; it lists every theme file; a file that cannot be used says why;
- the theme files keep two buttons, **New theme** and **Reload**;
- web pages' own light and dark styles follow the scheme the browser shows
  (chrome ledger L13);
- the window's decorations follow the scheme too;
- `theme:` actions parse strictly, and the harness shows all of it.

**Design.** A Plan agent's report, kept as
`target/scratch/storage/s12-part3/design-report.md`, with every engine fact
checked against the sources before any code. Its findings:
1. **No scrollbar is ever painted.** blitz-paint draws them only with its
   `scrollbars` feature, which nothing enables, so the plan's predicted
   churn of "dark-scene scrollbar thumbs (L13)" was wrong.
2. **L13 alone changes no existing capture.** No harness page, built-in
   page or chrome rule uses `prefers-color-scheme`, `light-dark()` or a
   system colour.
3. **`View::init` puts the chrome's scheme back to the window's**, which is
   light on X11. Only the window can keep it: hence `WindowRequest::Scheme`
   and Blitz's theme override (L13, H3).
4. **winit-win32 re-themes a window created without a theme** at every
   settings change, even after `set_theme(Some)`. So the chrome asks for the
   scheme again after every report, and step 11 must create its window with
   no theme on Windows.
5. **macOS stops reporting** while a window has an appearance of its own:
   hence `FollowSystem` when System is chosen again.
6. **`theme:next` had no live use**: its toolbar button has been a comment
   since the redesign. Nothing replaces it.
7. **Two S14 mutation specs** name code this step replaces (M14u, M14x).
8. **The date confound**: history scenes show a date 45 days back, so the
   full run was compared with a reference of the same UTC day.

One more finding came from checking the design against Stylo: with the
design alone, a page that declares no colour scheme would have turned its
highlights, dialogs and popovers dark (L13, H2). Every page now gets
`:root{color-scheme:light}`, which the page's own `color-scheme` overrides.

**Defaults taken**, where the approved plan left a choice:
- New theme chooses the theme it creates, so "edit it, then reload" works
  at once;
- the notice for a chosen theme that cannot be used goes at the top of the
  panel, where it is never below the fold;
- a row's reason wraps (it may hold a path and a line number);
- the sentence under the control drops "Sites keep their own styles", which
  L13 makes false;
- an unreadable preference and an unknown one share one sentence;
- the panel's order is the plan's (control, themes, swatches, card). The
  design asked whether the card should come before the swatches; the
  approved plan already gives the order.

**Built.**
- `chrome.rs`: `ThemeOp` (`system`, `dark`, `light`, `use:<name>`, `new`,
  `reload`; anything else refused); `WindowRequest` and its queue
  (`request_scheme`, `follows_system`, `take_window_requests`);
  `window_reported_scheme` asks for the scheme after every report; the
  Appearance builder (`AppearanceView`, `scheme_note`, the theme rows, the
  Theme files card); `theme_op`, `use_theme`, `new_theme`
  (`NEW_THEME_TRIES` names, `write_new`), `reload_themes`; the theme list
  cached and read again when Appearance is shown or reopened; L13's viewport
  line in `paint_theme`. The step-9 code it replaces (`CUSTOM_THEME`,
  `PaletteStatus`, `theme_action`, `create_custom_theme`, the old builder and
  renderer) is commented out with its reason.
- `tab.rs`: `PAGE_SCHEME_CSS`, every page's extra style sheet.
- `application.rs`: `carry_out_requests` after `can_create_surfaces`, after
  every window event and in `about_to_wait`; `show_scheme` (the override and
  the decorations, each only when it changes); `decoration_theme`,
  `scheme_change`; the decorations forgotten after a report.
- Tests: 11 chrome tests and 2 application tests (below); `ScratchProfile`
  gains themes installed for everyone.
- Harness: scenes 72–84, the `portal` answer reset in every new profile, the
  helpers, `FORMAT_IN`, calibration of the Appearance panel, and the
  recalibrated coordinates.
- Docs: `docs/ui/README.md` §3.2, §3.6 (new), §4.3, §7, §8.13, §11, §12;
  `docs/storage/README.md` §6.2, §12, §15.3; the diagram
  `diagrams/theme-change-sequence`; the crate README; L13.

**Experiments.** Evidence in `target/scratch/storage/s12-part3/`.
- **L13, H1–H3**: the chrome ledger's L13 entry (red, then each part of the
  fix, then the hand mutation in the window).
- **E1, the suite's first run** (`lib-tests-1.log`): 333 of 334.
  `new_theme_copies_the_current_colors` failed in its own helper, which
  listed every name ending in `.css`, the folder the test put in the way
  included. The helper now lists files only: 334 of 334 (`lib-tests-2.log`).
- **E2, calibration** (`target/ui-snapshots/calibrate-after-appearance*.png`).
  The segments' predicted centres held (System 103, Dark 192, Light 281, at
  y = 172). The third theme row is at y = 420, not 437, and the card's
  buttons, scrolled to the end, at (110, 736) and (205, 736). Scrolled to
  its end, a panel puts its card in the same place whatever it holds above,
  as long as it overflows, so scene 81 lists the same three theme files as
  the calibration.
- **E3, the first runs of the new scenes** (`iter-1/`). Scene 16's notice
  checks failed, as expected: the notice they measure is now the Theme
  files card's, lower (the crops were measured again: the notice's fill
  starts at y = 630, and its bar covers x = 61–63). Scene 76's
  `sidebar_px_vs_73` was 40.07 where 0 was predicted.
  - *Hypothesis:* the difference is the themes folder the card shows, which
    holds the profile's name ("…ight/" in 73, "…ocus/" in 76): ledger L10's
    confound.
  - *Experiment:* where the two captures differ.
  - *Result:* one 26 × 12 box at (193, 626), the path's text.
  - *Fix:* scene 76 uses scene 73's profile path, as the theme pairs do.
    Rerun (`iter-2/`): 0.
- **E4, the full run against its prediction.** The prediction, written
  before the run (`churn-prediction.tsv`): 56 scenes byte-identical to the
  S14 run of the same UTC day (`s14/harness-final`), and 15 (15–18, 52–59,
  69–71) changed only inside the sidebar. The run (`harness/`, binary
  `f48811d8…`, 2026-10-06T05:23Z): 84 scenes, 0 failed, 82 of 82 checks.
  The comparison (`churn/result.tsv`): 71 of 71 as predicted, with the 15
  changed ones at AE 0 outside `CROP_SIDEBAR`; the 13 new scenes made.
- **E5, the harness's portal reset (H-reset, by hand).** With `portal 0`
  taken out of `new_profile`, scene 82 run after 76 inherited 76's light
  answer: `strip_px_vs_15 = 38 526` and `error_notice_px = 0`, both FAIL
  (`hand-mutations/h-reset-harness.log`). The script was restored and
  compared (`cmp`).

**Checks** (`matrix/run-1/`).
- `cargo test --workspace --locked`: 478 passed, 0 failed, on stable and on
  1.95 (465 + 13); `-p gaze-shell --features frame-times`: 349.
- Clippy with `-D warnings`: clean on stable and 1.95, and with
  `frame-times`, `storage-trace`, and `crash-points,storage-trace`.
- `shellcheck`: no new kind of finding in `ui-snapshots.sh`.

**Mutation checks** (`s12-chrome.json`; `mutations/`). All 25 went red, and
every file was restored byte for byte:

| ID | Mutation | Test that went red |
|---|---|---|
| M12r | the chrome never shows its scheme to pages | `pages_follow_the_chrome_scheme` |
| M12s | the theme override never set | `a_scheme_request_changes_only_what_differs` |
| M12t | decorations set while following the system on macOS and Windows | `the_title_bar_follows_the_system_only_where_the_os_reports_changes` |
| M12u | X11 and Wayland decorations left without a theme | the same |
| M12v | decorations set again when unchanged | `a_scheme_request_changes_only_what_differs` |
| M12w | scheme requests not coalesced | `the_window_is_asked_to_show_the_scheme` |
| M12x | a window's report not answered with the scheme | the same |
| M12y | System chosen without `FollowSystem` | the same |
| M12z | System chosen without asking the portal again | `choosing_system_asks_the_system_again` |
| M12aa | a theme file that fell back not counted as following the system | `the_window_is_asked_to_show_the_scheme` |
| M12ab | `theme:next` parsed again | `actions_parse` |
| M12ac | `theme:use:` without the name check | `actions_parse` |
| M12ad | New theme writes over what is in the way | `new_theme_copies_the_current_colors` |
| M12ae | New theme writes in a read-only session | `new_theme_is_refused_in_a_read_only_session` |
| M12af | New theme stops at a name something unlisted holds | `new_theme_copies_the_current_colors` |
| M12ag | Reload keeps the old list | `a_theme_file_is_chosen_reloaded_and_dropped_when_broken` |
| M12ah | Reload does not read the chosen theme again | the same |
| M12ai | a theme file that cannot be used is choosable | `the_appearance_panel_lists_theme_files` |
| M12aj | no notice for a chosen theme that cannot be used | `an_unusable_choice_says_which_scheme_is_shown` |
| M12ak | the System sentence silent on a system with no preference | `the_system_sentence_reports_the_preference` |
| M12al | System never lit | `the_appearance_panel_lists_theme_files` |
| M12am | the list not read again when Appearance is shown | `opening_appearance_lists_the_theme_files_again` |
| M12an | the list not read again when the sidebar reopens on it | the same (extended to cover it) |
| M12ao | a theme file that cannot be used is chosen | `an_unusable_theme_cannot_be_chosen` |
| M12ap | a page that declares no scheme gets dark system colours | `pages_keep_light_system_colours_unless_they_support_dark` |

By hand, in release builds: H-L13 (the override's call commented out:
scene 83 FAIL; chrome ledger L13) and H-reset (E5).

The specs this step touched were run again: `s14-switch-over.json`, 39 red
and two retired, M14u newly ("Create palette file became New theme; M12ad
guards New theme's `write_new`"), with M14x moved to the restructured
`window_reported_scheme` and red; `s12-themes.json` 10 of 10;
`s12-system-theme.json` 7 of 7.

**Not verifiable on this machine.** The decorations on macOS and Windows
(`FollowSystem`, the re-theme at a settings change, the title bar following
an explicit choice) are verified in the winit sources only. They need the
manual checks the plan asks about at S16.

**Open.** The committed captures in `docs/screenshots/ui/after/` still show
the step-9 Appearance panel (and have no scenes 72–84). They are
regenerated only with the user's go-ahead.

---

## S13 (part 2) — The window made where it was, followed, and full screen

**Goal.** Plan step 11 (§3.4, §3.6): the window opens at the size and place
it was left, maximized or full screen as it was, and at its zoom, and
`state/window.json` follows it; F11 (Ctrl+Cmd+F on macOS) enters and leaves
full screen; and the storage scenes the plan's §5.5 lists that no step had
built (an old profile moved by a start with no `--profile`, a move that
meets a file, damaged state files, a second instance).

**Design.** A Plan agent's report, kept as
`target/scratch/storage/s13-part2/design-report.md`, every engine fact
checked against the sources. Its findings:
1. **Wayland's monitor scale is the whole-number output scale**
   (winit-wayland `output.rs:53-56`), so step 8's clamp would have shrunk a
   window on a fractional-scale desktop: a 2560×1440 output at 125 %
   measured 1280×720. The compositor bounds a new window itself
   (`configure_bounds`); the clamp is gone on Wayland.
2. **With no window manager, X11 reports no move** (`Moved` comes only
   from a synthetic ConfigureNotify) **and carries out no maximize**: the
   window must be read, not only watched.
3. **No event need report the first size**, so a reading is due 500 ms
   after the window is made.
4. **winit-x11 reports `Focused(false)` when a window is mapped**: a focus
   loss saves only once the window has had the focus.
5. **Blitz's zoom keys have no bound** and hold the zoom in an `f32` (L15).
6. **The design took the resize bench's sweep to last about 3.3 s** (the
   `span_ms` of L9's runs), longer than the 3 s bound, and so expected one
   save during it. E5 refuted this: the sweep lasts 1.6 s, and `span_ms`
   had counted the 1.5 s wait before it.
7. **A huge size in a damaged file would end every start** where no monitor
   bounds it (winit-x11 converts sizes to 16 bits with an `unwrap`): capped
   at 8192×8192.
8. **winit's macOS menu takes Cmd+H for Hide** before the window sees it
   (L16).
9. **Nothing in F1R3Gaze sends Blitz's `CloseWindow`**: `CloseRequested` is
   the one close to read the window before; `Drop` covers a loop that ends
   otherwise (macOS's Quit).
10. **The chrome goes with the window at `CloseRequested`**, so the pending
    window carries its own saver.
11. **No churn predicted in the 84 existing scenes.**

**Defaults taken.** Wayland's size left to the compositor; the full-screen
hint shown on entering, and when the window is made full screen; the zoom
kept to hundredths and clamped live; a focus loss saving only after a focus;
a first reading 500 ms after the window is made; `Drop` writing the last
reading without reading the window; no signal handler (a killed process
loses at most 500 ms of window changes, 3 s during a drag, as the history
loses up to 2 s); the window-manager scenes last; `F1R3GAZE_KEEP_WINDOW` for
an A/B in one binary. The design's question Q2 (L16) had a default, Cmd+Y,
the key Safari and Chrome use; it was taken. Its question Q1, whether
openbox may be installed for scenes 98 and 99, is held for the questions of
S16.

**Built.**
- `window_state.rs`: `MAX_SIZE`; the Wayland and no-monitor arms of
  `plan_restore` replaced (the old ones commented out with their reasons);
  `round_zoom`, `zoom_correction`; `MonitorReading` and
  `Monitor::from_platform`; `WindowReading` and `Sample::from_platform`;
  `Keeper`.
- `application.rs`: `PendingWindow`, `SaveWindow`, `KeptWindow`;
  `create_window` in `can_create_surfaces`; `observe_window`, `save_window`,
  `follow_zoom`, `toggle_full_screen`; the reads at `CloseRequested`, at a
  focus loss and when due (`about_to_wait` and `ControlFlow::WaitUntil`);
  `Drop`; the pure helpers `window_system_of`, `creation_theme`,
  `window_attributes`, `changes_window_state`, `loses_focus`,
  `control_flow_for`.
- `chrome.rs`: `Action::FullScreen`, `full_screen_chord`,
  `browser_shortcut` (the chord before Find; a repeat ignored),
  `WindowRequest::ToggleFullScreen`, `scheme_shown`, `full_screen_changed`
  and `full_screen_hint`; `launch()` hands over a `PendingWindow`; History on
  Cmd+Y on macOS (L16); `KEY_LABELS`, `key_label`, `key_labels` (L17).
- `frame_stats::WINDOW_SAVE` and the frame line's `window_saves` and
  `window_save`; `frame_stats::WindowSave`, which also prints each reading
  of the window on a line of its own (`window_save epoch=… took=…
  changed=…`), and `wall_clock_ms`, shared with the frame line (E5).
- `resize-bench.sh`: `--keep-window`; `--watch-saves`, which counts the
  writes of `window.json` with inotify (`scripts/lib/watch-renames.py`);
  `sweeps.tsv` and `saves-N.tsv`; a run's row delimited by its sweep's own
  start and end; `saves`, `save_writes` and `save_max_ms` counted from the
  `window_save` lines (E5).
- The harness: scenes 85–99, `launch_restored`, `launch_env`,
  `close_window`, `new_machine`, `machine_self_test`, `seed_window`,
  `seed_legacy`, `start_wm` and the others (`docs/ui/README.md` §12.2);
  `storage_paths_inside` factored out of the isolation library; `usage`
  prints the whole header (it stopped at line 42, two lines short since
  step 10).
- Docs: the storage README §2 (the components diagram), §13 (making the
  window, Wayland, reads and saves, full screen, zoom; the diagram
  `window-geometry-sequence`); the UI README §3.6, §3.7 (new), §8.5, §9,
  §11, §12; the crate README; `lib.rs`; L15, L16 and L17. Seven rustdoc
  links to private items made code spans (three from this branch, four
  from `main`), so `cargo doc` gives no warning.

**Experiments.** Evidence in `target/scratch/storage/s13-part2/`.
- **E0, the X server's monitor.** `xrandr` names Xvfb's output `screen`
  (`red-harness/run.txt`), the name winit gives the monitor.
- **E1, the red run** (`red-harness/`, the step-10 binary `f48811d8…`,
  scenes 02, 19 and 85–97). As predicted, every window-keeping scene
  failed: the window opened at winit's 800×600 (85, 86, 87),
  `window.json` was never written (85, 87, 88, 89, 90, 92, 93), and F11 did
  nothing (93). After twelve presses of Ctrl+− the window showed only its
  background (L15). Scenes 94–97 passed apart from checks of the window's
  size and place.
- **E2, scene 94's history.** It counted 9 visits where 8 were seeded: the
  window records one when its page loads, saved within 2 s, so the count
  depended on timing. The scene now counts the visits from before the
  start.
- **E3, the zoom's rendering.** On the new binary (`iter-1/`), scene 91's
  window, restored at 1.2, differed from scene 90's, zoomed to 1.2 with the
  keys: AE 2 075.16, a one-row shift of the status bar's border and
  sub-pixel glyph positions in the tab strip and the address field; the
  page itself was identical. Scene 92 likewise (AE 980.3). Both zooms are
  the same `f32`, 1.2000000476837158.
  - *H-a, a capture taken too early:* scene 90 with 3 s more before the
    capture. *Result:* AE 0 against scene 90. Refuted.
  - *H-b, the route to the zoom matters:* scene 91, then Ctrl+0 and Ctrl+=
    twice; and scene 90, then the same. *Result:* AE 0 against scene 91 and
    against scene 90 respectively, and the two still differ (AE 2 075.16).
    So what decides the rendering at a zoom is the zoom the window started
    at, not the route, nor F1R3Gaze's restore. Blitz's layout keeps
    something of its first zoom; both renderings are legible. An upstream
    note is among the questions of S16.
  - *Change:* scenes 91 and 92 compare a window with itself: restored, then
    after Ctrl+0 and the keys back to the same zoom. They match only if the
    zoom restored is exactly the keys' (`iter-2/`: AE 0 for both).
- **E4, the full run against its prediction.** The prediction
  (`churn-prediction.tsv`, written first): scenes 01–84 byte-identical to a
  control run of the step-10 binary through the current script, on the
  same UTC day. The control (`control/`, 07:18Z): 84 scenes, 82 of 82
  checks. The full run (`harness/`, binary `fad9eb22…`, 07:25Z): 97
  scenes, 0 failed, 128 of 128 checks. The comparison (`churn/result.tsv`):
  84 of 84 identical; scenes 85–97 made; 98 and 99 not run (Q1).
- **E5, the resize bench with and without the window kept** (`bench/`;
  each round's prediction written first in `bench/prediction.txt`; A/B in
  one binary through `F1R3GAZE_KEEP_WINDOW`; 5 runs a side, the 4 px sweep
  at 120 Hz).
  - *Round 1* (binary `bcd63c3d…`). Predicted: with the window kept, one
    save during the sweep (finding 6) and three writes of `window.json` a
    run; without, none. Result: `saves` 1 and 0; inotify counted no write
    during any sweep and one a run. Refuted: the save during the sweep, and
    the write after it, since the sweep ends at 1280×800 at (0, 0), where
    the window was saved before it, so the reading after it finds nothing
    to write. Cause: a run's span began at the first frame with a resize,
    which is the set-up's, so it took in the 1.5 s wait and the reading
    made in it. The sweep itself lasts 1617 ms.
  - *Fix 1:* a run's row covers the frames painted from the sweep's start
    until a second after its end (`sweeps.tsv`).
  - *Round 2* (`rerun/`, A/B/A: kept, not kept, kept again). The span was
    1617–1640 ms (predicted 1650–1800 ms: see *Timing*) and `extra_frames`
    0, as predicted; but `saves` was still 1 with the window kept. Cause (H-attr, from the logs): the reading is
    made 1126–1153 ms before the sweep (inotify's time of the write), and
    reported on the sweep's first frame, 25–54 ms after its start. The
    frame line counts the readings since the frame before, and no frame is
    painted in the wait.
  - *Fix 2:* each reading prints its own line, with its wall-clock time
    (`frame_stats::WindowSave`), and the bench counts the readings made
    from the first resize's arrival (its frame's `epoch` less its
    `latency`) to the last resize frame.
  - *Round 3* (`rerun2/`, binary `7d117103…`). As predicted: with the window
    kept, no reading and no write in any of the 10 sweeps, and exactly two
    readings a run, 1129–1217 ms before the sweep (it wrote: the run's one
    write) and 490–534 ms after its end (it found no change); without it,
    no reading. Given the same logs, the old count reports one reading in
    every kept sweep, as long as the reading made before it
    (`attribution-check/`).
  - *Positive control* (`long-sweep/`): at 40 Hz the sweep lasts 4.8 s,
    longer than the 3 s bound. As predicted, each sweep had one reading
    and one write, 3.04–3.07 s after its start, and each run three writes
    (before the sweep, during it, and after it, where it ends at a size
    other than the one saved during it).
  - *Timing* (`ab-statistics.txt`; Mann–Whitney, two-sided). In round 2
    (load 7–12) the kept runs' median interval between frames was 2.0 ms
    shorter than the unkept ones' (29.85 and 31.99 against 32.79 ms; p =
    0.040), but the two kept sets differed from each other as much (p =
    0.032): drift on a shared machine, since nothing of the keeping runs
    during a sweep. In round 3 (load about 40, steady, other test programs
    running) nothing differed (38.42, 39.94 against 40.54 ms; p = 0.768; no
    column below p = 0.46). The predicted band (a difference under 2 ms, p
    above 0.05) held in round 3; in round 2 it failed for drift, not for
    the keeping. `span_ms` was below the predicted 1650–1800 ms in round
    2: the span starts at the first resize frame's present, 25–54 ms after
    the sweep's start, which the prediction left out.
- **E6, hand mutations of the window's keeping** (`hand-mutations/`; each
  built into its own target directory, the file restored and compared byte
  for byte before the harness ran the scenes predicted to fail, with scene
  19 as their reference):

  | ID | Mutation | Predicted to fail | Result |
  |---|---|---|---|
  | H-close | no reading at `CloseRequested` | 89 `saved_at_close` | failed (0) |
  | H-first | no first reading after the window is made | 85 `first_save` | failed (0) |
  | H-zoom | Blitz's zoom not set back into range | 92 `zoom_floor_px_vs_keys` | failed (AE 870 133) |
  | H-wait | the loop never woken to save (`ControlFlow::Wait`) | 85 `first_save`; 88 `saved_after_drag` | 88 failed (990, the width saved during the drag); **85 passed** |

  The prediction for H-wait was refuted in part: E7.
- **E7, why H-wait left scene 85 passing.** *H85:* another wake-up carries
  the late first reading. The history's save, due 2 s after the page's
  visit, wakes the loop, and `about_to_wait` then finds the reading due.
  *Prediction:* with the step's binary, the window's state is written
  about 0.5 s after the window is made, well before `history.json`; with
  H-wait, within a few milliseconds after it. *Result* (`h-wait-85/`, scene
  85 alone, the profiles kept; times from the start of the scene's
  process): step's binary, `session.json` +1.181 s, `window.json` +1.572 s,
  `history.json` +3.178 s; H-wait, `session.json` +1.158 s, `history.json`
  +3.155 s, `window.json` +3.156 s, 0.97 ms after it. Confirmed. Scene 85
  now also checks `first_save_before_history`: the window's own state
  written before `history.json`. Its first form passed the step-10
  binary, whose `window.json` is start-up's default (reconcile makes a
  missing state file before the window exists); it now counts only the
  window's own state. Run again: the step's binary 1, H-wait 0, the
  step-10 binary 0, as predicted.
- **E8, the final run** (`final/`, binary `fe7c7049…`, built after E5's
  instrument and E7's check; prediction written first). Predicted: 97
  scenes, 129 checks passing (E4's 128 and E7's), and every capture
  byte-identical to E4's, since the release build's behaviour did not
  change. Result: 97 scenes, 0 failed, 129 of 129 checks; 97 of 97 captures
  identical (`cmp`); the only value that moved is scene 77's time to a
  title with a portal that never answers, 634 then 740 ms (limit 3 000).
  Confirmed.

**Checks** (`matrix/run-1/`, and at the end `final/matrix/`).
- `cargo test --workspace --locked`: 498 passed, 0 failed, on stable and on
  1.95 (478 + 20); `-p gaze-shell --features frame-times`: 369, then 372
  with E5's three tests of the instrument.
- Clippy with `-D warnings` in the five configurations: clean.
- `cargo doc -p gaze-shell --no-deps`: no warning.
- `shellcheck`: `resize-bench.sh`, `storage-bench.sh` and the isolation
  library clean (an old `printf` with a variable format now `%b`);
  `ui-snapshots.sh` has no new kind of finding. The usage of
  `storage-bench.sh` now prints the header up to its first empty line, as
  the other scripts' do: its fixed range (lines 2–55) was still exactly the
  header, but would go stale as theirs had.

**Mutation checks** (`s13-window.json`; `mutations/`, and after E5 and E7
`mutations-2/`). All 26 went red, and every file was restored byte for
byte:

| ID | Mutation | Test that went red |
|---|---|---|
| M13m | an unchanged state written | `only_changes_are_written` |
| M13n | a failed write counted as written | `a_failed_write_is_tried_again` |
| M13o | macOS samples kept in physical pixels | `samples_are_read_in_each_platforms_units` |
| M13p | macOS monitors kept in physical pixels | `monitors_are_described_in_desktop_units` |
| M13q | Wayland clamped by the whole-number output scale | `wayland_leaves_the_size_to_the_compositor` |
| M13r | no cap where no monitor bounds the size | `sizes_without_a_monitor_are_capped` |
| M13s | Blitz's `f32` noise kept in the zoom | `the_zoom_is_kept_in_range` |
| M13t | the zoom left out of range (L15) | `zoom_keys_stay_within_range` |
| M13u | Windows' window made with a theme | `a_window_is_made_with_no_theme_on_windows` |
| M13v | macOS placed in physical pixels | `the_window_is_made_at_the_planned_size_and_place` |
| M13w | a resize schedules no save | `only_geometry_events_schedule_a_save` |
| M13x | the X11 map's focus loss saves | `only_losing_a_focus_it_had_saves_at_once` |
| M13y | the loop never wakes to save | `the_loop_wakes_when_a_save_is_due` |
| M13z | XCB taken for a system that places windows | `the_window_system_comes_from_the_display` |
| M13aa | F11 with a modifier is the chord | `the_full_screen_chord_is_platform_specific` |
| M13ab | Find before full screen | `full_screen_comes_before_find_and_ignores_repeats` |
| M13ac | a held chord toggles again | the same |
| M13ad | the chord asks the window nothing | `the_full_screen_key_asks_the_window_once` |
| M13ae | no hint on how to leave full screen | `full_screen_says_how_to_leave_it` |
| M16, M16b | History without a key, or on Cmd+H, on macOS (L16) | `history_is_cmd_y_on_macos_and_ctrl_h_elsewhere` |
| M17, M17b | a tip naming Ctrl on macOS; the tokens left (L17) | `every_key_label_names_a_key_that_does_what_it_says` |
| M13af | a reading of the window not counted on the frame line | `a_reading_is_counted_like_a_span` |
| M13ag | a reading's line without its wall-clock time | `the_reading_line_says_when_how_long_and_whether_it_wrote` |
| M13ah | the wall clock in seconds, not milliseconds | `the_wall_clock_is_in_milliseconds_since_the_epoch` |

The last three run with `--features frame-times`. The bench's own fix (E5)
was checked the same way by hand: the old count, given round 3's logs,
reports the reading made before each kept sweep
(`attribution-check/old-vs-new.txt`). Scene 85's new check went red with
H-wait and with the step-10 binary (E7).

The specs this step's code touches were run again: `s13-window-state.json`
12 of 12 red (its anchors all still on live code), `s12-chrome.json` 25 of
25, `s14-switch-over.json` 39 red and two retired.

**Not verifiable on this machine.** Its desktop is KDE Plasma on Wayland,
and the harness runs X11 under Xvfb with no window manager; scenes 98 and
99 need openbox.
- macOS (winit-appkit): the window placed in points, the frame's corner
  and the surface's offset; full screen's animation and Ctrl+Cmd+F; Cmd+Y
  (L16); Quit, which closes no window first, saved through `Drop`.
- Windows (winit-win32): the window made with no theme; the scale at
  creation; maximize after the window is shown; a minimized window's
  position of −32 000, which the tracker skips.
- Wayland (KWin): the size restored and left to the compositor; full
  screen and maximize; no position, by design.
- With a window manager on X11: full screen and maximize restored (scenes
  98 and 99).

These need the manual checks the plan asks about at S16, and openbox
(question Q1, held for S16).

**Open.** Q1, whether openbox may be installed, so scenes 98 and 99 can
run. The committed captures in `docs/screenshots/ui/after/` still show the
step-9 Appearance panel and have no scenes 72–97; they are regenerated only
with the user's go-ahead. Whether to file Blitz's dependence of a zoom's
rendering on the zoom a window started at (E3) upstream, after searching
its issues and pull requests, is among the questions of S16.

---

## S15 (part 2) — What a start writes and costs

**Goal.** Plan §5.6: time a fresh start, a steady start and a migration
before the five roots and after them; record what each start does to its
profile; and explain the outliers part 1's trial showed. Evidence in
`target/scratch/storage/s15-part2/` (the predictions in `prediction.md`,
each written before its run), `target/scratch/storage/s15/` (the first
full run) and `target/scratch/storage/s15-full-2/` (the run with pairs).

**Builds.** After: `target/release/f1r3gaze` `fe7c7049…`, the end of step
11. Before: the S1 rebuild of `3d5ba26`, `8997869f…`. The machine:
`machine.txt` in each run (Threadripper PRO 5975WX, CPUs 2–9, governor and
EPP `performance`, boost on; the profiles on `/dev/nvme0n1p4`, ext4,
`nobarrier`; load 9 in the first run, 15 in the second, with a
whole-system rdiff-backup, postgres and other builds running).

**The plan's prediction**, "steady state does 0 renames and 0 fsyncs and
stays within noise", was already refuted by part 1's trial: a steady start
makes 5 fsyncs, 2 renames and 1 unlink. They come from two later parts of
the design. The amendment's anchor lock writes an `instance.pid` beside
each of the two locks atomically and durably (4 fsyncs, 2 renames). The
busy flag replaced the plan's barrier at every start: it is made, and at
the end of start-up removed and its folder synced (1 fsync, 1 unlink),
where the barrier would have synced every folder. Its second half, "fresh
and migration starts add a few milliseconds", held (below).

**P1, what each start writes.** *Derivation:* each start was run once
under a `storage-trace` build of the same source (`dcb0fea0…`,
`derive.sh`), which writes every `Fs` operation as a line of JSON.
`predict.py` maps each operation to the system calls `StdFs` makes for it
on Linux. `create_new` is an `openat` that creates and one `write`;
`sync_file` and `sync_dir` one `fsync` each; `hard_link`, `rename`,
`remove_file`, `remove_dir` and `set_mode` one call each. `create_dir_all`
makes $`2k-1`$ `mkdir` calls for $`k`$ missing folders, since std tries the
deepest first and fails with `ENOENT`. To that it adds the lock files'
calls, which `lock.rs` makes with std itself: per lock an `openat`, a
`flock`, an `ftruncate` and a `write`, and the runtime lock's `fchmod`.
The before build's rows come from its source: a fresh start
`create_dir_all`s the profile and writes `settings.conf`, then again for
`user-id` (the second `mkdir` an `EEXIST`); otherwise it writes nothing.
*Prediction:* exactly those counts, in all 11 columns, for all 23 starts.
*Result:* 0 of 253 cells differ, in both full runs. *Verdict:* confirmed:
every write a start makes goes through gaze-fs or the lock. (That the
derivation agreed with part 1's trial, made with the step-9 build, was
seen before the full runs.)

| Start (page or `wallet list`) | open to write | write | fsync | rename | link | unlink | rmdir | mkdir | chmod | ftruncate | flock |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `steady`, `moved` | 5 | 4 | 5 | 2 | 0 | 1 | 0 | 0 | 1 | 2 | 2 |
| `fresh` | 14 | 13 | 49 | 2 | 9 | 10 | 0 | 16 | 1 | 2 | 2 |
| `old-3` | 15 | 14 | 66 | 3 | 12 | 14 | 1 | 26 | 4 | 2 | 2 |
| `old-full` | 15 | 14 | 84 | 4 | 20 | 22 | 6 | 24 | 9 | 2 | 2 |
| `profile check`, any profile | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

**P2, timing** (the first full run, hyperfine, medians; $`\Delta`$ = after
less before):

| ID | Prediction | Result | Verdict |
|---|---|---|---|
| T1 | page, steady: $`\lvert\Delta\rvert \le 3`$ ms | +3.58 ms | refuted (H-drift) |
| T2 | `wallet list`, steady: +1.0 to +2.5 ms, Mann–Whitney p < 0.001 | +1.28 ms, p = 1.6e-29 | confirmed |
| T3 | `wallet list`: fresh < old-3 < old-full, as their fsyncs, each +4 to +16 ms | +5.63, +7.34, +8.92 ms | confirmed |
| T4 | page, fresh / old-3 / old-full: 0 to +20 ms | +6.35, +8.80, +8.62 ms | confirmed |
| T5 | `profile check` on steady faster than `wallet list` on steady | 2.67 against 3.75 ms | confirmed |
| T6 | moved within 1 ms of steady | page 0.93 ms, `wallet list` 0.01 ms | confirmed |

**P3, where the time goes (H-fsync).** *Prediction:* the summed time inside
fsync in one start under `strace -T` rises with the start's fsyncs, and is
at least a third of $`\Delta`$ for fresh, old-3 and old-full. *Result*
(counting every fsync of the start, the barrier's syncs of the folders
above the profile too: 63, 79 and 97): the share was 65–98 % in both runs.
The sum rose with the count for the page starts in both runs, but not for
`wallet list` in either: old-full 5.83 and 5.92 ms against old-3 6.68 and
6.94 ms, its 97 fsyncs averaging 0.060–0.061 ms against old-3's
0.085–0.088 ms. Fsyncs differ in cost, so their sum need not follow their
count. *Verdict:* the share confirmed; the ordering refuted for two of four
series (one traced start each). strace's stops make each call look longer,
so the shares are upper bounds.

**P4, the outliers.** Part 1's trial had two: `after-page-fresh` 494.9 ms
(median 262.7) and `after-wallet-old-full` 253.9 ms (median 15.8).
- *H-out:* a slow start waits in one fsync behind other programs' writes.
  *Test* (`h-out/`): 200 migrating `wallet list` starts under
  `strace -f -T -e trace=fsync`. *Prediction:* every start slower than the
  median plus 10 median absolute deviations holds an fsync longer than
  50 ms, and no faster start does. *Result:* 10 slow starts. Seven held
  many slow fsyncs, not one (sums 68–627 ms, the longest 35–56 ms each), in
  consecutive starts (19–20, 89–91). Three held none (sums 8.7–21.8 ms).
  *Verdict:* refuted.
- *H-out′:* the slow-fsync starts fall in episodes when the disk is busy
  with other programs' writes, since every journal commit then waits. The
  profiles are on `/`, where a whole-system rdiff-backup (state `D`) and
  postgres ran, and the disk was 22–73 % busy in the seconds sampled.
  *Correlational test* (`h-out-2/`, the starts timed and
  `/proc/diskstats` sampled every 100 ms): no slow-fsync start occurred,
  so nothing could be compared, and a start's own writes dominate a
  100 ms window anyway. *Interventional test* (`h-out-3/`, 200 starts in
  blocks of 50, alternately with `fsync-writer.py` writing and syncing
  64 KiB in a loop on the same file system). *Prediction:* a single
  fsync's median at least 10 times higher with the writer; starts above 3
  times the plain median sum at least 50 % with it and at most 5 %
  without. *Result:* 0.066 against 0.061 ms (1.1 times); 10 % against 0 %.
  *Verdict:* refuted: small synced writes slow few starts.
- *H-out″:* what stalls a start's fsyncs is another program's large
  flush, as a database's checkpoint makes it. In ordered mode ext4 writes
  a file's data before the journal commit that names it (\[Linux-ext4\],
  README section 16), so a commit an fsync needs waits for the flush. *Test* (`h-out-4/`, as
  h-out-3 with `checkpoint-writer.py`: 64 MiB written, one fsync, 200 ms of
  rest). *Prediction:* at least 20 % of the starts with the writer above 3
  times the plain median sum, at most 5 % without, and the longest fsync of
  such a start at least 10 ms. *Result:* 23 % against 1 %; their longest
  fsyncs 12.1–51.9 ms. *Verdict:* confirmed, by intervention. The natural
  episodes (35–56 ms per fsync, many in a row) fit a longer flush of the
  same kind.

A start's exposure to such stalls grows with its fsyncs: 5 when steady, 63
to 97 when it makes or moves a profile.

**T1, explained.**
- *H-page:* the page's extra beyond the shared start-up is user-space work
  on the page's path (the TOML settings, the theme's files, L13's style
  sheet). *Test* (`perf/`): `perf stat -r 30` of each build's steady
  start (`task-clock:u`; `perf_event_paranoid` is 2). *Prediction:* the
  page's task-clock rises by at least 2 ms; `wallet list`'s by less than 1
  ms. *Result:* page 69.81 → 70.47 ms (+0.66), `wallet list` 2.84 → 3.76 ms
  (+0.92). *Verdict:* refuted.
- *Timelines* (`timeline/`, five starts of each under `strace -f -tt -T`):
  both builds' headless loops sleep 46 times (188 ms), so the page settles
  at the same iteration. The after build makes about 215 more calls:
  `statx` +39, `openat` +37, `close` +32, `getdents64` +24, `getpid` +22,
  `read` +19, `fstat` +12, `getppid` +8, `sendto` +8, `fsync` +5 and the
  lock's. The `getppid` and `sendto` calls are pgmcp's file-tracing shim
  (below).
- *H-sys:* the page's extra is kernel time and waiting from those calls.
  *Test* (`rusage.py`, 30 alternating starts, `getrusage`). *Result:* the
  kernel charges CPU time in whole ticks (`wallet list` before: user 0.00,
  system 4.01 ms), so the split cannot decide it. *Verdict:* untested by
  this method. But the same alternating runs gave a page $`\Delta`$ of
  +1.66 ms, not +3.58.
- *H-drift:* hyperfine's +3.58 ms holds drift between its two blocks, each
  build's 50 starts back to back, about 15 s apart on a loaded machine.
  *Test* (`pairs.py`, 100 pairs per command, the order alternating).
  *Prediction:* the median of the pairs' differences +1.0 to +2.5 ms for
  the page, Wilcoxon signed-rank p < 0.001 \[Wilcoxon-1945\]; +1.0 to +2.0 ms
  for `wallet list`. *Result:* page +1.64 ms (IQR −0.45 to +3.20), p =
  2.3e-6; `wallet list` +1.35 ms (IQR +0.99 to +1.75), p = 3.9e-18; which
  build went first moved the difference by 0.2–0.3 ms. *Verdict:*
  confirmed. The page's steady cost is the start-up's.

**The method changed.** `scripts/storage-bench.sh` now also measures every
before / after comparison in alternating pairs
(`scripts/lib/storage-bench-pairs.py`: 30 pairs for a page and 100 for
`wallet list`, after 3 not kept; each start prepared as hyperfine prepares
it, the preparation untimed), and `table.md` reports the pairs' median
difference, interquartile range and Wilcoxon p beside hyperfine's
comparison. Its usage now prints the header up to its first empty line.

**The run with pairs** (`s15-full-2/`, load about 15). *Prediction:* P1
again; pairs: `wallet list` steady +1.0 to +2.0 ms, fresh / old-3 /
old-full within 2 ms of the first run's +5.63 / +7.34 / +8.92 and in that
order; the page within 2.5 ms of `wallet list` for each profile and +1.0
to +2.5 ms when steady; every p < 0.001. *Result:* P1 0 of 253 cells
differ.

| Profile | page: median difference (ms) | `wallet list`: median difference (ms) |
|---|---|---|
| `steady` | +1.56 (IQR +0.91 to +2.65) | +1.29 (+1.07 to +1.54) |
| `fresh` | +5.46 (+3.95 to +6.82) | +5.76 (+5.41 to +5.95) |
| `old-3` | +7.56 (+6.27 to +9.59) | +7.08 (+6.78 to +7.41) |
| `old-full` | +8.86 (+7.45 to +10.29) | +8.20 (+7.79 to +8.50) |

Every p was below 1e-6. *Verdict:* confirmed. hyperfine's blocks in the
same run gave the page steady +8.15 ms (p = 0.078), old-3 +34.84 ms and
old-full −4.92 ms: H-drift, larger under more load.

**V1, the holder's record without fsyncs.** Of a steady start's five
fsyncs, four make the `instance.pid` files durable, though the hold they
record ends with any crash. *Experiment* (`v1/`): `lock.rs` edited to
write each sidecar to a temporary file and rename it without syncing,
built into its own target directory (`33c80688…`), the file restored and
compared byte for byte. *Prediction:* a steady start makes 1 fsync, 2
renames and 5 opens to write; in 100 pairs of `wallet list`, V1 less the
current build −0.6 to −0.1 ms (p < 0.001), and 30 pairs of the page agree
within 0.5 ms. *Result:* 1, 2 and 5; `wallet list` −0.353 ms (IQR −0.824
to +0.079), p = 1.1e-4; the page −2.62 ms (IQR −43.7 to +2.4), p = 0.031,
measured while the load reached 35. *Verdict:* confirmed for `wallet
list`; the page inconclusive under that load. V1 is not adopted: the
sidecars' durability is part of the approved lock design, so the choice
is among the questions of S16.

**The environment.** The session's settings set
`LD_PRELOAD=libpgmcp_fstrace.so` (`PGMCP_HOOK_MODE=enforce`) for every
process run here, so every measurement of this ledger ran under it. It
wraps `open`, `openat`, `creat`, `fopen`, `link`, `mkdir`, `rename`,
`symlink`, `truncate` and `unlink` in both builds, and reports some calls
to its socket. In a steady page start of the after build, its own
`getpid`, `getppid` and `sendto` calls took 0.34 ms under strace; the
before build reported nothing. It was not bypassed: it is the user's
enforcement hook.

**Checks.** `shellcheck`: `storage-bench.sh` clean, and every script of
this entry clean. `storage-bench.sh --quick` ran end to end with the pairs
(`s15-quick-2/`).

**Not verifiable on this machine.** Costs on ext4 with barriers (the
default), on APFS (macOS's `F_FULLFSYNC`) and on NTFS (`FlushFileBuffers`),
where each fsync asks the drive to empty its cache, and which this
machine's mount does not do. The counts of P1 hold everywhere that runs
the same code; the times are this machine's.

**Open.** V1 (above), with the questions of S16.

---

## S16 — The move of this machine's profile, rehearsed on a copy

**Goal.** Plan §5.7: before the real move (S17, only on the user's
go-ahead), run it on a copy of `~/.local/share/f1r3gaze` with the release
binary, the way a real start would find and move it, and prove the
original untouched. Evidence in `target/scratch/storage/s16/` (the
predictions in `prediction.md`, written before the first run; the script
`dry-run.sh`; each run's checks and traces in `run*/evidence/`).

**The original** (only read): `settings.conf`, 433 bytes, the `f6ee26a`
template; `user-id`, 32 bytes; `workspace.json`, 675 bytes, with the theme
`dark`, the sidebar closed, the panel `appearance`, tree tabs off, one tab
and four visits. No folder of the new layout existed: no build had moved
it. No system folder holds an `f1r3fly-io` folder.

**The environment.** The user's session sets `XDG_RUNTIME_DIR`,
`XDG_CONFIG_DIRS` and `XDG_DATA_DIRS`, and none of the four `XDG_*_HOME`
variables, so every root derives from HOME. The rehearsal did the same:
HOME was the run's `home/`, the four variables unset, and the runtime and
system folders the run's own; no session bus, the fake `dbus-send`. Before
any start, `paths` had to name only the run's folder (it did: config, data,
state and cache under `home/`, the old profile at
`home/.local/share/f1r3gaze`).

**Steps and checks** (`dry-run.sh`):
1. A manifest of the original (each entry's name, type, size, mode,
   modification time, inode and sha256), then `cp -a` into `home/`; the
   copy's manifest equal but for the inodes.
2. `profile check` under strace: exit 1 (a start would move the profile),
   and no write-like call on the profile's roots or on the original.
3. The first start (`--headless gaze://newtab --timeout 5`, no
   `--profile`): exit 0, no write-like call on the original. The id kept
   byte for byte; `settings.toml` byte for byte the template; the session
   (1 tab, active 0, sidebar closed, panel `appearance`, tree tabs off) and
   the tab's address as in the workspace; the four visits with their
   addresses, titles and times; both originals in `backups/`; the old
   folder holding only `MIGRATED.txt`, which names no address; the marker
   present and the plan gone; no conflict. Its notices: the theme note and
   the move.
4. `profile check` after the move: exit 0.
5. The window, under Xvfb and inside a network namespace with only a
   downed loopback (`unshare --user --net --map-current-user`), so that the
   restored https tab could not reach the network: it opened at 1280×800
   at the screen's corner on the moved session, its tab showing "Can't open
   this page" (the name could not be resolved); closed with
   `WM_DELETE_WINDOW`, it exited 0 and saved 1280×800 in `window.json`,
   the session still one tab.
6. A start after that: exactly a steady start's writes (open to write 5,
   write 4, fsync 5, rename 2, unlink 1, chmod 1, ftruncate 2, flock 2;
   S15), and no other file's sha256, size, mode or time changed.
7. The original's manifest at the end: identical, inodes and times
   included.

**Runs.**
- *Run 1* (`run-1-vacuous-counts/`): 26 of 28 checks passed. The two
  failures were the checks' own. `window_saved` compared text, and
  `window.json` holds `1280.0`, a float. And the strace runs lacked `-tt`,
  without which the bench table's parser reads no line, so every count was
  0: the positive control (`last_writes`) failed, and the "no write" checks
  before it had passed vacuously; their verdicts are void. Changes: strace
  with `-tt -T`; the counter says "no calls parsed" when it parses nothing;
  the size compared as numbers. The original's manifest check, which needs
  no strace, passed.
- *Run 2* (`run-2-output-counted/`): 26 of 28. The two failures were the
  write column alone (40 and 9 where 0 and 4 were predicted): the commands'
  own output, redirected into `evidence/`, inside the folder the counts
  used. Counting run 2's trace again on the profile's roots gave exactly
  the steady start's numbers. Change: the counts name `home/` and the
  runtime folder only.
- *Run 3* (`run/`): 28 of 28 checks passed.

**Verdict.** Every prediction held: the real move, run on a copy, does
what sections 10.1–10.6 say, and leaves the original untouched.

**Questions held for this step** (the plan's §5.7 and the entries before),
asked together, the answers awaited before anything else:
1. macOS and Windows machines for the manual checks (S12 part 3, S13
   part 2).
2. Q1: may openbox be installed (`sudo pacman -S openbox`), so that scenes
   98 and 99 run?
3. May the committed captures in `docs/screenshots/ui/after/` be
   regenerated?
4. Should the Blitz findings be filed upstream after searching its issues
   and pull requests: a zoom rendered differently by the zoom a window
   started at (S13 part 2, E3); no `<meta name="color-scheme">`; no Canvas
   fill behind a page (S12 part 3)?
5. V1 (S15 part 2): write the `instance.pid` files atomically without
   syncing them?
6. The KDE Wayland checks of the plan's §5.7 on the user's desktop: the
   theme following the desktop's scheme (which changes the desktop's
   setting, restored after), F11, maximize and the size restored, and a
   second instance.
7. S17: may the real profile be moved now, after a tarball of
   `~/.local/share/f1r3gaze`?

**The answers** (2026-10-06): 1, both, so a checklist for each system,
which the user runs; 2, the user installed openbox (and asked how it
differs from Xvfb: Xvfb is the X server, a screen in memory; openbox a
window manager, the client that carries out maximize and full screen,
which nothing answers on Xvfb alone); 3, regenerate; 4, search, then file
what is new (filed as DioxusLabs/blitz#1076, #1077 and #1078; chrome ledger
L18); 5, adopt V1 (part 3 of S15); 6, all the desktop checks,
the scheme flipped and restored; 7, move it, with a tarball first.

---

## S15 (part 3) — V1: who holds the lock, written without fsyncs

**Decision.** Asked with the questions of S16, the user chose V1 on
2026-10-06: write each `instance.pid` atomically but without syncing it.
Evidence in `target/scratch/storage/v1/` (the predictions in
`prediction.md`, written before the builds and runs).

**Built.**
- gaze-fs: `replace_unsynced(fs, path, bytes, perm)`. It resolves links,
  makes a temporary file with the final mode, and renames it over `path`,
  syncing nothing; on an error it removes the temporary file. Exported from
  the crate's root.
- `lock.rs`: the holder's record beside each lock is written with it; the
  old call, `write_atomic`, is commented out with its reason.
- Docs: storage README §7.2 (⟨Replace for readers⟩), §8 (how the record is
  written), §15.2 (the kill references), §15.3 (the spec), §15.4 (the
  counts and V1's cost); the diagrams `steady-start-writes` and
  `instance-lock-sequence`.

**Tests, red first.**
- `the_holder_record_is_replaced_without_syncing` (lock): on a profile
  whose folders exist, taking the lock syncs nothing, and each record is
  published by a rename. Before the change: red, 2 `sync_file` and 2
  `sync_dir`.
- `replace_unsynced_is_atomic_for_readers` (gaze-fs): under every crash
  point, POSIX and NTFS, a process crash leaves the old content or the
  new, whole; a power cut leaves those or `GARBAGE`, never no file.
- `replace_unsynced_syncs_nothing` (gaze-fs): the trace has no sync, the
  rename is there, the content and the mode 0600 are the new ones.
- The workspace: 501 tests (498 and these three), 0 failed, on stable and
  on 1.95; `-p gaze-shell --features frame-times` 373; clippy with
  `-D warnings` clean in the five configurations; `cargo doc` no warning
  (`matrix/`).
  `storage-trace.sh --quick`: every trace accepted, both mutants rejected.
  `storage-kill.sh --quick`: 40 real kills, each a behaviour of the model,
  nothing lost.

**Mutation checks** (`s15-v1.json`; `mutations/`). All 3 went red, and
every file was restored byte for byte:

| ID | Mutation | Test that went red |
|---|---|---|
| M15a | the record synced again (`write_atomic`) | `the_holder_record_is_replaced_without_syncing` |
| M15b | `replace_unsynced` syncs its temporary file | `replace_unsynced_syncs_nothing` |
| M15c | `replace_unsynced` removes the file, then writes it in place | `replace_unsynced_is_atomic_for_readers` |

**The kill references.** README §15.2 said the reference runs make 341 and
433 operations before `profile opened`; this run counted 335 and 427.
*Experiment:* the same quick run with only the old write restored in
`lock.rs` (then restored and compared byte for byte): 339 and 431, so V1
removed exactly the 4 syncs. The other 2 came from where the profile was:
a diff of S14's reference trace against this one is entirely the barrier's
lookups and syncs of the folders above the profile (`kind` and `sync_dir`
for each), and S14's run was one folder deeper. The README now gives the
count with that dependence.

**Re-derivation** (`derivation/`, the `storage-trace` build of the V1
source, `3cbf242d…`). *Prediction:* S15 part 2's counts less exactly 4
fsyncs for every start that takes the lock, all else unchanged. *Result:*
steady and moved fsync 1, fresh 45, old-3 62, old-full 80; every other
cell unchanged; `profile check` 0. *Verdict:* confirmed.

**The full run with pairs** (`target/scratch/storage/s15-v1/`, release
`7eb838af…`; the load 48 at the start and 19 at the end, with other builds,
an indexer and a backup running). *P1:* 0 of 253 cells differ from the
re-derivation: confirmed. *Pairs* (median of the differences, after less
before; every Wilcoxon p below 2e-3):

| Profile | page (ms) | `wallet list` (ms) |
|---|---|---|
| `steady` | +4.53 (IQR +1.45 to +18.81) | +1.00 (+0.80 to +1.29) |
| `fresh` | +6.51 | +6.44 |
| `old-3` | +8.13 | +7.58 |
| `old-full` | +13.05 | +8.50 |

*Prediction:* `wallet list` steady +0.5 to +1.3 ms: confirmed (+1.00,
against part 2's +1.29: V1's −0.35 ms). Fresh, old-3 and old-full 0 to 1 ms
below part 2's: refuted, 0.30 to 0.68 ms above. The prediction compared two
runs under different loads (15 and 48), and the load moves every start's
cost: the right measure of V1 is the pair of builds measured together
(part 2, V1: −0.353 ms). The page's pairs were too noisy at this load to
say more (an IQR of 17 ms when steady).

---

## S13 (part 3) — Full screen and maximize under a window manager, and the captures regenerated

**Why.** Scenes 98 and 99 need a window manager, which nothing on this
machine had (question Q1 of S13 part 2). The user installed openbox
(Openbox 3.6.1) on 2026-10-06, and asked that the committed captures be
regenerated. Evidence in `target/scratch/storage/s13-wm/` (predictions in
`prediction.md`, each written before its run) and
`target/scratch/storage/final-v1/`.

**What openbox adds.** Xvfb is the X server: a screen drawn into memory.
openbox is a window manager, a separate client on that screen, which places
windows, frames them, and carries out what a window asks of it. On X11 an
application does not maximize or go full screen by itself: it sends
`_NET_WM_STATE` messages to the root window, which only a window manager
answers. `start_wm` runs openbox on the harness's screen with no key or
mouse bindings, so it cannot take the harness's keys.

**First run** (`run-1/`, scenes 19, 98 and 99, binary `7eb838af…`).
*Prediction:* all 15 checks of 98 and 99 pass, `fullscreen_px_vs_19` 0.
*Result:* 15 of 15. *Verdict:* confirmed.

**Red run** (`red/`, the step-10 binary `f48811d8…`, which keeps no window
state and has no F11). *Prediction:* the full-screen checks, the restored
states and the saved size fail; 99's `maximized_state` and
`maximized_geometry`, the window manager's own doing, may pass. *Result:* 12
failed, as predicted, and those two passed; but `no_creep` passed too. It
reads `window.json`, and a binary that never writes it leaves the seeded
place, so there it passes vacuously.

**H-creep, a hand mutation** (built into its own target directory,
`window_state.rs` restored and compared byte for byte). `Sample::from_platform`
saves the surface's corner (the frame's corner plus the frame's inset) as
the window's place. *Prediction:* `no_creep` and
`normal_kept_while_maximized` fail, the place saved being moved by
openbox's frame. *Result:* both failed, and with them the two checks that
then use the moved place (`restored_then_left_place`,
`unmaximized_place`); the other 11 passed. *Verdict:* confirmed: the
scenes catch a place that creeps by the frame.

**The captures regenerated** (`docs/screenshots/ui/after/`, the harness's
own output; `final-v1/prediction.md`). *Prediction:* 99 scenes, 0 failed;
144 checks pass (E8's 129 and these 15); 01–97 byte-identical to E8's, and
98 and 99 to the first run's. *Result:* 99 scenes, 0 failed, 144 of 144;
97 of 97 identical to E8, and 19, 98 and 99 identical to `run-1/`.
*Verdict:* confirmed. The folder's README and the index
`docs/screenshots/ui/README.md` describe the new set.

**A defect of the harness, fixed.** The run's `run.txt` named the window
manager on two lines, `Openbox 3.6.1` and `none installed`: under
`pipefail`, `openbox --version | head -n1` fails when `head` closes the
pipe early, so the fallback ran as well. The harness now reads the version
without a pipe (`window_manager_version`; the old line commented out with
the reason), and this run's `run.txt` was corrected by hand, its original
kept in `final-v1/run.txt.as-written`.

---

## S17 — This machine's profile moved

**The go-ahead.** The user's answer of 2026-10-06 to question 7 of S16:
move it, with a tarball first. Evidence in `target/scratch/storage/s17/`
(the predictions in `prediction.md`, written before; the script
`move.sh`; checks and traces in `evidence/`).

**How.** The release binary of V1 (`7eb838af…`, the one S16's run 4
rehearsed with, 28 of 28) in the user's own environment: HOME
`/home/dylon`, the four `XDG_*_HOME` unset, `XDG_RUNTIME_DIR`
`/run/user/1000`. `move.sh` stops before moving anything unless no
F1R3Gaze runs, `paths` names exactly the expected config, data, state,
cache, runtime and old folder, none of the four new roots exists, and the
old folder holds `settings.conf`, `user-id` and `workspace.json` and
nothing else. Then:
1. a manifest of the old folder; the tarball
   `~/.local/share/f1r3gaze-before-move-20261006T100446Z.tar.gz` (sha256
   `5ea60534…61f0`, a copy beside the evidence), extracted again and
   compared file by file with the manifest;
2. `profile check`, under strace;
3. the move: `f1r3gaze --headless gaze://newtab --timeout 5`, under
   strace, then the dry run's audits against the tarball's files;
4. `profile check` again;
5. a second start, under strace, with every file listed before and after.

**Result.** 18 of 19 checks passed at once: the tarball held the
originals; the move exited 0; the id kept byte for byte (and its time:
moved, not copied); `settings.toml` the template; the session one tab with
the old tab's address; the four visits; both originals in `backups/`; the
old folder holding only `MIGRATED.txt`, which names no address; the marker
there and the plan gone; no conflict; `profile check` 0 after; the second
start exactly a steady start's writes, no other file changed.

**The one failure, a defect of the count.** `check_before_writes` counted
3 `write` calls on the roots during `profile check`. They were its report,
written to stdout (a file in `evidence/`): strace prints the bytes
written, and the report quotes the roots' paths, which the counting rule
of `scripts/lib/storage-bench-table.py` matched anywhere in a line. In the
rehearsal the scratch paths were long enough to fall past strace's
32-character cut, so it never showed. *Fix:* for `write` and `pwrite64`
only the descriptor's path counts (`DATA_CALLS`, `FD_PATH`; the old test
commented out with the reason). *Check:* the saved trace recounted, 3 with
the old rule, 0 with the new; S15's two analyses unchanged (0 of 253 cells
differ). *Verdict:* `profile check` wrote nothing; the move is complete.

**Where things are now.** `~/.config/f1r3fly-io/f1r3gaze`
(`settings.toml`, its example, `themes/`, `backups/`),
`~/.local/share/f1r3fly-io/f1r3gaze` (the id, the marker, the lock files,
the data folders), `~/.local/state/f1r3fly-io/f1r3gaze` (`session.json`,
`history.json`, `window.json`, `backups/`), `~/.cache/f1r3fly-io/f1r3gaze`;
`~/.local/share/f1r3gaze` holds `MIGRATED.txt`. The tarball stays in
`~/.local/share/` until the user removes it.

---

## S18 (part 1) — On the user's desktop: KDE Plasma on Wayland

**Goal.** The plan's §5.7 desktop checks, on this machine's own session
(KWin on Wayland; one display, 5120 × 2160 at scale 1; colour scheme
BreezeDark), which the user allowed on 2026-10-06 with the scheme flipped
and restored. Evidence in `target/scratch/storage/kde/` (predictions in
`prediction.md`, each written before its run; `kde-checks.sh`;
`evidence-run-1/`, `evidence-run-2/`, `evidence/`, `evidence-hint/`).

**How.** The release binary of V1 (`7eb838af…`) with a throwaway
`--profile`, whose `paths` named only it. The window is driven through
KWin's scripting interface by the app's process id (activate, resize,
maximize, close), and its state read the same way, the scripts printing to
the journal. F11 is typed with ydotool, only right after KWin has activated
the window; the captures are spectacle's of the active window only. On any
exit the scheme is restored, ydotoold stopped, and the app ended.

| ID | Check | Prediction | Run 3 |
|---|---|---|---|
| K1 | the scheme followed: dark at start; BreezeLight, then a focus gained; BreezeDark again | the page's mean luminance < 0.3, > 0.7, < 0.3 | 0.126, 0.949, 0.126 |
| K2 | F11 in, F11 out | KWin full screen, `window.json` `fullscreen` true; then neither | as predicted; the status bar's hint shown |
| K3 | a 1000 × 700 frame, maximized by KWin, closed, started again, unmaximized | maximized saved with the normal size kept (1000 × 672, the frame less its title bar); restored maximized (the frame is KWin's maximize area, 5120 × 2116); back to 1000 × 672 | as predicted |
| K4 | closed unmaximized, started again | the same frame size, `window.json` not rewritten | 1000 × 700; unchanged |
| K5 | a second instance (`--headless`) while the window is open | exit 1, naming the holder `(window, since …)` | as predicted |

**Runs.**
- *Run 1:* 10 of 19 checks passed; none of the failures was the app's. K1's
  measure read the top 34 rows of spectacle's capture, which are the
  window's shadow margin; and the capture has an alpha channel, which
  ImageMagick's mean includes. The page's mean, without alpha, was 0.126,
  0.949 and 0.126. K2's first F11 was lost (H-ydo: ydotool's first key after
  its daemon starts), the second entered full screen, so K3 and K4 then
  tested a full-screen window.
- *Run 2*, with the page measured without alpha and a Shift press first:
  18 of 19. The failure was the script's: its helper numbered KWin scripts
  with a counter that never advanced inside `$(...)`, so a script read an
  earlier one's lines under the same name (both said full screen).
- *Run 3*, each script named by the time in nanoseconds: 19 of 19.
- *The hint.* Run 3's crop of the status bar's left end showed "Running
  f1r3lang", and so did a capture 851 ms after F11 (H-hint-late, that the
  hint had expired, refuted); the bar puts the stage there and the flash at
  its right end, where both captures show "Press F11 to leave full screen".

**Verdict.** On KDE Plasma's Wayland session the chrome follows the
desktop's scheme on a focus it gains, F11 and its hint work, maximize is
kept and restored, the size is kept (Wayland gives no position), and a
second instance is refused. The scheme was BreezeDark at the end of every
run.
