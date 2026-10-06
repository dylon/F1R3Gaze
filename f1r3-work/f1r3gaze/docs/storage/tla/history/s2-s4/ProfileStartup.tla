--------------------------- MODULE ProfileStartup ---------------------------
(***************************************************************************)
(* F1R3Gaze's start-up under crashes: moving a legacy single-folder        *)
(* profile into the new layout, then reconciling the app-owned files.      *)
(* The design is in docs/storage/README.md, section "The formal model".    *)
(*                                                                         *)
(* The file system is an abstract persistence model in the style of        *)
(* Pillai et al., "All File Systems Are Not Created Equal" (OSDI 2014),    *)
(* section 3.2, with three assumptions:                                    *)
(*   A1  a directory operation (create, link, unlink, rename) is atomic;   *)
(*   A2  directory operations reach the disk in any subset and any order,  *)
(*       except that fsync of a directory makes every pending operation    *)
(*       that touches that directory durable;                              *)
(*   A3  a file's data is durable only once the file itself is fsynced.    *)
(*                                                                         *)
(* Inodes separate names from data, so a link or a rename carries          *)
(* unsynced data with it. A ProcessCrash keeps everything the kernel       *)
(* holds; a PowerCut keeps only what is durable plus any subset of the     *)
(* pending directory operations.                                           *)
(*                                                                         *)
(* With Ntfs, the semantics F1R3Gaze relies on under Windows instead       *)
(* (crates/gaze-fs/src/fs.rs, StdFs):                                      *)
(*   W1  each volume logs its metadata changes in order, so a power cut    *)
(*       keeps a prefix of each volume's pending operations;               *)
(*   W2  syncing a file (FlushFileBuffers) commits its volume's log;       *)
(*   and directories cannot be synced, while every link or rename that     *)
(*   names a file then syncs it.                                           *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Order,       \* the legacy items, in the order the program visits them
    Converted,   \* items rewritten into a new format (settings.conf)
    CrossDev,    \* items whose new place is on another file system
    Checked,     \* the managed files reconcile visits, in order
    MaxCrashes,  \* how many crashes one behaviour may contain
    PowerLoss,   \* whether a crash may lose what is not yet durable
    Ntfs,        \* Windows semantics (W1, W2) instead of A2
    Fault        \* "none", or a deliberate defect: the model's mutation checks

Items   == {Order[j] : j \in 1..Len(Order)}
Managed == {Checked[j] : j \in 1..Len(Checked)}
Runs    == 1..(MaxCrashes + 1)
Faults  == {"none", "regenerate-before-backup", "replace-destination",
            "marker-first", "no-fsync-before-publish",
            "no-dir-fsync-before-unlink", "resume-stale-temp",
            "no-start-barrier", "settle-only", "no-commit-name"}

ASSUME /\ Converted \subseteq Items
       /\ CrossDev \subseteq Items
       /\ Managed \cap Items = {}
       /\ MaxCrashes \in Nat
       /\ PowerLoss \in BOOLEAN
       /\ Ntfs \in BOOLEAN
       /\ Fault \in Faults
-----------------------------------------------------------------------------
(***************************************************************************)
(* Paths are <<directory, name>>. Every name is a tuple, so TLC never      *)
(* has to compare a string with a tuple. "legacy" is the old folder,       *)
(* "new" the new roots, "bak" the backups and preserved originals, all on  *)
(* volume A. "far" is a directory on volume B: where items on another file *)
(* system (CrossDev) go, with their preserved originals.                   *)
(***************************************************************************)
Home(x)   == IF x \in CrossDev THEN "far" ELSE "new"
PreserveDir(i) == IF i \in CrossDev THEN "far" ELSE "bak"
Src(i)    == <<"legacy", <<"item", i>>>>
Dst(x)    == <<Home(x), <<"item", x>>>>      \* Dst(m) is managed file m
Tmp(x)    == <<Home(x), <<"tmp", x>>>>
PBak(i)   == <<PreserveDir(i), <<"preserved", i>>>>  \* a converted item's original
BakTmp(i) == <<PreserveDir(i), <<"tmp-preserved", i>>>>
Bak(x, r) == <<"bak", <<"backup", x, r>>>>    \* backups/<time of run r>/x
Marker    == <<"new", <<"marker">>>>          \* data/layout.json
MarkerTmp == <<"new", <<"marker-tmp">>>>
Dir(p)    == p[1]
Vol(d)    == IF d = "far" THEN "B" ELSE "A"

InitialPaths == {Src(i) : i \in Items} \cup {Dst(x) : x \in Items \cup Managed}
TempPaths    == {Tmp(x) : x \in Items \cup Managed}
                \cup {BakTmp(i) : i \in Items} \cup {MarkerTmp}
Paths        == InitialPaths \cup TempPaths \cup {Marker}
                \cup {PBak(i) : i \in Items}
                \cup {Bak(m, r) : m \in Managed, r \in Runs}

None       == <<"none">>
Garbage    == <<"garbage">>
Note       == <<"note">>
Orig(i)    == <<"orig", i>>
Partial(i) == <<"partial", i>>
Conv(i)    == <<"conv", i>>
Theirs(i)  == <<"theirs", i>>
Good(m)    == <<"good", m>>
Bad(m)     == <<"bad", m>>
Default(m) == <<"default", m>>
Contents == {None, Garbage, Note}
            \cup {Orig(i) : i \in Items} \cup {Partial(i) : i \in Items}
            \cup {Conv(i) : i \in Converted} \cup {Theirs(i) : i \in Items}
            \cup {Good(m) : m \in Managed} \cup {Bad(m) : m \in Managed}
            \cup {Default(m) : m \in Managed}

Want(i)     == IF i \in Converted THEN Conv(i) ELSE Orig(i)
Valid(m, c) == c \in {Good(m), Default(m)}

NoInode == <<"no-inode">>
Inodes  == {<<"pre", p>> : p \in InitialPaths}
           \cup {<<"new", p, r>> : p \in TempPaths, r \in Runs}
-----------------------------------------------------------------------------
VARIABLES
    ns,       \* path -> inode, as the program sees the names
    dns,      \* path -> inode, as a power cut would leave the names
    data,     \* inode -> content, as the program sees it
    ddata,    \* inode -> content, as a power cut would leave it
    pending,  \* directory operations not yet durable, oldest first
    pc,       \* phase: start, migrate, marker, reconcile, ready
    pos,      \* the item or managed file being visited
    st,       \* the step within that item
    run,      \* 1 + the crashes so far
    taken,    \* ghost: legacy items whose new place was already occupied
    start     \* ghost: each managed file's state before the first start

vars == <<ns, dns, data, ddata, pending, pc, pos, st, run, taken, start>>

C(p)      == IF ns[p] = NoInode THEN None ELSE data[ns[p]]
Exists(p) == ns[p] # NoInode
Fresh(p)  == <<"new", p, run>>

Initial(p) ==
    CASE Dir(p) = "legacy" -> Orig(p[2][2])
      [] Dir(p) \in {"new", "far"} /\ p[2][2] \in taken -> Theirs(p[2][2])
      [] Dir(p) = "new" /\ p[2][2] \in Managed /\ start[p[2][2]] = "good" -> Good(p[2][2])
      [] Dir(p) = "new" /\ p[2][2] \in Managed /\ start[p[2][2]] = "bad" -> Bad(p[2][2])
      [] OTHER -> None

Init == /\ taken \in SUBSET Items
        /\ start \in [Managed -> {"missing", "good", "bad"}]
        /\ ns = [p \in Paths |-> IF p \in InitialPaths /\ Initial(p) # None
                                 THEN <<"pre", p>> ELSE NoInode]
        /\ dns = ns
        /\ data = [n \in Inodes |-> IF n[1] = "pre" THEN Initial(n[2]) ELSE None]
        /\ ddata = data
        /\ pending = << >>
        /\ pc = "start" /\ pos = 1 /\ st = "begin" /\ run = 1
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

Keep == UNCHANGED <<ns, dns, data, ddata, pending>>
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

\* Replacing an existing name is what the program never does; two faults do.
MayReplace == Fault \in {"replace-destination", "regenerate-before-backup"}

Create(p, c) == /\ ~Exists(p)
                /\ data[Fresh(p)] = None          \* one inode per path and run
                /\ DirOp({<<p, Fresh(p)>>})
                /\ data'  = [data  EXCEPT ![Fresh(p)] = c]
                /\ ddata' = [ddata EXCEPT ![Fresh(p)] = Garbage]
                /\ UNCHANGED dns
Write(p, c) == /\ Exists(p)
               /\ data' = [data EXCEPT ![ns[p]] = c]
               /\ UNCHANGED <<ns, dns, ddata, pending>>
FsyncFile(p) == /\ Exists(p)
                /\ ddata' = [ddata EXCEPT ![ns[p]] = data[ns[p]]]
                /\ IF Ntfs THEN CommitVolume(pending, Vol(Dir(p)))
                   ELSE UNCHANGED <<dns, pending>>
                /\ UNCHANGED <<ns, data>>
\* A directory cannot be synced under Ntfs.
FsyncDir(d) == IF Ntfs THEN Keep
               ELSE LET Touches(op) == d \in op.dirs
                        Other(op)   == d \notin op.dirs
                    IN /\ dns' = ApplyAll(dns, SelectSeq(pending, Touches))
                       /\ pending' = SelectSeq(pending, Other)
                       /\ UNCHANGED <<ns, data, ddata>>
\* The start barrier syncs every directory: nothing under Ntfs.
Barrier == IF Ntfs THEN Keep
           ELSE /\ dns' = ApplyAll(dns, pending)
                /\ pending' = << >>
                /\ UNCHANGED <<ns, data, ddata>>
\* link(2): a second name for the same inode; fails if the name exists.
Link(a, b) == /\ Exists(a)
              /\ \/ ~Exists(b)
                 \/ MayReplace
              /\ Naming({<<b, ns[a]>>}, ns[a], b)
              /\ UNCHANGED data
Unlink(p) == /\ Exists(p)
             /\ DirOp({<<p, NoInode>>})
             /\ UNCHANGED <<dns, data, ddata>>
\* rename(2) after checking that the target is free (no hard links).
Rename(a, b) == /\ Exists(a)
                /\ \/ ~Exists(b)
                   \/ MayReplace
                /\ Naming({<<a, NoInode>>, <<b, ns[a]>>}, ns[a], b)
                /\ UNCHANGED data
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

\* Start: the instance lock is held; flush what a crashed run left pending.
Begin == /\ pc = "start"
         /\ IF Fault \in {"no-start-barrier", "settle-only"} THEN Keep ELSE Barrier
         /\ Phase(IF Exists(Marker) THEN "reconcile"
                  ELSE IF Fault = "marker-first" THEN "marker"
                  ELSE "migrate")

Item == Order[pos]

MigrateBegin ==
    /\ pc = "migrate" /\ pos <= Len(Order) /\ st = "begin"
    /\ LET i == Item IN
       IF Exists(Tmp(i))
       THEN IF Fault = "resume-stale-temp"
            THEN Keep /\ Then("publish")
            ELSE Unlink(Tmp(i)) /\ Then("swept-tmp")
       ELSE IF Exists(BakTmp(i)) THEN Unlink(BakTmp(i)) /\ Then("swept-bak")
       ELSE IF ~Exists(Src(i)) THEN Keep /\ NextOne
       ELSE IF Exists(Dst(i)) /\ C(Dst(i)) = Want(i)
       THEN Keep /\ Then(IF Fault = "settle-only" THEN "settle" ELSE "retire")
       ELSE IF Exists(Dst(i)) /\ Fault # "replace-destination"
       THEN Keep /\ NextOne                       \* a conflict: both are kept
       ELSE IF i \in Converted THEN Create(Tmp(i), Conv(i)) /\ Then("fsync")
       ELSE IF i \in CrossDev THEN Create(Tmp(i), Partial(i)) /\ Then("copy-rest")
       ELSE \/ Link(Src(i), Dst(i)) /\ Then("sync-new")
            \/ Rename(Src(i), Dst(i)) /\ Then("sync-new")

MigrateStep ==
    /\ pc = "migrate" /\ pos <= Len(Order)
    /\ LET i == Item IN
       \/ st = "copy-rest" /\ Write(Tmp(i), C(Src(i))) /\ Then("fsync")
       \/ /\ st = "fsync"
          /\ IF Fault = "no-fsync-before-publish" THEN Keep ELSE FsyncFile(Tmp(i))
          /\ Then("verify")
       \/ /\ st = "verify" /\ Keep
          /\ Then(IF C(Tmp(i)) = Want(i) THEN "publish" ELSE "discard")
       \/ st = "discard" /\ Unlink(Tmp(i)) /\ Then("discarded")
       \/ st = "discarded" /\ FsyncDir(Dir(Tmp(i))) /\ NextOne
       \/ st = "swept-tmp" /\ FsyncDir(Dir(Tmp(i))) /\ Then("begin")
       \/ st = "swept-bak" /\ FsyncDir(Dir(BakTmp(i))) /\ Then("begin")
       \/ st = "publish" /\ Link(Tmp(i), Dst(i)) /\ Then("unlink-tmp")
       \/ st = "unlink-tmp" /\ Unlink(Tmp(i)) /\ Then("sync-new")
       \/ /\ st = "sync-new"
          /\ IF Fault = "no-dir-fsync-before-unlink" THEN Keep ELSE FsyncDir(Dir(Dst(i)))
          /\ Then("retire")
       \/ st = "settle" /\ FsyncFile(Dst(i)) /\ Then("settle-dir")
       \/ st = "settle-dir" /\ FsyncDir(Dir(Dst(i))) /\ Then("retire")
       \/ /\ st = "retire"
          /\ IF ~Exists(Src(i)) THEN Keep /\ NextOne
             ELSE IF i \notin Converted THEN Unlink(Src(i)) /\ Then("sync-legacy")
             ELSE IF Exists(PBak(i)) /\ C(PBak(i)) = Orig(i)
             THEN Unlink(Src(i)) /\ Then("sync-legacy")
             ELSE IF Exists(PBak(i)) THEN Keep /\ NextOne
             ELSE IF i \notin CrossDev THEN Link(Src(i), PBak(i)) /\ Then("sync-bak")
             ELSE Create(BakTmp(i), Partial(i)) /\ Then("pcopy-rest")
       \/ st = "pcopy-rest" /\ Write(BakTmp(i), C(Src(i))) /\ Then("pfsync")
       \/ /\ st = "pfsync"
          /\ IF Fault = "no-fsync-before-publish" THEN Keep ELSE FsyncFile(BakTmp(i))
          /\ Then("pverify")
       \/ /\ st = "pverify" /\ Keep
          /\ Then(IF C(BakTmp(i)) = Orig(i) THEN "plink" ELSE "pdiscard")
       \/ st = "pdiscard" /\ Unlink(BakTmp(i)) /\ Then("pdiscarded")
       \/ st = "pdiscarded" /\ FsyncDir(Dir(BakTmp(i))) /\ NextOne
       \/ st = "plink" /\ Link(BakTmp(i), PBak(i)) /\ Then("punlink-tmp")
       \/ st = "punlink-tmp" /\ Unlink(BakTmp(i)) /\ Then("sync-bak")
       \/ st = "sync-bak" /\ FsyncDir(Dir(PBak(i))) /\ Then("unlink-src")
       \/ st = "unlink-src" /\ Unlink(Src(i)) /\ Then("sync-legacy")
       \/ st = "sync-legacy" /\ FsyncDir("legacy") /\ NextOne

MigrateEnd == /\ pc = "migrate" /\ pos > Len(Order) /\ Keep
              /\ Phase(IF Fault = "marker-first" THEN "reconcile" ELSE "marker")

AfterMarker == IF Fault = "marker-first" THEN "migrate" ELSE "reconcile"
MarkerStep ==
    /\ pc = "marker"
    /\ \/ st = "begin" /\ Exists(Marker) /\ Keep /\ Phase(AfterMarker)
       \/ /\ st = "begin" /\ ~Exists(Marker) /\ Exists(MarkerTmp)
          /\ Unlink(MarkerTmp) /\ Then("swept")
       \/ st = "swept" /\ FsyncDir("new") /\ Then("begin")
       \/ /\ st = "begin" /\ ~Exists(Marker) /\ ~Exists(MarkerTmp)
          /\ Create(MarkerTmp, Note) /\ Then("fsync")
       \/ st = "fsync" /\ FsyncFile(MarkerTmp) /\ Then("publish")
       \/ st = "publish" /\ Link(MarkerTmp, Marker) /\ Then("unlink-tmp")
       \/ st = "unlink-tmp" /\ Unlink(MarkerTmp) /\ Then("sync")
       \/ st = "sync" /\ FsyncDir("new") /\ Phase(AfterMarker)

Checking == Checked[pos]

ReconcileBegin ==
    /\ pc = "reconcile" /\ pos <= Len(Checked) /\ st = "begin"
    /\ LET m == Checking IN
       IF Exists(Tmp(m)) THEN Unlink(Tmp(m)) /\ Then("swept")
       ELSE IF ~Exists(Dst(m)) THEN Create(Tmp(m), Default(m)) /\ Then("fsync")
       ELSE IF Valid(m, C(Dst(m))) THEN Keep /\ NextOne
       ELSE IF Fault = "regenerate-before-backup"
       THEN Create(Tmp(m), Default(m)) /\ Then("fsync")
       ELSE Rename(Dst(m), Bak(m, run)) /\ Then("sync-bak")

ReconcileStep ==
    /\ pc = "reconcile" /\ pos <= Len(Checked)
    /\ LET m == Checking IN
       \/ st = "swept" /\ FsyncDir("new") /\ Then("begin")
       \/ st = "sync-bak" /\ FsyncDir("bak") /\ Then("begin")
       \/ /\ st = "fsync"
          /\ IF Fault = "no-fsync-before-publish" THEN Keep ELSE FsyncFile(Tmp(m))
          /\ Then("publish")
       \/ st = "publish" /\ Link(Tmp(m), Dst(m)) /\ Then("unlink-tmp")
       \/ st = "unlink-tmp" /\ Unlink(Tmp(m)) /\ Then("sync-new")
       \/ st = "sync-new" /\ FsyncDir("new") /\ NextOne

ReconcileEnd == /\ pc = "reconcile" /\ pos > Len(Checked) /\ Keep
                /\ pc' = "ready"
                /\ UNCHANGED <<pos, st, run, taken, start>>

Done == pc = "ready" /\ UNCHANGED vars

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
PowerCut == /\ PowerLoss
            /\ run <= MaxCrashes
            /\ \E keep \in Keeps :
                  LET names == ApplySome(dns, pending, keep)
                  IN /\ ns' = names
                     /\ dns' = names
            /\ data' = ddata
            /\ UNCHANGED ddata
            /\ pending' = << >>
            /\ Restart

Step == \/ Begin \/ MigrateBegin \/ MigrateStep \/ MigrateEnd \/ MarkerStep
        \/ ReconcileBegin \/ ReconcileStep \/ ReconcileEnd
Next == Step \/ ProcessCrash \/ PowerCut \/ Done
Spec == Init /\ [][Next]_vars /\ WF_vars(Step)
-----------------------------------------------------------------------------
(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)
TypeOK == /\ ns \in [Paths -> Inodes \cup {NoInode}]
          /\ dns \in [Paths -> Inodes \cup {NoInode}]
          /\ data \in [Inodes -> Contents]
          /\ ddata \in [Inodes -> Contents]
          /\ pc \in {"start", "migrate", "marker", "reconcile", "ready"}
          /\ pos \in 1..(Len(Order) + Len(Checked) + 1)
          /\ run \in Runs
          /\ taken \subseteq Items
          /\ start \in [Managed -> {"missing", "good", "bad"}]

\* Every legacy item's original content exists somewhere, at every moment.
NoLoss == \A i \in Items : \E p \in {Src(i), Dst(i), PBak(i)} : C(p) = Orig(i)

\* A place that was already occupied is never written.
NoOverwrite == \A i \in taken : C(Dst(i)) = Theirs(i)

\* A corrupt managed file's bytes survive, at its place or in a backup.
CorruptKept == \A m \in Managed : start[m] = "bad" =>
                  \E p \in {Dst(m)} \cup {Bak(m, r) : r \in Runs} : C(p) = Bad(m)

\* A valid managed file is never touched.
ValidUntouched == \A m \in Managed : start[m] = "good" => C(Dst(m)) = Good(m)

\* No garbage or partial content is ever visible at a published name.
PublishedComplete ==
    /\ \A x \in Items \cup Managed :
          C(Dst(x)) \notin {Garbage} \cup {Partial(i) : i \in Items}
    /\ \A i \in Items : C(PBak(i)) \notin {Garbage, Partial(i)}
    /\ C(Marker) \in {None, Note}

\* The marker is only ever seen once every item has moved.
MarkerImpliesComplete ==
    Exists(Marker) =>
        \A i \in Items \ taken :
            /\ ~Exists(Src(i))
            /\ C(Dst(i)) = Want(i)
            /\ i \in Converted => C(PBak(i)) = Orig(i)

\* When start-up is done, everything is in place and valid.
ReadyIsValid ==
    pc = "ready" =>
        /\ Exists(Marker)
        /\ \A m \in Managed : Valid(m, C(Dst(m)))
        /\ \A i \in Items \ taken : C(Dst(i)) = Want(i)

\* When start-up is done, nothing it did is still waiting to reach the disk,
\* so the next start may skip the barrier (docs/storage/README.md, 7.2).
ReadyIsDurable == pc = "ready" => pending = << >>

Termination == <>[](pc = "ready")

THEOREM Spec => [](/\ TypeOK /\ NoLoss /\ NoOverwrite /\ CorruptKept
                   /\ ValidUntouched /\ PublishedComplete
                   /\ MarkerImpliesComplete /\ ReadyIsValid /\ ReadyIsDurable)
=============================================================================
