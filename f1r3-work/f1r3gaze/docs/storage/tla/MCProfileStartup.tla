---------------------------- MODULE MCProfileStartup ----------------------------
(* Model values for TLC (the Toolbox pattern): the cfg files bind the        *)
(* constants of ProfileStartup to these definitions.                         *)
EXTENDS ProfileStartup

CONSTANTS u, k, s, w, so, ss, sh, m

\* u: user-id, moved on one file system; k: a key, moved to another file
\* system; s: settings.conf, its original preserved in the backups; so:
\* settings.toml, written from it; m: session.json, a managed file. The
\* actions in the plan's order: writes (M3), moves (M4), preserves (M5).
MC_Order     == << <<"write", so>>, <<"move", u>>, <<"move", k>>, <<"preserve", s>> >>

\* The same actions in another order: the verdicts must not depend on it.
MC_ItemOrder == << <<"write", so>>, <<"preserve", s>>, <<"move", u>>, <<"move", k>> >>

\* The wide configuration: workspace.json too (w), with its two outputs
\* (ss: session.json, sh: history.json), its original preserved on the
\* other file system.
MC_WideOrder == << <<"write", so>>, <<"write", ss>>, <<"write", sh>>,
                   <<"move", u>>, <<"move", k>>, <<"preserve", s>>, <<"preserve", w>> >>

\* Each file is in its class's folder, as in the code's layout (ledger S6,
\* part 2): settings.toml in config; user-id and the marker in data;
\* session.json and history.json, and the managed file m, in state; the
\* key on the other file system. A preserved original goes into config's
\* backups (s) or onto the other file system (w); m's copy before its
\* repair into state's backups. (Until S6 part 2 every file but k was in one
\* folder with the marker, so the sync that ends start-up also synced m's
\* folder, and hid what the busy flag does after a crash in the repair.)
MC_SrcDir  == [x \in {u, k, s, w} |-> "legacy"]
MC_DstDir  == [x \in {u, k, so, ss, sh, m} |->
                  CASE x = k -> "far"
                    [] x = so -> "config"
                    [] x \in {ss, sh, m} -> "state"
                    [] OTHER -> "data"]
MC_BakDir  == [x \in {s, w, m} |->
                  CASE x = w -> "far"
                    [] x = s -> "config-backups"
                    [] OTHER -> "state-backups"]
MC_FarDirs == {"far"}
MC_Checked == <<m>>
==================================================================================
