# Native harness conformance

The integration runner executes `bash scripts/harness-canary/conformance
--record ./evidence/conformance` after building the candidate. Set the ordinary
shared test gates and target directory in that runner. The writer does not run
this command. `--image` selects the candidate image; expected exact versions
come from `ORCH_EXPECTED_CODEX_VERSION` and `ORCH_EXPECTED_CLAUDE_VERSION`.

Each native test uses a networkless container. A mounted Unix socket relays
only to the scripted model's loopback service. The harness has no Internet
interface, uses a disposable home/workspace and a synthetic key, and its actual
version and safe case evidence are retained. A missing image is an unrun check.
Status 0 means every recorded native case passed, 1 means a test failed, and
2 means prerequisites or native evidence are incomplete.

The case bodies are shared with the node through `orch-conformance::native`.
The node carries its runner and the scripted model in the static node program;
its driver makes a fresh networkless unit for a verified cached build. No image
or testing dependency is downloaded by that check. Individual results stay
with the build, and missing native evidence fails certification. Both harnesses
also check that a tool answer reaches the model in one text or structured form,
without a duplicate.

Command approvals must wait without effects and a denial must prevent the
write. Person questions must arrive from the native process and accept answers
by question id. A tool returned by a scripted app-server counterpart remains
fixture evidence until the real binary emits it. In particular, an MCP
elicitation and a nested Codex agent need the loaded configuration that offers
them; a catalogue mapping or an accepted launch flag alone cannot certify them.
The report records a required native case as failed when its behavior is absent.

The Codex elicitation test loads the `questions` MCP server through the session's
real HTTP MCP configuration. It streams an actual `elicitation/create` request,
waits for the real client response and compares the chosen content. The model
relay remains fixed to the scripted server and other paths are refused. A
separate native `spawn_agent` attempt checks parent/child stream correlation.
Required native observations fail when the behavior is absent. Codex's unused
dynamic-tool surface is a separate informational probe: Orch's declared tools
use MCP, whose one-form result case remains required. The node's internal runner
can run that probe explicitly with `--probe dynamic_tools`; the background
certification command does not request it or use it to disable a feature.

Before expanding a supported version range, attach passing evidence for each
newly advertised version. Keep the current exact pinned-version record and
separate older observed runs; never infer that every intervening version ran.
The failure canary remains a separate check of failure classification.

For a Claude fast-mode cooldown, a provider manager uses a disposable thread
on their own plan, enables fast mode for an eligible model, and starts a turn
while the provider reports cooldown. Record the build, model, requested tier,
safe result frame containing `fast_mode_state`, the displayed ordinary-speed
notice, usage and final status. Omit credentials, token files and authorization
headers. This state cannot be manufactured by a scripted model: the mapper's
cooldown frame test is labelled fixture evidence until such a capture exists.
