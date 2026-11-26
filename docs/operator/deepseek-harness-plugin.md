# Read why a DeepSeek Harness extra is off

Find out which extras of DeepSeek Harness are on for a build, and why one is off. At the end you know whether to change a setting, choose another build, or leave the extra off.

DeepSeek Harness (`dsh`) speaks the Agent Client Protocol, which has no live text, no steering, no questions to a person, no enforced plan mode, no token counts, no compaction phases, no fork and no way to bring a thread's history along. Orch ships a plugin that runs inside `dsh` and adds each of these. How `dsh` runs is on [Run DeepSeek Harness](acp-programs.md).

## Before you start

You need to be a system administrator, with **Management** open on **Agent programs**. The reason for each extra is shown there, on the page of a build's checks.

## 1. Find the extras that are off

1. In **Management → Agent programs**, choose DeepSeek Harness and a build.
2. Open the build's status. Each extra that is off shows a reason beside it.

If the plugin file is missing, `dsh` runs as plain ACP: sessions, tool calls, approvals, cancel and resume work, and every extra is reported off.

## The extras and what each needs

| Extra | What it adds | What it needs from `dsh` |
|---|---|---|
| Live text | Text and reasoning as they are written | The agent loop's live stream |
| Steering | A message to a turn that is running, and a follow-up | The agent handle's `steer` and `followup` |
| Questions | `ask_user_question` as a question a person answers | The user-questions service and the question tool's row |
| Plan | Plan mode that refuses changes until the plan is accepted; the plan as a card | The plan-mode service and the tool pipeline's guards |
| Approvals | Asks before a command that writes, a file write or an edit | The approval service |
| Token breakdown | Fresh input, cache read and write, and output, per model call | The session log's usage records |
| Compaction phases | When `dsh` compacts, and a manual compaction | The compaction service |
| Skills | The session's skills, in `<home>/skills` or the directory Orch names | The skills registry |
| Fork | A copy of a session at the end of a turn | The agent registry and the session store |
| Carry history | A switched thread's history written into a new `dsh` session | The same |
| Shell and files in the environment | `dsh`'s commands and files run in the environment | The tools registry, and a launch that switches off `dsh`'s own tools |

## Why an extra is off

| Reason | Meaning | What to do |
|---|---|---|
| Switched off in settings | Orch's settings for the program turn it off | Change the setting |
| This build lacks what it needs | A service the extra uses is not in this `dsh` build | Use a build that Orch has checked, or leave the extra off |
| This build changed what it relies on | The service is there but behaves differently from what the plugin was written against | The same: `dsh` is a preview and its internal interfaces change between builds |
| Its own check failed | The extra's start-up check failed; the detail says what | Read the detail |
| It failed while running | The extra was on and raised an error; it stays off for the process | Read the detail. The thread goes on without it |
| This environment lacks what it needs | A directory or file the extra needs is missing, or the arrangement does not call for the extra (the environment is where `dsh` runs) | Read the detail |

`dsh` is a preview with announced breaking changes, and its plugin interface is not stable. Orch pins the build it ships and checks a newer build before an environment receives it. A build that changes an interface turns the affected extras off with their reasons. Plain ACP keeps working, and no extra that is on is ever broken.

## How the plugin is loaded

The plugin is one file, `orch-dsh-bridge.mjs`, kept beside the `dsh` build in the tool cache. Orch writes a patch file for each session and gives it to `dsh --patch`. The patch holds one row with the plugin's absolute path, and a row for `dsh`'s own `ask_user_question` tool, which the ACP profile does not load by itself. Nothing in `dsh`'s shared home is changed, and the plugin does nothing in a process that only prints (a help, a version, a configuration dump).

## Shell and files in the environment

When the environment is not on the machine `dsh` runs on (the separated arrangement), the plugin registers the tools the model uses to reach it (`bash`, `read`, `write`, `edit`, `glob`, `grep`, `job_output`, `job_kill`, `job_list`) under the names of `dsh`'s own. Each call goes to Orch's executor with the address and token Orch gives that session. Approvals and plan mode work as they do for `dsh`'s own tools. `dsh`'s own services keep working on the provider's machine: its home, its skills and its session files.

The launch must switch off the rows that hold the same tool names (`tool-bash`, `tool-fs`, `tool-fs-search`, `tool-jobs`, `tool-working-directory`) and set `backends.enabled`. When a name is taken, the extra reports itself off with the row to switch off. When `backends.enabled` is not set, it reports itself off as not applicable.

## What leaves the machine

Nothing but the model request. The plugin does not call out, and `dsh`'s own uploads (the session log and the plugin list sent with every request to DeepSeek's official API) are switched off in the patch and refused again by Orch's model forwarder.
