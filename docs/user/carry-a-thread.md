---
title: Carry a thread to another agent
screens: /thread/$threadId, /admin/$section
keywords: Carry, fork, switch, history, native import, neutral import, file handoff, Codex, harness builds, Check, unavailable
---

# Carry a thread to another agent

Choose **Carry** in a fork or switch plan to bring the selected conversation.
Read the plan's history method before confirming. The method follows what
the destination's exact agent build has proved it can import.

## Read the method

A native import restores the agent program's own conversation. A neutral
import brings the recorded turns in the shared format. A file handoff gives
the new agent recent context and the full conversation as a file. The plan
names the method it will use.

When a side thread's fork includes its first turn, the plan also counts the
parent conversation captured when the side was created.

A Codex thread keeps its history in the program's own store, and Orch never
copies it. In the home where it was made, and when an environment moves with
its volumes, the thread resumes natively. A native fork also needs the turn
check described below. A
moved environment opens a thread when it is next used, not when it moves.
When Codex resumes a session, a quiet line explains that it keeps the session's
original standing instructions. Changes to those instructions apply to the next
new session. This note does not mean the turn failed.
To fork at a turn natively, Orch first checks the turn's place against the
program's record of the conversation, which is kept to 32 MiB. A conversation
that holds more than the setting **Native fork check size** allows (32 MiB
unless a place changes it) is not checked: the plan then says **neutral import**
or **file handoff**, as for another home, and tells you so.

Into a different home (another environment, another credential's home, or a
machine without the volume) Codex cannot give its history back whole. The plan
then says **neutral import** when every selected turn fits the place's carry
share of the new model's window. Otherwise it says **file handoff**: the
newest turns that fit go in word for word, and the whole conversation is a
file in the workspace. What travels is bounded by the window, not by the
length of the thread.

Where a program's history does cross as a file (a native import for a program
that exports it, between machines), the job shows how many bytes have reached
Orch and when the rest should. Cancelling the job stops the transfer on the
machine, which removes its file. A transfer that fails part way leaves what
arrived with Orch for a day: carrying the thread again, while it has not
changed, sends only the rest.

## Check another build

In **Management → Builds**, **Check** observes the exact build in an
isolated environment. An unavailable native import keeps that feature off
and records why. Other checks can still pass. For a program that exports its
history, native import becomes available when the build exports and imports
the conversation into a fresh home and reads both completed turns back. For
Codex, the check forks a conversation at its second turn in the home where it
was made, and reopens it in a copy of that home: the next request must carry
the turns up to the fork and no later one. Changing a profile's overrides
cannot enable an unproved native import.
