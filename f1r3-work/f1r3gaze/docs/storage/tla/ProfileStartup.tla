--------------------------- MODULE ProfileStartup ---------------------------
(***************************************************************************)
(* F1R3Gaze's start-up under crashes: moving a legacy single-folder        *)
(* profile into the new layout, then repairing the managed files. The      *)
(* design is in docs/storage/README.md (section 7) and tla/README.md.      *)
(*                                                                         *)
(* The file system is an abstract persistence model in the style of        *)
(* Pillai et al., "All File Systems Are Not Created Equal" (OSDI 2014),    *)
(* with three assumptions:                                                 *)
(*   A1  a directory operation (create, link, unlink, rename) is atomic;   *)
(*   A2  directory operations reach the disk in any subset and any order,  *)
(*       except that fsync of a directory makes every pending operation    *)
(*       that touches that directory durable;                              *)
(*   A3  a file's data is durable only once the file itself is fsynced.    *)
(*                                                                         *)
(* Inodes separate names from data, so a link or a rename carries          *)
(* unsynced data with it. A ProcessCrash keeps everything the kernel       *)
(* holds; a PowerCut keeps only what is durable plus some of the pending   *)
(* directory operations.                                                   *)
(*                                                                         *)
(* With Ntfs, the semantics F1R3Gaze assumes under Windows instead         *)
(* (crates/gaze-fs/src/fs.rs, StdFs):                                      *)
(*   W1  each volume logs its metadata changes in order, so a power cut    *)
(*       keeps a prefix of each volume's pending operations;               *)
(*   W2  syncing a file (FlushFileBuffers) commits its volume's log;       *)
(*   and directories cannot be synced, while every link or rename that     *)
(*   names a file then syncs it.                                           *)
(*                                                                         *)
(* Since S2-S4 (history/s2-s4/): a damaged managed file is copied into the *)
(* backups and then replaced in place, never moved away; the barrier runs  *)
(* only after a start that did not finish (the busy flag); the migration  *)
(* is the plan's list of actions; and act records each file operation for *)
(* trace validation (ledger S8).                                           *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Order,       \* the migration plan's actions, in order:
                 \*   <<"write", o>>     write the converted output o
                 \*   <<"move", i>>      move item i to its new place
                 \*   <<"preserve", i>>  move item i's original into the backups
    SrcDir,      \* item -> the folder it is in (the legacy profile)
    DstDir,      \* moved item, output or managed file -> the folder of its name
    BakDir,      \* preserved item or managed file -> the folder of its backup
    MarkerDir,   \* the folder of the marker and of the busy flag (data)
    FarDirs,     \* the folders on another volume
    Checked,     \* the managed files reconcile visits, in order
    MaxCrashes,  \* how many crashes one behaviour may contain
    PowerLoss,   \* whether a crash may lose what is not yet durable
    Ntfs,        \* Windows semantics (W1, W2) instead of A2
    BadCopies,   \* whether a copy to another volume can come out wrong
    TraceMode,   \* whether act records each file operation (ledger S8)
    Fault        \* "none", or a deliberate defect: the model's mutation checks

Acts    == {Order[j] : j \in 1..Len(Order)}
Outputs == {a[2] : a \in {b \in Acts : b[1] = "write"}}
Moved   == {a[2] : a \in {b \in Acts : b[1] = "move"}}
Kept    == {a[2] : a \in {b \in Acts : b[1] = "preserve"}}
Items   == Moved \cup Kept
Managed == {Checked[j] : j \in 1..Len(Checked)}
Named   == Moved \cup Outputs \cup Managed
Runs    == 1..(MaxCrashes + 1)
Faults  == {"none", "regenerate-before-backup", "replace-destination",
            "marker-first", "no-fsync-before-publish",
            "no-dir-fsync-before-unlink", "resume-stale-temp",
            "no-start-barrier", "settle-only", "no-commit-name",
            "move-then-write", "no-backup-fsync", "no-backup-dir-fsync",
            "no-verify", "no-busy-flag", "no-marker-barrier"}

ASSUME /\ Len(Order) = Cardinality(Acts)
       /\ \A a \in Acts : a[1] \in {"write", "move", "preserve"}
       /\ Moved \cap Kept = {}
       /\ Outputs \cap Items = {}
       /\ Managed \cap (Items \cup Outputs) = {}
       /\ \A i \in Moved : SrcDir[i] # DstDir[i]
       /\ \A i \in Kept : SrcDir[i] # BakDir[i]
       /\ MaxCrashes \in Nat
       /\ PowerLoss \in BOOLEAN
       /\ Ntfs \in BOOLEAN
       /\ BadCopies \in BOOLEAN
       /\ TraceMode \in BOOLEAN
       /\ Fault \in Faults
-----------------------------------------------------------------------------
(***************************************************************************)
(* Paths are <<folder, name>>, and every name is a tuple, so TLC never     *)
(* compares a string with a tuple.                                         *)
(***************************************************************************)
Src(i)    == <<SrcDir[i], <<"item", i>>>>
Dst(x)    == <<DstDir[x], <<"item", x>>>>          \* also managed file x
Tmp(x)    == <<DstDir[x], <<"tmp", x>>>>
PBak(i)   == <<BakDir[i], <<"preserved", i>>>>     \* a preserved original
BakTmp(i) == <<BakDir[i], <<"tmp-preserved", i>>>>
Bak(m, r) == <<BakDir[m], <<"backup", m, r>>>>     \* backups/<run r>/m
Marker    == <<MarkerDir, <<"marker">>>>           \* data/layout.json
MarkerTmp == <<MarkerDir, <<"marker-tmp">>>>
Busy      == <<MarkerDir, <<"busy">>>>             \* data/.startup-busy
Dir(p)    == p[1]
Vol(d)    == IF d \in FarDirs THEN "B" ELSE "A"

To(a)    == IF a[1] = "preserve" THEN PBak(a[2]) ELSE Dst(a[2])
TmpOf(a) == IF a[1] = "preserve" THEN BakTmp(a[2]) ELSE Tmp(a[2])
Cross(a) == a[1] # "write" /\ Vol(Dir(To(a))) # Vol(Dir(Src(a[2])))

InitialPaths == {Src(i) : i \in Items} \cup {Dst(x) : x \in Named}
TempPaths    == {Tmp(x) : x \in Named} \cup {BakTmp(i) : i \in Kept} \cup {MarkerTmp}
BakPaths     == {Bak(m, r) : m \in Managed, r \in Runs}
Creatable    == TempPaths \cup BakPaths \cup {Busy}
Paths        == InitialPaths \cup Creatable \cup {Marker} \cup {PBak(i) : i \in Kept}

None       == <<"none">>
Garbage    == <<"garbage">>
Note       == <<"note">>
Orig(i)    == <<"orig", i>>
Partial(i) == <<"partial", i>>
Conv(o)    == <<"conv", o>>
Theirs(x)  == <<"theirs", x>>
Good(m)    == <<"good", m>>
Bad(m)     == <<"bad", m>>
Default(m) == <<"default", m>>
Fix(m)     == <<"fix", m>>      \* a damaged file's repair: its usable part, or its default
Contents == {None, Garbage, Note}
            \cup {Orig(i) : i \in Items} \cup {Partial(i) : i \in Items}
            \cup {Conv(o) : o \in Outputs} \cup {Theirs(x) : x \in Moved \cup Outputs}
            \cup {Good(m) : m \in Managed} \cup {Bad(m) : m \in Managed}
            \cup {Default(m) : m \in Managed} \cup {Fix(m) : m \in Managed}

Want(a)     == IF a[1] = "write" THEN Conv(a[2]) ELSE Orig(a[2])
Valid(m, c) == c \in {Good(m), Default(m), Fix(m)}

NoInode == <<"no-inode">>
Inodes  == {<<"pre", p>> : p \in InitialPaths}
           \cup {<<"new", p, r>> : p \in Creatable, r \in Runs}
-----------------------------------------------------------------------------
VARIABLES
    ns,       \* path -> inode, as the program sees the names
    dns,      \* path -> inode, as a power cut would leave the names
    data,     \* inode -> content, as the program sees it
    ddata,    \* inode -> content, as a power cut would leave it
    pending,  \* directory operations not yet durable, oldest first
    pc,       \* phase: start, migrate, marker, prepare, reconcile, finish, ready, halted
    pos,      \* the action or managed file being visited
    st,       \* the step within it
    run,      \* 1 + the crashes so far
    taken,    \* ghost: names that were occupied before the first start
    start,    \* ghost: each managed file's state before the first start
    act       \* ghost: the file operation of the last step (TraceMode only)

vars == <<ns, dns, data, ddata, pending, pc, pos, st, run, taken, start, act>>

C(p)      == IF ns[p] = NoInode THEN None ELSE data[ns[p]]
Exists(p) == ns[p] # NoInode
Fresh(p)  == <<"new", p, run>>

NoPath == <<"no-path">>
NoAct  == [op |-> "none", p |-> NoPath, q |-> NoPath, c |-> None, d |-> ""]
SetAct(r) == act' = IF TraceMode THEN r ELSE NoAct
Did(o, p, q, c, d) == SetAct([op |-> o, p |-> p, q |-> q, c |-> c, d |-> d])

Initial(p) ==
    CASE \E i \in Items : p = Src(i) -> Orig(p[2][2])
      [] \E x \in taken : p = Dst(x) -> Theirs(p[2][2])
      [] \E m \in Managed : p = Dst(m) /\ start[m] = "good" -> Good(p[2][2])
      [] \E m \in Managed : p = Dst(m) /\ start[m] = "bad" -> Bad(p[2][2])
      [] OTHER -> None

Init == /\ taken \in SUBSET (Moved \cup Outputs)
        /\ start \in [Managed -> {"missing", "good", "bad"}]
        /\ ns = [p \in Paths |-> IF p \in InitialPaths /\ Initial(p) # None
                                 THEN <<"pre", p>> ELSE NoInode]
        /\ dns = ns
        /\ data = [n \in Inodes |-> IF n[1] = "pre" THEN Initial(n[2]) ELSE None]
        /\ ddata = data
        /\ pending = << >>
        /\ pc = "start" /\ pos = 1 /\ st = "begin" /\ run = 1
        /\ act = NoAct
-----------------------------------------------------------------------------
(***************************************************************************)
(* The file system.                                                        *)
(***************************************************************************)
Op(us) == [dirs |-> {Dir(u[1]) : u \in us}, upd |-> us]
Apply(names, op) ==
    [p \in Paths |-> IF \E u \in op.upd : u[1] = p
                     THEN (CHOOSE u \in op.upd : u[1] = p)[2]
                     ELSE names[p]]
RECURSIVE ApplyAll(_, _)
ApplyAll(names, ops) ==
    IF ops = << >> THEN names ELSE ApplyAll(Apply(names, Head(ops)), Tail(ops))
RECURSIVE ApplySome(_, _, _)
ApplySome(names, ops, keep) ==
    IF ops = << >> THEN names
    ELSE ApplySome(IF 1 \in keep THEN Apply(names, Head(ops)) ELSE names,
                   Tail(ops), {j - 1 : j \in keep \ {1}})

FsVars == <<ns, dns, data, ddata, pending>>
Keep == UNCHANGED FsVars /\ SetAct(NoAct)
DirOp(us) == /\ ns' = Apply(ns, Op(us))
             /\ pending' = Append(pending, Op(us))

\* The volume a directory operation is on (one never spans two volumes).
VolOf(op) == Vol(CHOOSE d \in op.dirs : TRUE)

\* W2: the pending operations on volume v become durable, in order.
CommitVolume(ops, v) ==
    LET On(op)  == VolOf(op) = v
        Off(op) == VolOf(op) # v
    IN /\ dns' = ApplyAll(dns, SelectSeq(ops, On))
       /\ pending' = SelectSeq(ops, Off)

\* A directory operation that gives inode n the name p. Under Ntfs the file
\* is then synced, which commits its volume (StdFs's commit_name on Windows)
\* unless the fault no-commit-name leaves that out.
Naming(us, n, p) ==
    /\ ns' = Apply(ns, Op(us))
    /\ IF Ntfs /\ Fault # "no-commit-name"
       THEN /\ ddata' = [ddata EXCEPT ![n] = data[n]]
            /\ CommitVolume(Append(pending, Op(us)), Vol(Dir(p)))
       ELSE /\ pending' = Append(pending, Op(us))
            /\ UNCHANGED <<dns, ddata>>

\* Replacing an existing name by a link or a checked rename is what the
\* program never does; one fault does.
MayReplace == Fault = "replace-destination"

Create(p, c) == /\ ~Exists(p)
                /\ data[Fresh(p)] = None          \* one inode per path and run
                /\ DirOp({<<p, Fresh(p)>>})
                /\ data'  = [data  EXCEPT ![Fresh(p)] = c]
                /\ ddata' = [ddata EXCEPT ![Fresh(p)] = Garbage]
                /\ UNCHANGED dns
                /\ Did("create", p, NoPath, c, "")
Write(p, c) == /\ Exists(p)
               /\ data' = [data EXCEPT ![ns[p]] = c]
               /\ UNCHANGED <<ns, dns, ddata, pending>>
               /\ Did("write", p, NoPath, c, "")
FsyncFile(p) == /\ Exists(p)
                /\ ddata' = [ddata EXCEPT ![ns[p]] = data[ns[p]]]
                /\ IF Ntfs THEN CommitVolume(pending, Vol(Dir(p)))
                   ELSE UNCHANGED <<dns, pending>>
                /\ UNCHANGED <<ns, data>>
                /\ Did("fsync-file", p, NoPath, None, "")
\* A directory cannot be synced under Ntfs.
FsyncDir(d) == /\ IF Ntfs THEN UNCHANGED FsVars
                  ELSE LET Touches(op) == d \in op.dirs
                           Other(op)   == d \notin op.dirs
                       IN /\ dns' = ApplyAll(dns, SelectSeq(pending, Touches))
                          /\ pending' = SelectSeq(pending, Other)
                          /\ UNCHANGED <<ns, data, ddata>>
               /\ Did("fsync-dir", NoPath, NoPath, None, d)
\* The start barrier syncs every folder: nothing under Ntfs.
Barrier == /\ IF Ntfs THEN UNCHANGED FsVars
              ELSE /\ dns' = ApplyAll(dns, pending)
                   /\ pending' = << >>
                   /\ UNCHANGED <<ns, data, ddata>>
           /\ Did("barrier", NoPath, NoPath, None, "")
\* link(2): a second name for the same inode; fails if the name exists.
Link(a, b) == /\ Exists(a)
              /\ \/ ~Exists(b)
                 \/ MayReplace
              /\ Naming({<<b, ns[a]>>}, ns[a], b)
              /\ UNCHANGED data
              /\ Did("link", a, b, None, "")
Unlink(p) == /\ Exists(p)
             /\ DirOp({<<p, NoInode>>})
             /\ UNCHANGED <<dns, data, ddata>>
             /\ Did("unlink", p, NoPath, None, "")
\* rename(2) after checking that the target is free (no hard links).
Rename(a, b) == /\ Exists(a)
                /\ \/ ~Exists(b)
                   \/ MayReplace
                /\ Naming({<<a, NoInode>>, <<b, ns[a]>>}, ns[a], b)
                /\ UNCHANGED data
                /\ Did("rename", a, b, None, "")
\* rename(2) over an existing name: write_atomic's last step, used only to
\* repair a managed file after a durable copy of it exists.
RenameOver(a, b) == /\ Exists(a)
                    /\ Naming({<<a, NoInode>>, <<b, ns[a]>>}, ns[a], b)
                    /\ UNCHANGED data
                    /\ Did("rename", a, b, None, "")
-----------------------------------------------------------------------------
(***************************************************************************)
(* The program: one sequential interpreter whose local state (pc, pos, st) *)
(* every crash discards.                                                   *)
(***************************************************************************)
Then(s)  == /\ st' = s
            /\ UNCHANGED <<pc, pos, run, taken, start>>
NextOne  == /\ pos' = pos + 1
            /\ st' = "begin"
            /\ UNCHANGED <<pc, run, taken, start>>
Phase(p) == /\ pc' = p
            /\ pos' = 1
            /\ st' = "begin"
            /\ UNCHANGED <<run, taken, start>>

FirstPhase == IF Exists(Marker) THEN "prepare"
              ELSE IF Fault = "marker-first" THEN "marker"
              ELSE "migrate"

\* Start, with the instance lock held. A start that finds the busy flag
\* follows one that did not finish: the barrier makes durable what it left
\* pending. So does a start that finds no marker: the code creates the data
\* root, and the flag inside it, at its first start, and a start that died
\* before it set the flag leaves none (the model's folders always exist, so
\* here the marker's condition only adds barriers; ledger S6, part 2). A
\* power cut leaves nothing pending, so a start after one needs no barrier,
\* whether or not the flag survived it. Then the flag is set, before
\* anything is written; it is removed only once start-up is done.
StartStep ==
    /\ pc = "start"
    /\ \/ /\ st = "begin"
          /\ IF \/ Fault \in {"no-start-barrier", "settle-only"}
                \/ ~Exists(Busy) /\ (Exists(Marker) \/ Fault = "no-marker-barrier")
             THEN Keep ELSE Barrier
          /\ Then("flag")
       \/ /\ st = "flag"
          /\ IF Exists(Busy) \/ Fault = "no-busy-flag" THEN Keep ELSE Create(Busy, Note)
          /\ Phase(FirstPhase)

Action == Order[pos]

MigrateBegin ==
    /\ pc = "migrate" /\ pos <= Len(Order) /\ st = "begin"
    /\ LET a == Action
           x == a[2]
       IN IF Exists(TmpOf(a))
          THEN IF Fault = "resume-stale-temp"
               THEN Keep /\ Then("publish")
               ELSE Unlink(TmpOf(a)) /\ Then("swept")
          ELSE IF a[1] = "write"
          THEN IF C(Dst(x)) = Conv(x)
               THEN Keep /\ Then(IF Fault = "settle-only" THEN "settle" ELSE "done")
               ELSE IF Exists(Dst(x)) /\ Fault # "replace-destination"
               THEN Keep /\ NextOne                       \* a conflict: both are kept
               ELSE Create(Tmp(x), Conv(x)) /\ Then("fsync")
          ELSE IF ~Exists(Src(x)) THEN Keep /\ NextOne
          ELSE IF C(To(a)) = Orig(x)
          THEN Keep /\ Then(IF Fault = "settle-only" THEN "settle" ELSE "retire")
          ELSE IF Exists(To(a)) /\ Fault # "replace-destination"
          THEN Keep /\ NextOne                            \* a conflict: both are kept
          ELSE IF Cross(a) THEN Create(TmpOf(a), Partial(x)) /\ Then("copy-rest")
          ELSE \/ Link(Src(x), To(a)) /\ Then("sync-new")
               \/ Rename(Src(x), To(a)) /\ Then("sync-new")    \* no hard links

MigrateStep ==
    /\ pc = "migrate" /\ pos <= Len(Order)
    /\ LET a == Action
           x == a[2]
           t == TmpOf(a)
       IN \/ st = "copy-rest" /\ Write(t, C(Src(x))) /\ Then("fsync")
          \/ BadCopies /\ st = "copy-rest" /\ Write(t, Garbage) /\ Then("fsync")
          \/ /\ st = "fsync"
             /\ IF Fault = "no-fsync-before-publish" THEN Keep ELSE FsyncFile(t)
             /\ Then("verify")
          \/ /\ st = "verify" /\ Keep
             /\ Then(IF C(t) = Want(a) \/ Fault = "no-verify" THEN "publish" ELSE "discard")
          \/ st = "discard" /\ Unlink(t) /\ Then("discarded")
          \/ /\ st = "discarded" /\ FsyncDir(Dir(t))
             /\ pc' = "halted" /\ UNCHANGED <<pos, st, run, taken, start>>
          \/ st = "swept" /\ FsyncDir(Dir(t)) /\ Then("begin")
          \/ st = "publish" /\ Link(t, To(a)) /\ Then("sync-published")
          \/ st = "publish" /\ Rename(t, To(a)) /\ Then("sync-renamed")   \* no hard links
          \/ /\ st = "sync-published"
             /\ IF Fault = "no-dir-fsync-before-unlink" THEN Keep ELSE FsyncDir(Dir(To(a)))
             /\ Then("unlink-tmp")
          \/ st = "unlink-tmp" /\ Unlink(t) /\ Then("sync-tmp")
          \/ /\ st \in {"sync-tmp", "sync-renamed"} /\ FsyncDir(Dir(t))
             /\ Then(IF a[1] = "write" THEN "done" ELSE "retire")
          \/ /\ st = "sync-new"
             /\ IF Fault = "no-dir-fsync-before-unlink" THEN Keep ELSE FsyncDir(Dir(To(a)))
             /\ Then("retire")
          \/ st = "settle" /\ FsyncFile(To(a)) /\ Then("settle-dir")
          \/ /\ st = "settle-dir" /\ FsyncDir(Dir(To(a)))
             /\ Then(IF a[1] = "write" THEN "done" ELSE "retire")
          \/ st = "done" /\ Keep /\ NextOne
          \/ /\ st = "retire"
             /\ IF Exists(Src(x)) THEN Unlink(Src(x)) /\ Then("sync-legacy")
                ELSE FsyncDir(Dir(Src(x))) /\ NextOne        \* after a rename
          \/ st = "sync-legacy" /\ FsyncDir(Dir(Src(x))) /\ NextOne

MigrateEnd == /\ pc = "migrate" /\ pos > Len(Order) /\ Keep
              /\ Phase(IF Fault = "marker-first" THEN "prepare" ELSE "marker")

AfterMarker == IF Fault = "marker-first" THEN "migrate" ELSE "prepare"
MarkerStep ==
    /\ pc = "marker"
    /\ \/ st = "begin" /\ Exists(Marker) /\ Keep /\ Phase(AfterMarker)
       \/ /\ st = "begin" /\ ~Exists(Marker) /\ Exists(MarkerTmp)
          /\ Unlink(MarkerTmp) /\ Then("swept")
       \/ st = "swept" /\ FsyncDir(MarkerDir) /\ Then("begin")
       \/ /\ st = "begin" /\ ~Exists(Marker) /\ ~Exists(MarkerTmp)
          /\ Create(MarkerTmp, Note) /\ Then("fsync")
       \/ st = "fsync" /\ FsyncFile(MarkerTmp) /\ Then("publish")
       \/ st = "publish" /\ Link(MarkerTmp, Marker) /\ Then("sync-published")
       \/ st = "publish" /\ Rename(MarkerTmp, Marker) /\ Then("sync")   \* no hard links
       \/ st = "sync-published" /\ FsyncDir(MarkerDir) /\ Then("unlink-tmp")
       \/ st = "unlink-tmp" /\ Unlink(MarkerTmp) /\ Then("sync")
       \/ st = "sync" /\ FsyncDir(MarkerDir) /\ Phase(AfterMarker)

\* Start-up's step 5 sweeps the temporary files of writers that died. Of
\* the modelled names, only the marker's can outlive the migration: a crash
\* between linking the marker and unlinking its temporary file leaves both,
\* and the next start finds the marker and never runs MarkerStep. Every
\* action's temporary file is swept when the action is replayed (ledger S8).
PrepareStep ==
    /\ pc = "prepare"
    /\ \/ st = "begin" /\ Exists(MarkerTmp) /\ Unlink(MarkerTmp) /\ Then("swept")
       \/ st = "swept" /\ FsyncDir(MarkerDir) /\ Then("begin")
       \/ st = "begin" /\ ~Exists(MarkerTmp) /\ Keep /\ Phase("reconcile")

Checking == Checked[pos]

ReconcileBegin ==
    /\ pc = "reconcile" /\ pos <= Len(Checked) /\ st = "begin"
    /\ LET m == Checking IN
       IF Exists(Tmp(m)) THEN Unlink(Tmp(m)) /\ Then("swept")
       ELSE IF ~Exists(Dst(m)) THEN Create(Tmp(m), Default(m)) /\ Then("fsync")
       ELSE IF Valid(m, C(Dst(m))) THEN Keep /\ NextOne
       ELSE IF Fault = "regenerate-before-backup"
       THEN Create(Tmp(m), Fix(m)) /\ Then("repair-fsync")
       ELSE IF Fault = "move-then-write"
       THEN Rename(Dst(m), Bak(m, run)) /\ Then("moved-bak")
       ELSE Create(Bak(m, run), C(Dst(m))) /\ Then("bak-fsync")   \* Backups::copy_into

ReconcileStep ==
    /\ pc = "reconcile" /\ pos <= Len(Checked)
    /\ LET m == Checking IN
       \/ st = "swept" /\ FsyncDir(Dir(Tmp(m))) /\ Then("begin")
       \* A missing file: write_new.
       \/ /\ st = "fsync"
          /\ IF Fault = "no-fsync-before-publish" THEN Keep ELSE FsyncFile(Tmp(m))
          /\ Then("publish")
       \/ st = "publish" /\ Link(Tmp(m), Dst(m)) /\ Then("sync-published")
       \/ st = "publish" /\ Rename(Tmp(m), Dst(m)) /\ Then("sync-new")   \* no hard links
       \/ st = "sync-published" /\ FsyncDir(Dir(Dst(m))) /\ Then("unlink-tmp")
       \/ st = "unlink-tmp" /\ Unlink(Tmp(m)) /\ Then("sync-new")
       \/ st = "sync-new" /\ FsyncDir(Dir(Tmp(m))) /\ NextOne
       \* A damaged file: a durable copy, then the repair renamed over it
       \* (write_atomic).
       \/ /\ st = "bak-fsync"
          /\ IF Fault = "no-backup-fsync" THEN Keep ELSE FsyncFile(Bak(m, run))
          /\ Then("bak-sync-dir")
       \/ /\ st = "bak-sync-dir"
          /\ IF Fault = "no-backup-dir-fsync" THEN Keep ELSE FsyncDir(Dir(Bak(m, run)))
          /\ Then("repair")
       \/ st = "repair" /\ Create(Tmp(m), Fix(m)) /\ Then("repair-fsync")
       \/ /\ st = "repair-fsync"
          /\ IF Fault = "no-fsync-before-publish" THEN Keep ELSE FsyncFile(Tmp(m))
          /\ Then("replace")
       \/ st = "replace" /\ RenameOver(Tmp(m), Dst(m)) /\ Then("sync-replaced")
       \/ st = "sync-replaced" /\ FsyncDir(Dir(Dst(m))) /\ NextOne
       \* The old order, kept as a fault: moved to the backups, then the
       \* repair published.
       \/ st = "moved-bak" /\ FsyncDir(Dir(Bak(m, run))) /\ Then("repair-new")
       \/ st = "repair-new" /\ Create(Tmp(m), Fix(m)) /\ Then("fsync")

ReconcileEnd == /\ pc = "reconcile" /\ pos > Len(Checked) /\ Keep
                /\ Phase("finish")

\* Done: the busy flag goes, and its removal is made durable.
FinishStep ==
    /\ pc = "finish"
    /\ \/ /\ st = "begin"
          /\ IF Exists(Busy) THEN Unlink(Busy) ELSE Keep
          /\ Then("sync")
       \/ /\ st = "sync" /\ FsyncDir(MarkerDir)
          /\ pc' = "ready" /\ UNCHANGED <<pos, st, run, taken, start>>

Done   == pc = "ready" /\ UNCHANGED vars
Halted == pc = "halted" /\ UNCHANGED vars   \* a failed migration: read-only until restart

Restart == /\ pc' = "start" /\ pos' = 1 /\ st' = "begin" /\ run' = run + 1
           /\ UNCHANGED <<taken, start>>
ProcessCrash == /\ run <= MaxCrashes
                /\ Keep
                /\ Restart
\* Which pending operations a power cut may keep, by index: any subset (A2),
\* or under Ntfs a prefix of each volume's operations (W1).
Rank(i) == Cardinality({j \in 1..i : VolOf(pending[j]) = VolOf(pending[i])})
Keeps == IF Ntfs
         THEN { {i \in 1..Len(pending) :
                    Rank(i) <= IF VolOf(pending[i]) = "A" THEN ja ELSE jb} :
                ja \in 0..Len(pending), jb \in 0..Len(pending) }
         ELSE SUBSET (1..Len(pending))
CutKeeping(keep) ==
    LET names == ApplySome(dns, pending, keep)
    IN /\ ns' = names
       /\ dns' = names
       /\ data' = ddata
       /\ UNCHANGED ddata
       /\ pending' = << >>
       /\ SetAct(NoAct)
       /\ Restart
PowerCut == /\ PowerLoss
            /\ run <= MaxCrashes
            /\ \E keep \in Keeps : CutKeeping(keep)

Step == \/ StartStep \/ MigrateBegin \/ MigrateStep \/ MigrateEnd \/ MarkerStep \/ PrepareStep
        \/ ReconcileBegin \/ ReconcileStep \/ ReconcileEnd \/ FinishStep
Next == Step \/ ProcessCrash \/ PowerCut \/ Done \/ Halted
Spec == Init /\ [][Next]_vars /\ WF_vars(Step)
-----------------------------------------------------------------------------
(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)
TypeOK == /\ ns \in [Paths -> Inodes \cup {NoInode}]
          /\ dns \in [Paths -> Inodes \cup {NoInode}]
          /\ data \in [Inodes -> Contents]
          /\ ddata \in [Inodes -> Contents]
          /\ pc \in {"start", "migrate", "marker", "prepare", "reconcile", "finish", "ready", "halted"}
          /\ pos \in 1..(Len(Order) + Len(Checked) + 1)
          /\ run \in Runs
          /\ taken \subseteq Moved \cup Outputs
          /\ start \in [Managed -> {"missing", "good", "bad"}]
          /\ TraceMode \/ act = NoAct

\* Every legacy item's original content exists somewhere, at every moment.
NoLoss == \A i \in Items :
             \E p \in IF i \in Kept THEN {Src(i), PBak(i)} ELSE {Src(i), Dst(i)} :
                C(p) = Orig(i)

\* A name that was already occupied is never written.
NoOverwrite == \A x \in taken : C(Dst(x)) = Theirs(x)

\* A damaged managed file's bytes survive, at its place or in a backup.
CorruptKept == \A m \in Managed : start[m] = "bad" =>
                  \E p \in {Dst(m)} \cup {Bak(m, r) : r \in Runs} : C(p) = Bad(m)

\* A whole managed file is never touched.
ValidUntouched == \A m \in Managed : start[m] = "good" => C(Dst(m)) = Good(m)

\* A managed file that existed is never absent, after any crash.
NeverAbsent == \A m \in Managed : start[m] # "missing" => Exists(Dst(m))

\* What a repair keeps is never only in a backup: a damaged file holds its
\* original or its repair.
RepairInPlace == \A m \in Managed : start[m] = "bad" => C(Dst(m)) \in {Bad(m), Fix(m)}

\* No garbage or partial content is ever visible at a published name.
PublishedComplete ==
    /\ \A x \in Named : C(Dst(x)) \notin {Garbage} \cup {Partial(i) : i \in Items}
    /\ \A i \in Kept : C(PBak(i)) \notin {Garbage, Partial(i)}
    /\ C(Marker) \in {None, Note}

\* What every finished migration holds.
Migrated == /\ \A i \in Items : ~Exists(Src(i)) \/ i \in taken
            /\ \A i \in Moved \ taken : C(Dst(i)) = Orig(i)
            /\ \A i \in Kept : C(PBak(i)) = Orig(i)
            /\ \A o \in Outputs \ taken : C(Dst(o)) = Conv(o)

\* The marker is only ever seen once every action is complete.
MarkerImpliesComplete == Exists(Marker) => Migrated

\* When start-up is done, everything is in place and whole.
ReadyIsValid ==
    pc = "ready" =>
        /\ Exists(Marker)
        /\ Migrated
        /\ \A m \in Managed : Valid(m, C(Dst(m)))
        /\ \A m \in Managed : start[m] = "bad" => C(Dst(m)) = Fix(m)
        /\ ~Exists(Busy)

\* When start-up is done, nothing it did is still waiting to reach the disk.
ReadyIsDurable == pc = "ready" => pending = << >>

Termination == <>[](pc = "ready")

THEOREM Spec => [](/\ TypeOK /\ NoLoss /\ NoOverwrite /\ CorruptKept
                   /\ ValidUntouched /\ PublishedComplete
                   /\ MarkerImpliesComplete /\ ReadyIsValid /\ ReadyIsDurable
                   /\ NeverAbsent /\ RepairInPlace)
=============================================================================
