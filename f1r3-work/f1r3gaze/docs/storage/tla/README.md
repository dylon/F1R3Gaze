# The start-up protocol in TLA+

`ProfileStartup.tla` specifies what F1R3Gaze does to its files when it starts:
- the barrier, which syncs the folders it writes into, run only after a
  start that did not finish (the busy flag) or when there is no marker yet;
- the migration of an old single-folder profile, as the plan's actions:
  `write` (a converted output), `move` (an item), `preserve` (an original
  moved into the backups);
- the marker that records the migration done;
- step 5's sweep of a dead start's temporary marker (`prepare`);
- the repair of managed files: a durable copy into the backups, then the
  repair renamed over the file.

TLC checks that no crash, at any step and under either kind of crash, loses or
overwrites data. The design this models is in [../README.md](../README.md),
sections 7 and 9. The results are in the [ledger](../ledger.md), entries S2,
S4 and S6 (part 2).

## Files

| File | What it is |
|---|---|
| `ProfileStartup.tla` | the specification: the file system, the protocol, the crashes, the invariants |
| `MCProfileStartup.tla` | the model values TLC runs it with: which items there are, which move across file systems, which are converted, which files are managed, and the folder of each |
| `MCProfileStartup.cfg` | **deep**: two crashes, power loss, every invariant |
| `MCProfileStartupWide.cfg` | **wide**: four legacy items, one crash: items are independent |
| `MCProfileStartupLive.cfg` | **live**: one crash, every invariant and `Termination` |
| `MCProfileStartupNtfs.cfg` | **ntfs**: the deep run under Windows semantics (W1, W2); every invariant but `ReadyIsDurable`, and `Termination` |
| `MCProfileStartupOrder.cfg` | **order**: the same actions in another order |
| `MCProfileStartupBadCopy.cfg` | **badcopy**: copies to another volume can come out wrong |
| `ProfileStartupTrace.tla` | trace validation: a real start's operations, put into the model's terms, are accepted iff the model can do them (ledger S8) |
| `history/s2-s4/` | the model and runner of ledger entries S2 to S4, before the revision of S2 part 3 |

## The model

**The file system.** It follows the persistence properties of Pillai et al.
(OSDI 2014) in the form of assumptions A1–A3 (the design document, section
7.1):

| Variable | What it holds |
|---|---|
| `ns`, `dns` | which file each name refers to, as processes see it and as a power cut would leave it |
| `data`, `ddata` | each file's content, seen and durable |
| `pending` | directory operations every process sees but no power cut is sure to keep |

**The program.**

| Variable | What it holds |
|---|---|
| `pc`, `pos`, `st` | where start-up is: the phase (`start`, `migrate`, `marker`, `prepare`, `reconcile`, `finish`, `ready`, or `halted` after a failed copy), the action or file, the step |
| `act` | the last file operation, when `TraceMode` is on; otherwise always `NoAct` |
| `run` | how many starts there have been (crashes are bounded by `MaxCrashes`) |
| `taken`, `start` | ghost variables: which names were taken before the migration, and how each managed file started (missing, good or bad) |

**The two crashes.**

| Action | What survives |
|---|---|
| `ProcessCrash` | the kernel's view, whole |
| `PowerCut` | the durable view, plus the pending operations `Keeps` allows: any subset (`Ntfs = FALSE`), or a per-volume prefix (`Ntfs = TRUE`) |

**The constants.**

| Constant | Meaning |
|---|---|
| `Order` | the plan's actions, in order: `<<"write", o>>`, `<<"move", i>>`, `<<"preserve", i>>` |
| `SrcDir`, `DstDir`, `BakDir` | the folder each item, output or managed file is in, goes to, or is backed up in |
| `MarkerDir` | the folder of the marker and of the busy flag (data) |

**The folders.** Each modelled file sits in its class's folder, as in the
code's layout: `so` (settings.toml) in `config`; `u` (user-id) and the
marker in `data`; `ss`, `sh` and the managed file `m` (session.json) in
`state`; the key `k` in `far`, another file system; `s`'s original in
`config-backups`, `w`'s in `far`, and `m`'s copy in `state-backups`. Until
ledger S6 part 2, every file but `k` shared one folder with the marker. The
sync that ends start-up then synced `m`'s folder too, so the model could not
show what the busy flag does after a crash in the repair.
| `FarDirs` | the folders on another volume, which items reach by a verified copy |
| `Checked` | the managed files start-up repairs |
| `BadCopies` | whether a copy can come out wrong |
| `TraceMode` | whether the ghost variable `act` records each file operation (trace validation, ledger S8) |
| `MaxCrashes` | the most crashes in one behaviour |
| `PowerLoss` | whether power cuts happen at all |
| `Ntfs` | Windows semantics |
| `Fault` | `"none"`, or a deliberate defect (below) |

## The invariants

| Invariant | Meaning |
|---|---|
| `TypeOK` | the variables hold what they should |
| `NoLoss` | every legacy item has its original content at its old name, its new name, or its backup |
| `NoOverwrite` | a destination that was taken still holds what it held |
| `CorruptKept` | a managed file that started damaged has its content kept until it is repaired |
| `ValidUntouched` | a managed file that started whole is never changed |
| `PublishedComplete` | a published name always has the whole content |
| `MarkerImpliesComplete` | the migration marker exists only once every item has moved |
| `NeverAbsent` | a managed file that existed is never absent, after any crash |
| `RepairInPlace` | a damaged managed file holds its original or its repair, never only a backup |
| `ReadyIsValid` | at ready, the migration is complete, every managed file is whole (a damaged one holds its repair), and the busy flag is gone |
| `ReadyIsDurable` | at ready, nothing is pending, so the next start may skip the barrier |
| `Termination` | with finitely many crashes, start-up always reaches ready |

## The faults

Each fault puts one defect into the protocol. TLC must exit with status 12
and name the invariant the defect breaks.

| Fault | The defect | The invariant broken |
|---|---|---|
| `regenerate-before-backup` | a damaged file replaced before its backup | `CorruptKept` |
| `replace-destination` | a move that replaces a taken destination | `NoOverwrite` |
| `marker-first` | the marker written before the items move | `MarkerImpliesComplete` |
| `no-fsync-before-publish` | a file published before its data is synced | `PublishedComplete` |
| `no-dir-fsync-before-unlink` | a source unlinked before its copy's folder is synced | `NoLoss` |
| `resume-stale-temp` | a crashed copy's temporary file resumed as if whole | `PublishedComplete` |
| `no-start-barrier` | no barrier at start | `NoLoss` |
| `settle-only` | only the copy found is synced, not the whole barrier | `ReadyIsDurable` (and `MarkerImpliesComplete`; see below) |
| `no-commit-name` (`Ntfs`) | on Windows, a name not committed by syncing its file | `NoLoss` |
| `no-busy-flag` | the busy flag never set, so the barrier runs only while there is no marker | `ReadyIsDurable`: a repair's operation in `state` is still pending at ready (before S6 part 2, without the marker's condition: `NoLoss`) |
| `no-backup-fsync` | a damaged file's copy not synced before its repair | `CorruptKept` |
| `no-backup-dir-fsync` | the copy's name not synced before the repair | `CorruptKept` (with `ReadyIsDurable` left out) |
| `move-then-write` | the old repair order: moved into the backups, then the repair written | `NeverAbsent`; `RepairInPlace` and, after a crash, `ReadyIsValid` on their own |
| `no-verify` (`BadCopies`) | a copy published without reading it back | `PublishedComplete` |

`settle-only` breaks two invariants at the same depth. Which one TLC reports
first depends on how its workers are scheduled: 1 to 4 workers reported
`MarkerImpliesComplete`, 8 and 12 workers mostly `ReadyIsDurable` (ledger S4,
part 2). So each is checked on its own, with the other left out of the
configuration:
- `fault-settle-only` leaves `MarkerImpliesComplete` out;
- `hazard-settle-only-marker` leaves `ReadyIsDurable` out.

**More hazards and controls.** The first three rows show why the deep
configuration needs power loss and two crashes. The others show what a
defect does not break, so that no fault's verdict is read as more than it
says:

| Run | Result | What it shows |
|---|---|---|
| `hazard-no-barrier-one-crash-pending` | no barrier, one crash: breaks `ReadyIsDurable` | one crash suffices to leave operations pending at ready |
| `control-no-barrier-one-crash-data-safe` | no barrier, one crash, without `ReadyIsDurable`: passes | losing data takes two crashes |
| `control-no-fsync-without-power-loss` | a missing fsync, no power cuts: passes | process crashes alone cannot see a missing fsync, so tests that only kill processes would miss it |
| `control-move-then-write-no-crash` | the old repair order, no crashes: passes | the order only matters under crashes |
| `control-ntfs-no-backup-dir-fsync` | no folder sync of the copy, under NTFS: passes | syncing the copy commits its name (W2) |
| `control-no-marker-barrier` | the barrier only after a busy flag, not when there is no marker: passes | the marker's condition guards the creation of the data root, before the flag can be set; the model's folders always exist, so here it only adds barriers that find nothing pending. The code's test `first_launch_survives_every_crash` shows what it guards |

## Checking real start-ups (ledger S8)

The model is checked against the code, not only by reading both: real
start-ups are recorded and replayed in the model (trace validation, after
Cirstea, Kuppe, Loillier and Merz, SEFM 2024).

1. **Recording.** `crates/gaze-shell/tests/storage_trace.rs` runs start-up
   (`profile::start_up`, the one copy of `main.rs`'s order) on an in-memory
   file system (`MemFs`) through a
   recording one (`TraceFs`), with the crashes and power cuts each case
   calls for. Each simulated restart is another process
   (`gaze_fs::with_process_id`), so its predecessor's temporary files are
   another process's, as after a real restart.
2. **Converting.** `crates/gaze-shell/src/profile/migrate/trace.rs` maps
   each concrete name to the model's (`Src`, `Dst`, `Tmp`, `PBak`,
   `BakTmp`, `Marker`, `MarkerTmp`, `Busy`) and each operation to an event:
   `create` (with the content a fingerprint identifies), `write`,
   `fsync-file`, `fsync-dir`, `link`, `rename`, `unlink`, one `barrier`
   with the folders it synced, `crash-process` and `crash-power`. A power
   cut's kept operations become indices into the model's queue, which holds
   the modelled operations only, in order. Names the model does not have
   (folders made, the plan, `MIGRATED.txt`, the cache, start-up's repair)
   are left out; a sync of a modelled folder the protocol does not need
   there is an *eager* sync, which the model allows anywhere. The result is
   `Trace_<case>.tla` and `.cfg`.
3. **Checking.** TLC explores only the model steps that match the next
   event, plus steps that touch no file. It reports `TraceEnd` violated
   exactly when every event was matched: the trace is accepted. A rejected
   trace is reported with how many of its events the model could follow
   (a binary search with probe invariants `l <= n`) and the first it could
   not.

| Cases | What happens |
|---|---|
| `users_profile`, `full` | a whole start: this machine's profile's shape, and every kind of item |
| `cross_device`, `ntfs` | the old profile on another volume; Windows semantics |
| `conflict` | a destination already taken |
| `bad_copy` | a copy that comes out wrong: the move stops, the next start finishes it |
| `killed_one_<k>`, `killed_two_<k>` | a process killed after every 5th operation (every 25th with `--quick`), one volume or two, then a start that finishes |
| `killed_after_marker_temp`, `killed_after_marker_link`, `killed_after_copy` | a crash in the windows the sweeps and the copy's check are for |
| `power_<none|all|alternate>_<k>` | a power cut a quarter, half and three quarters through, keeping none, all, or every other pending operation |
| `mutant_unsynced`, `mutant_swapped` | a move with its folder sync removed, or its link and unlink swapped: must be **rejected** |

The first full run found that the model had no step 5 sweep: a start that
died between linking the marker and unlinking its temporary file is
followed by one that finds the marker, and whose step 5 removes the
temporary file, which the model could not do. `PrepareStep` was added
(ledger S8); every verdict of the runs below is unchanged.

```text
scripts/storage-trace.sh            # 171 traces: a crash at every 5th operation
scripts/storage-trace.sh --quick    # 49 traces (CI)
```

### Real kills (ledger S8, part 2)

`crates/gaze-shell/tests/crash_points.rs` checks the same model against the
real binary on the real file system. Built with `crash-points` and
`storage-trace`, `f1r3gaze` aborts at its k-th storage operation
(`F1R3GAZE_CRASH_AT=k`) and appends every operation it finished to a file
(`F1R3GAZE_STORAGE_TRACE`), as JSON lines (`Traced::json`). A clean start
then finishes. The test appends the crash between the two processes'
records, reads them back (`migrate::trace::events_of_ndjson`), takes the
plan the migration noted (`plan_of`), and converts and checks the trace as
above. Every operation before the note `profile opened` is traced (the
instance lock's too), so a process killed at k has recorded exactly
k − 1; the test asserts it for every kill.

```text
scripts/storage-kill.sh             # 164 kills: every 5th operation, and four windows
scripts/storage-kill.sh --quick     # every 25th (CI, on Linux, macOS and Windows)
```

## Running

`scripts/storage-model.sh` runs everything:
1. it copies this folder into `--out` (so TLC's `states/` folders and trace
   files never land in `docs/`);
2. it runs SANY, then each configuration, fault, hazard and control;
3. it prints `PASS` or `FAIL` per run.

```text
scripts/storage-model.sh              # every run, in a systemd scope with memory limits
scripts/storage-model.sh --quick      # SANY, ntfs, live, the faults, hazards and controls (CI)
scripts/storage-model.sh --workers 4 --heap 6g --no-scope --jar tla2tools.jar
```

**Exit status.** 0 if every run went as predicted; 1 if one did not; 2 if a
tool is missing.

**The jar.** `tla2tools.jar` 1.7.4 (TLC 2.19 of 2024-08-08), sha256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`. CI's
`model` job downloads it and checks that sum.
