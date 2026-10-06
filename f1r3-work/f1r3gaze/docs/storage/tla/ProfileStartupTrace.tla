------------------------- MODULE ProfileStartupTrace -------------------------
(***************************************************************************)
(* Trace validation of the start-up protocol (ledger S8), after Cirstea,    *)
(* Kuppe, Loillier and Merz, "Validating Traces of Distributed Programs     *)
(* Against TLA+ Specifications", SEFM 2024.                                 *)
(*                                                                         *)
(* A real start's file operations, recorded by gaze-fs's TraceFs and put   *)
(* into the model's terms by crates/gaze-shell/src/profile/migrate/trace.rs,*)
(* form Log. TLC explores only the steps of ProfileStartup that do what the *)
(* next event of Log says, and a few steps that touch no file. The trace is *)
(* a behaviour of the model exactly when every event can be matched: then   *)
(* TraceEnd is violated, which is how TLC reports the acceptance. A trace   *)
(* that cannot be matched leaves TLC with no step to take: it stops with no *)
(* violation, and the trace is rejected.                                    *)
(***************************************************************************)
EXTENDS ProfileStartup, TLC

CONSTANTS
    Log,         \* the events, in order: records [op, p, q, c, d, dirs, keep]
    TraceTaken,  \* the destinations that held something before the first start
    TraceStart   \* each managed file's state before the first start

VARIABLE l       \* the next event of Log to match

TraceInit == /\ Init
             /\ taken = TraceTaken
             /\ start = TraceStart
             /\ l = 1

Ev == Log[l]

\* A step of the model that does to the files what the event says.
Same(e) == /\ act'.op = e.op
           /\ act'.p = e.p
           /\ act'.q = e.q
           /\ act'.c = e.c
           /\ act'.d = e.d

\* A directory sync the protocol does not need where it happens: it only
\* makes pending operations durable, so it removes crash outcomes and adds
\* none.
Eager(e) == /\ e.op = "fsync-dir"
            /\ FsyncDir(e.d)
            /\ UNCHANGED <<pc, pos, st, run, taken, start>>

\* The start's barrier. The folders it synced (e.dirs) must cover every
\* pending operation, all of which the model's Barrier makes durable.
BarrierOk(e) == /\ e.op = "barrier"
                /\ \A j \in 1..Len(pending) : pending[j].dirs \subseteq e.dirs
                /\ StartStep
                /\ act'.op = "barrier"

\* A crash the test made: the process killed, or the power cut, keeping the
\* pending operations whose indices (into the model's queue) are in e.keep.
CrashOk(e) == \/ /\ e.op = "crash-process"
                 /\ ProcessCrash
              \/ /\ e.op = "crash-power"
                 /\ PowerLoss
                 /\ run <= MaxCrashes
                 /\ e.keep \in Keeps
                 /\ CutKeeping(e.keep)

TraceNext ==
    \/ /\ l <= Len(Log)
       /\ \/ CrashOk(Ev)
          \/ BarrierOk(Ev)
          \/ Eager(Ev)
          \/ /\ Step
             /\ act'.op \notin {"none", "barrier"}
             /\ Same(Ev)
       /\ l' = l + 1
    \* A step that touches no file: a decision, or moving to the next action.
    \/ /\ Step
       /\ act'.op = "none"
       /\ UNCHANGED l

\* Violated exactly when every event of Log was matched: the acceptance.
TraceEnd == l <= Len(Log)
=============================================================================
