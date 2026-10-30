# The DeepSeek Harness plugin

DeepSeek Harness (`dsh`) speaks the Agent Client Protocol, which has no live text, no steering, no questions to a
person, no enforced plan mode, no token counts, no compaction phases, no fork and no way to bring a thread's history
along. Orch ships a plugin that runs inside `dsh` and adds each of these. This page says how it is loaded, what each
extra needs, why one can be off, and how to read the reason.

## How it is loaded

The plugin is one file, `orch-dsh-bridge.mjs`, kept beside the `dsh` build in the tool cache. Orch writes a patch file for
each session and gives it to `dsh --patch`. The patch holds one row with the file's absolute path, and a row for dsh's own
`ask_user_question` tool, which the ACP profile does not load by itself. Nothing in dsh's shared home is changed. The
plugin does nothing in a process that only prints (a help, a version, a configuration dump).

If the file is missing, `dsh` runs as plain ACP: sessions, tool calls, approvals, cancel and resume work, and every extra is
reported off.

## The extras and what each needs

| Extra | What it adds | What it needs from dsh |
|---|---|---|
| Live text | text and reasoning as they are written | the agent loop's live stream |
| Steering | a message to a turn that is running, and a follow-up | the agent handle's `steer` and `followup` |
| Questions | `ask_user_question` as a question a person answers | the user-questions service and the question tool's row |
| Plan | plan mode that refuses changes until the plan is accepted; the plan as a card | the plan-mode service and the tool pipeline's guards |
| Approvals | asks before a command that writes, a file write or an edit | the approval service |
| Token breakdown | fresh input, cache read and write, output, per model call | the session log's usage records |
| Compaction phases | when `dsh` compacts, and a manual compaction | the compaction service |
| Skills | the session's skills, in `<home>/skills` or the directory Orch names | the skills registry |
| Fork | a copy of a session at the end of a turn | the agent registry and the session store |
| Carry history | a switched thread's history written into a new `dsh` session | the same |
| Shell and files in the environment | `dsh`'s commands and files run in the environment | the tools registry, and a launch that switches off `dsh`'s own tools |

## Shell and files in the environment

When the environment is not on the machine `dsh` runs on (the separated arrangement), the plugin registers the tools the model
uses to reach it (`bash`, `read`, `write`, `edit`, `glob`, `grep`, `job_output`, `job_kill`, `job_list`) under the names of
`dsh`'s own, and each call goes to Orch's executor with the address and token Orch gives that session. Approvals and plan mode
work as they do for `dsh`'s own tools. `dsh`'s own services keep working on the provider's machine: its home, its skills, its
session files. The launch must switch off the rows that hold the same tool names (`tool-bash`, `tool-fs`, `tool-fs-search`,
`tool-jobs`, `tool-working-directory`) and set `backends.enabled`; the extra reports itself off with the row to switch off when
a name is taken, and off as not applicable when `backends.enabled` is not set.

## Why an extra is off

Orch shows a reason beside each extra that is off, on the page of a build's checks, under **Management**.

| Reason | Meaning | What to do |
|---|---|---|
| Switched off in settings | Orch's settings for the program turn it off | Change the setting |
| This build lacks what it needs | A service the extra uses is not in this `dsh` build | Use a build that Orch has checked, or leave the extra off |
| This build changed what it relies on | The service is there but behaves differently from what the plugin was written against | The same: `dsh` is a preview and its internal interfaces change between builds |
| Its own check failed | The extra's start-up check failed; the detail says what | Read the detail |
| It failed while running | The extra was on and raised an error; it stays off for the process | Read the detail; the thread goes on without it |
| This environment lacks what it needs | A directory or file the extra needs is missing, or the arrangement does not call for the extra (the environment is where `dsh` runs) | Read the detail |

`dsh` is a preview with breaking changes announced, and its plugin interface is not stable. Orch pins the build it ships and
checks a newer build before an environment receives it. A build that changes an interface turns the affected extras off with
their reasons; plain ACP keeps working, and no extra that is on is ever broken.

## What leaves the machine

Nothing but the model request: the plugin does not call out, and `dsh`'s own uploads (the session log and the plugin list sent
with every request to DeepSeek's official API) are switched off in the patch and refused again by Orch's model forwarder.
