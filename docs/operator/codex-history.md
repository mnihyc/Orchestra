# Codex history and carries

[简体中文](zh-CN/codex-history.md)

Codex keeps its history in its own home. Keep that home with the environment's
volumes when preserving native sessions. Orch does not transplant a rollout file
into another home or force Codex to keep its history in the legacy format.

| Destination | History method |
| --- | --- |
| The same home | Resume the existing native session |
| An environment moved with its home, using Mount as-is or Copy | Reopen the native session when that thread is next used |
| Another home, credential's home, or machine without the volume | Carry the newest recorded turns within the destination model's window, with a transcript file |

A move with the home transfers the volume. It does not open every thread during
the move. Threads open when they are next used.

## Fork checks

A native fork at a particular turn also needs a complete check of that turn's
position against the program's recorded conversation. **Native fork check size**
defaults to 32 MiB. The program's record also has a 32 MiB limit. Raising the
setting does not remove the record limit or the checks of content and order.

When the native check cannot be completed, the plan names **neutral import** or
**file handoff**. Read that method before confirming. A native resume does not
mean that a fork of the same history has passed this check.

## Carries into another home

Codex does not expose a lossless export of its raw model history for another
home. The carry uses the recorded conversation instead. It brings the newest
turns that fit the place's carry share, 80% of the new model's window by default.
It writes the full conversation to a file in the workspace, a page at a time.
The model input is bounded by the window; the transcript's disk work grows with
the conversation. Reserve disk space for that file.

After a native resume or adoption, one ordinary timeline line explains that
the session retains its original standing instructions. Instruction changes
apply to the next new session. This line does not mean the turn failed.

See [Carry a thread](../user/carry-a-thread.md),
[agent build checks](harness-build-checks.md), and
[managed agent history](agent-history.md) for method selection, build validation
and retention.
