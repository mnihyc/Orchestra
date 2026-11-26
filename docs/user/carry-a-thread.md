---
group: operations
title: Carry a thread to another agent
screens: /thread/$threadId, /admin/$section
keywords: Carry, fork, switch, history, native import, neutral import, file handoff, Codex, harness builds, Check, unavailable
summary: Review how a fork or a switch carries the conversation you selected to another agent.
---

# Carry a thread to another agent

Choose **Carry** in the plan of a fork or a switch to bring the conversation you selected. Read the history method of the plan before you confirm. The method follows what the exact build of the agent program at the destination has been shown to import.

## Read the method

The plan names one of three methods:

- **Built-in import** restores the conversation of the agent program itself.
- **Neutral import** brings the recorded turns in the shared format.
- **File handoff** gives the new agent the recent context and the full conversation as a file.

When the fork of a side thread includes its first turn, the plan also counts the conversation of the parent that was captured when the side thread was made.

**Codex.** A Codex thread keeps its history in the store of the program, and Orch never copies it.

- In the home where it was made, and when an environment moves with its volumes, the thread resumes with its own history. A moved environment opens a thread when it is next used, not when it moves.
- When Codex resumes a session, a quiet line explains that it keeps the standing instructions the session started with. Changes to those instructions apply to the next new session. This note does not mean that the turn failed.
- To fork at a turn with its own history, Orch first checks the place of the turn against the record that the program keeps of the conversation, which is kept to 32 MiB. A conversation that holds more than **Built-in fork check size** allows (32 MiB unless a place changes it) is not checked. The plan then says **neutral import** or **file handoff**, as for another home, and tells you so.
- Into a different home (another environment, the home of another credential, or a machine without the volume), Codex cannot give its history back whole. The plan then says **neutral import** when every turn you selected fits the carry share of the window of the new model for that place. Otherwise it says **file handoff**: the newest turns that fit go in word for word, and the whole conversation is a file in the workspace. What travels is bounded by the window and not by the length of the thread.

Where the history of a program does cross as a file (its own import for a program that exports it, between machines), the job shows how many bytes have reached Orch and when the rest should. Cancelling the job stops the transfer on the machine, which removes its file. A transfer that fails part way leaves what arrived with Orch for a day. Carrying the thread again while it has not changed sends only the rest.

## Check another build

In **Management → Builds**, **Check** observes the exact build in an isolated environment. When the program's own import is unavailable, that feature stays off and the check records why. Other checks can still pass.

- For a program that exports its history, the import becomes available when the build exports the conversation, imports it into a fresh home and reads both completed turns back.
- For Codex, the check forks a conversation at its second turn in the home where it was made, and reopens it in a copy of that home. The next request must carry the turns up to the fork and no later one.
- Changing the overrides of a profile cannot turn on an import that has not been shown to work.
