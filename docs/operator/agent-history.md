# Managed agent history

Orch retains conversations in PostgreSQL. Native agent history is a cache in
the managed home of a provider and agent profile. The machine measures files
in batches and stores their size with the segment; viewing settings or a list
does not walk homes.

Large inventories and historical records use temporary disk indexes. Keep
enough room in the node process's temporary directory for a captured transcript
and its indexes. Transcript and record spools are temporary. A home inventory
is cached for its current pass.
Adoption keeps the latest complete turns within the existing 32 MiB record
limit and reports earlier turns left out. Historical resync does not charge
recorded usage again; plan changes restore their final recorded state.

Native transport applies backpressure when its consumer stalls. The native
process is still supervised while delivery waits. A lost consumer or failed
native input is reported as a connection failure; pending work does not
silently disappear.

`retention.agent_history_idle_days` ships as 7 and accepts any nonnegative
number of days. Zero disables automatic removal. The existing retention job
resolves it for each thread, including archived and recently deleted threads.
Personal homes and adopted sessions are refused. Retained server provenance
covers sessions Orch created before managed cleanup was introduced.

A turn, waiting message, pending approval or plan, or retained native dependency
keeps the cache. The reservation compares the exact last-turn time under the
thread lock. A turn already admitted wins. An outstanding native command keeps
its admission fence through server restarts until its outcome is known.
An offline machine leaves an unavailable observation rather than a zero size.

Codex uses `thread/delete`, which frees active or archived rollouts. A build
missing the method is labelled by its interface check and uses only the exact
owned rollout file names. Archiving is not cleanup. Claude Code removes the
session transcript, its adjacent session directory and its file-history tree.
Neither path removes another session or the provider's sign-in.

Managed Codex homes disable duplicate terminal prompt recall with
`history.persistence = "none"`; rollouts remain resumable. Managed Claude Code
homes set `cleanupPeriodDays = 3650` so the program's independent sweep does
not compete with Orch's ordinary idle policy. Existing personal homes receive
neither setting.

Managed homes also keep native updates and independent activity under Orch's
control. Codex runs the selected build over standard input and output, with
daemon auto-start disabled and a private daemon updater preference written
once. An existing preference file is preserved. Claude Code receives settings
that disable account connector and skill/plugin sync, remote control and
incoming cross-session messages. Automatic background work and hooks are off.
Explicit Harness-kept notes still use the thread's own configured directory.
These additional controls do not change a person's own home.

An unexpected program exit leaves its reason in the conversation and marks a
running turn interrupted. The provider's Runtime section retains the latest
program error with its time, including after the process comes back online.
The next input follows the existing session-recovery and history policy.

The next turn reconstructs a missing session using the place's switch-history
choice. Recovery freezes the retained conversation. Codex can import recent
turns directly; a file handoff materializes the full indexed transcript before
opening the native session. Failed preparation keeps
the original input; a missing cache never silently discards the conversation.
