---------------------------- MODULE MCProfileStartup ----------------------------
(* Model values for TLC (the Toolbox pattern): the cfg files bind the        *)
(* constants of ProfileStartup to these definitions.                         *)
EXTENDS ProfileStartup

CONSTANTS u, k, s, v, m

\* u: user-id (same file system); k: keys/ (another file system);
\* s: settings.conf (converted); m: session.json (a managed file).
MC_Order     == <<u, k, s>>
MC_Converted == {s}
MC_CrossDev  == {k}
MC_Checked   == <<m>>

\* The wide configuration: v is a second converted item on another file
\* system, to test that the verdicts do not depend on how items combine.
MC_WideOrder     == <<u, k, s, v>>
MC_WideConverted == {s, v}
MC_WideCrossDev  == {k, v}
==================================================================================
