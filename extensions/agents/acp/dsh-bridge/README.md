# orch-dsh-bridge

Orch's plugin for DeepSeek Harness (`dsh`). It runs inside the `dsh --profile acp` process and adds what dsh's Agent
Client Protocol server does not carry, so that a dsh thread behaves like a thread of Orch's other agent programs.

| Extra | What it adds |
|---|---|
| `streaming` | Text and reasoning deltas of each model call, as they arrive |
| `steering` | Steer a running turn, or queue a follow-up for after it |
| `questions` | dsh's `ask_user_question` as a question a person answers in Orch |
| `plan_mode` | Plan mode that refuses changes until the plan is approved, and the plan as a card |
| `approvals` | Asks before a change (a command that writes, a file write or edit) and lets reads through; Orch sets ask or bypass for each session |
| `usage` | The token counts of each model call: fresh input, cache read and write, output |
| `compaction` | The phases of a compaction, and a manual run |
| `skills` | The skills Orch keeps for the session, in `skills/<name>/SKILL.md` under dsh's home |
| `fork` | A copy of a session at a turn boundary |
| `native_carry` | Orch's neutral history written into a new dsh session, and dsh's own record written out |
| `executor_backends` | The model's `bash`, `read`, `write`, `edit`, `glob`, `grep` and job tools act in an environment of Orch's, through its executor (separated arrangement) |

The plugin announces which extras work in a `_orch/hello` notice after `initialize`. An extra that cannot work in a dsh
build is off with its reason, and the others keep working. Nothing here changes the standard protocol.

## Loading

The plugin is one ES module, built by `npm run build` into `dist/orch-dsh-bridge.mjs`. It is loaded by a patch file given
to `dsh --patch`, as a row with an absolute module path:

```yaml
- insert:
    - id: orch-dsh-bridge
      name: /absolute/path/to/orch-dsh-bridge.mjs
      config: {}
    - id: tool-ask-user          # the profile does not load dsh's question tool by itself
      name: '@deepseek-ai/dsh-tool-ask-user'
```

Nothing else of the profile changes. When its module is evaluated, which happens for every row before any row starts,
the plugin takes over `process.stdin` and `process.stdout`: dsh's ACP server then reads and writes through the plugin's
channel. Lines for `_orch/*` methods and the answers to the plugin's own requests are handled by the plugin, and every
other line reaches the server unchanged. A process that only prints (a help, a version, a configuration dump) is left
alone.

## Settings

Orch also tells the plugin about each session in `_meta.orch` of `session/new`, `session/resume` and `session/prompt`: the
approval mode, whether a turn is a planning turn, and the directory of the session's skills.

The row's `config` (all optional; every number is a default, not a limit):

| Key | Meaning |
|---|---|
| `extras` | `{ <extra>: false }` switches an extra off |
| `streaming.flushMs`, `streaming.flushChars` | How long and how much text a notice gathers |
| `planMode.readTools`, `planMode.commandTool`, `planMode.readCommands` | What plan mode lets run, and which commands count as only reading |
| `approvals.mode`, `approvals.askTools` | Whether a session starts asking before changes (Orch's `_meta.orch.approval_mode` and `_orch/approval/set` override it), and for which tools |
| `backends.enabled` | `true` in a separated arrangement (the launch also switches off `tool-bash`, `tool-fs`, `tool-fs-search`, `tool-jobs`, `tool-working-directory`); `backends.url`, `backends.tokenEnv` an address for sessions without `_meta.orch.exec`; `backends.jobWaitMs`, `globMax`, `grepMax` |
| `hostVersion` | The dsh version, when the caller knows it |

## Messages

`src/protocol.ts` defines every message and its fields. All methods start with `_orch/`.

## Build and test

`dist/orch-dsh-bridge.mjs` and its `.sha256` are generated and committed: the Rust adapter embeds the bundle and writes it into each session's home.
`node build.mjs` writes them; `npm run check` (which CI's web job runs, since it needs Node and the frontend's bundler) builds again somewhere else and fails if either committed file differs, then type-checks.
Change `src/`, run `node build.mjs`, commit the three files together.

```
npm run typecheck
npm run build
ORCH_DSH_BIN=/path/to/dsh-runtime npm test            # a native executable, or
ORCH_DSH_NODE_ENTRY=/path/to/lib/bin.js npm test      # the npm build
```

Without either variable the cases that start a real dsh are skipped. Each case runs against a scripted model server in
`tests/support`.

## Pinned builds

The extras were written against dsh 0.2.1-rc.2 and 0.2.1-rc.4 and use dsh's internal plugin API (the services named in
`src/host.ts` and the events the extras listen to). dsh's API is not stable between builds; a build that changes it
turns the affected extras off with their reason, and the standard protocol keeps working.
