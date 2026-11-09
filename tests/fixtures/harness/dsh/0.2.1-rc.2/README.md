# DeepSeek Harness 0.2.1-rc.2 over native ACP

These are authored protocol fixtures, not vendor-process recordings. The replay
uses `orch_testkit::harness_corpus::replay_io` and the actual `AcpAdapter` launcher,
RPC parser, session actor and approval response path. No vendor executable, model
key or network service is needed. All 55 required scenarios have a support decision;
the native surface has 34 supported scenarios and 21 explicit unsupported reasons.
Five additional cases cover compatible initialization additions, refused or invalid
versions, and an unknown idle update. None of the six tolerance cases is skipped.

The source is the immutable dsh release
[`687f282e8625c6b8d11ff58211a90524be2e4317`](https://github.com/deepseek-ai/deepseek-harness/tree/687f282e8625c6b8d11ff58211a90524be2e4317/packages/acp/acp),
whose `package.json` pins `@agentclientprotocol/sdk` **1.4.0**. The downloaded
[SDK package](https://registry.npmjs.org/@agentclientprotocol/sdk/-/sdk-1.4.0.tgz)
has SHA-256 `57beb0f7705b09406e5bcc984d1f6a141940680b4c42755be026f77f64365a37`;
its `schema/schema.json` has SHA-256
`7f77702b34e0a0558e77220e9007bf8ee161a976bb8ac5021aba1b7e7b2c5708`.
The Python index spelling `0.2.1rc2` is the same release identity as the corpus's
`0.2.1-rc.2`; the [runtime wheel metadata](https://pypi.org/project/deepseek-harness-runtime-bin/0.2.1rc2/)
is not evidence of a local program execution. The native server's own
`agentInfo.version` is `0.0.1`; it is not relabelled as the runtime version.

The release's native ACP intentionally omits presentation hooks, structured
questions, fork, steering and other UI features. Their unsupported entries explain
that distinction. Orch's companion protocol restores additional features; its
existing `dsh-bridge/tests` and native `dsh_plugin` suite qualify that separate
implementation. This corpus disables the companion in its test configuration so
that a companion response cannot be misreported as a vendor-native ACP feature.
It changes no production capability or launch. A generic `todo_write` is retained
as the native tool's actual Other payload, rather than inventing a plan event.

ACP's [initialization contract](https://agentclientprotocol.com/protocol/v1/initialization)
announces one integer **major**, not a dotted schema release. Compatible optional
fields and capabilities on major 1 are accepted; selected major 2 is refused with
both versions in the reason, as are missing or malformed versions. The fixture's
new capability/optional-field injection represents a compatible minor addition;
it does not invent an announced `protocolVersion: 1.5` or treat the program's
version as the protocol's version.

All native requests and replies are checked. Expected results select stable
payload members explicitly, with exact event order/count and array lengths.
The ignore list names only ordinary transport/status, growing-item phases and
separately tested model-request accounting; completed items, errors, facts,
approvals and turn outcomes are compared. Raw logging is disabled. Unknown
updates and methods must produce counted passive markers, including idle; a
future permission option cannot become an implicit grant. With no recognized
answer, a generic card offers cancellation and the actual protocol receives
`outcome: cancelled`. Malformed text must reach an error item and the next valid
frame still runs. Native process exit preserves partial output and session loss.

Managed goal service/driver/command/tool pins share the observable violation:
model output arriving without an Orch prompt. It reports `harness_diverged` and
starts no work. The existing manifest tests also check all four launch pins by
name. The privacy rows and telemetry pin are checked in their existing launch /
forwarder tests; no nonexistent native telemetry notification is invented here.

Run centrally:

```sh
cargo test -p orch-adapter-acp --test upgrade_compatibility
cargo test -p orch-adapter-acp --lib
cargo test -p orch-adapter-acp --test turns --test process --test generic_suite
```

The live `dsh_real`, `dsh_plugin` and `dsh_mcp` suites still require the integrator's
explicit native harness fixture; offline schema evidence does not replace them.
