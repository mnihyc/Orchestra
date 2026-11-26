# Run DeepSeek Harness

Make DeepSeek Harness available on your hosts and machines, keep its pinned build, and know what leaves the machine. At the end threads can use DeepSeek Harness, and you can read why one that does not start fails.

DeepSeek Harness is the program of the Agent Client Protocol that ships with Orch (version 1, over the program's standard input and output). Its extras are on their own page: [Read why a DeepSeek Harness extra is off](deepseek-harness-plugin.md). How builds are fetched and checked is on [Choose where agent program builds come from](agent-programs.md). People use it as described in [DeepSeek Harness](../user/deepseek-harness.md).

## Before you start

You need:

- A provider the program can use, and an environment with a key for it: DeepSeek, OpenRouter, or an OpenAI or Anthropic endpoint.
- Hosts that can fetch the build, or a mirror: the first use downloads a wheel from `files.pythonhosted.org`.
- Disk: the wheel is about 95 MB and about 400 MB once unpacked, and a host keeps one copy for each version.

Nothing else is needed on a machine. The wheel (`deepseek-harness-runtime-bin` on PyPI, one for each processor) holds one program, and Orch unpacks it into the tool cache after verifying the SHA-256 the index keeps.

## 1. Check the pinned build

`deploy/harness-builds.json` pins the version a release ships (`0.2.1-rc.2`, which PyPI writes `0.2.1rc2`) with the address and SHA-256 of each wheel. To move the pin:

1. Open `https://pypi.org/pypi/deepseek-harness-runtime-bin/<version>/json` and take the file's address and SHA-256.
2. Download the file and work out its SHA-256 yourself.
3. Change the entry in `deploy/harness-builds.json` only if the two agree.

## 2. Use it

Make a thread with DeepSeek Harness as its agent program. Each thread gets its own process, in a directory of its own.

**What you will see.** The thread starts, answers, and shows the program's extras that are on. An extra that is off shows a reason ([Read why a DeepSeek Harness extra is off](deepseek-harness-plugin.md#why-an-extra-is-off)).

**How a session runs.** The adapter starts the process with the address of the thread's model proxy, a token that is the connector's (never a model key), the model list, the approval words, and the variables named in the program's launch data. Nothing else passes from the connector's environment except the names the launch data lists (`PATH`, `LANG`, `TZ`, the certificate paths). Orch writes the program's configuration file at every start, and the key stays in Orch: the program's own sign-in is not used.

Orch turns off the program's own goal service, round driver, command and tools. It supplies its own `goal` tool over the session's authenticated MCP address, so every goal continuation passes Orch's normal turn checks.

## What leaves the machine

- The program is set not to send its session log or its plugin list, and not to report telemetry.
- Orch's model proxy is the second lock. It removes the headers that name the client and refuses a request that carries either upload, in full, with `request_refused` (403) naming the member. Nothing is sent.
- The check of the program fails it when a model request carries an upload or a start lacked the telemetry switch.

## If a thread cannot start

| What the thread says | What to do |
|----------------------|------------|
| The environment has no key for a provider the program supports | Give the environment a key for DeepSeek, OpenRouter, or an OpenAI or Anthropic endpoint |
| The build is not installed on this machine | Let the node fetch it (the host must reach `files.pythonhosted.org`), or point `agents.harness_mirror` at a mirror |
| The program did not answer `initialize` | Read the last lines the program wrote on its error output, which the message holds |

A failed history import is reported separately from an unavailable program. Retry the switch with **Summary** or **None**, or inspect the recorded refusal and the program's history support. A whole import omits `throughTurn`; the bridge also accepts `null` as whole history; an integer limits the import to that many turns.

## If a session is lost

| What the thread says | What to do |
|----------------------|------------|
| The agent program lost the session, and says how it ended (an exit code, or the signal that ended it) | The program's process ended while the session was open. The message holds how it ended and what the program wrote on its error output since the session was up (what it wrote while it started is left out, because it is not why it ended). The next message starts the program again and resumes the conversation |

## Add another program of this protocol

A program of this protocol is a manifest (`protocol: "acp"`) and, for what the manifest cannot say, launch data. With the manifest alone Orch starts `launch.programs[0]` with `launch.args` and `launch.env`, writes `launch.files` into the session's home, and drives the program with the protocol's defaults: one approval mode, and the tools named as the program names them. The launch data of a shipped program (`extensions/agents/acp/programs/<kind>.json` in the source) carries the rest: how a model is named, how approval modes map to the program's words, tool names and families, what must not be sent, and the plugin to load.
