# Programs of the Agent Client Protocol

How Orch runs a program that speaks the Agent Client Protocol (version 1, over the program's standard streams), and what
the operator has to know of DeepSeek Harness, the one that ships. The plugin that runs inside it has a page of its own
(DeepSeek Harness plugin); how builds are fetched and checked is on [Agent programs](agent-programs.md).

## What ships

| Program | Kind | Builds | Needs on a machine |
|---------|------|--------|--------------------|
| DeepSeek Harness | `dsh` | The platform wheel of `deepseek-harness-runtime-bin` on PyPI, one for each processor, verified by the SHA-256 the index keeps | Nothing: the wheel holds one native program, and Orch unpacks it into the tool cache |

`deploy/harness-builds.json` pins the version a release ships (`0.2.1-rc.2`, PyPI's `0.2.1rc2`) with the address of each
wheel and its SHA-256. To move the pin, take the file's address and SHA-256 from `https://pypi.org/pypi/deepseek-harness-runtime-bin/<version>/json`,
download the file, work out its SHA-256 yourself, and change the entry. The wheel is about 95 MB and about 400 MB
once unpacked; a host keeps one copy for each version.

## How a session runs

The adapter starts one process for each thread, in a directory of its own, and gives it: the address of the model
proxy of the thread and a token that is the connector's (never a model key), the model list, the approval words, and
every variable of its own, named in the program's launch data. Nothing passes from the connector's environment except
the names the launch data lists (`PATH`, `LANG`, `TZ`, the certificate paths). The program's configuration is a file
Orch writes at every start. Its sign-in is not used: the key stays in Orch.

The managed patch disables dsh's native goal service, round driver, command and tools. Orch supplies its own `goal`
tool over the authenticated session MCP address; every goal continuation passes Orch's normal turn checks.

## Privacy

- The program is set not to send its session log or its plugin list, and not to report telemetry.
- Orch's model proxy is the second lock: it removes the headers that name the client and refuses, whole, a request
  that carries either upload. A refusal is answered with `request_refused` (403) and names the member; nothing is sent.
- The build check for a program fails it when a model request carries an upload or a start lacked the telemetry switch.

## Adding another program

A program of this protocol is a manifest (`protocol: "acp"`) and, for what the manifest cannot say, launch data. With
the manifest alone Orch starts `launch.programs[0]` with `launch.args` and `launch.env`, writes `launch.files` into the
session's home, and drives the program with the protocol's own defaults: one approval mode, the tools named as the
program names them. The launch data of a program Orch ships (the file `extensions/agents/acp/programs/<kind>.json` in the
source) carries the rest: how a model is named, how approval modes map to the program's words, tool names and
families, what must not be sent, and the plugin to load.

## If a thread cannot start

| What the thread says | What to do |
|----------------------|------------|
| The environment has no key for a provider the program supports | Give the environment a key for DeepSeek, OpenRouter, or an OpenAI or Anthropic endpoint |
| The build is not installed on this machine | Let the node fetch it (the host must reach `files.pythonhosted.org`), or point `agents.harness_mirror` at a mirror |
| The program did not answer `initialize` | Read the last lines the program wrote on its error output, which the message holds |

A failed history import is reported separately from an unavailable program. Retry the switch with Summary or None, or inspect the recorded native refusal and the program’s history support. Whole neutral import omits `throughTurn`; the bridge also accepts `null` as whole history. An integer limits the import to that many turns.

## If a session is lost

| What the thread says | What to do |
|----------------------|------------|
| The harness lost the session, and says how the program ended (an exit code, or the signal that ended it) | The program's process ended while the session was open. The message holds how it ended and what the program wrote on its error output since the session was up; what it wrote while it started is left out, because it is not why it ended. The next message starts the program again and resumes the conversation |
