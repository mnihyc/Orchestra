# Claude Code protocol compatibility evidence

Each version directory contains the 55 required support decisions plus one late-steer supplement, with 53 supported cases. The three
unsupported cases are separate Codex checkpoint, prompt-user-rule and network approval protocols;
Claude's actual `can_use_tool` command, file, generic permission and MCP permission cases remain covered.
No case is silently skipped. The six tolerance cases are supported for both versions.

These fixtures were written from the published `@anthropic-ai/claude-agent-sdk` packages 0.3.312 and
0.3.319. Each manifest records its immutable tarball URL and SHA-256. They are **published protocol**
evidence, not recordings from either installed program. The independent consumed-shape and managed-pin
schema snapshots live under the adapter's `tests/fixtures/protocol/`; their extractor and source hashes
remain the authority for schema drift. Existing 2.1.296 native recordings keep their original identity.

`extensions/agents/claude-code/tests/upgrade_compatibility.rs` selects the actual consumer for each case:

- Ordinary messages exercise the public frame parser and `EventMapper`, with the shared manual clock.
  Tool cases require the native name, full literal JSON input, full literal output and neutral tool kind.
  Compaction retains automatic/manual/unknown provenance, including an idle boundary; failure cases
  require the native classification and retryability, not just an error-like sentence.
- Lifecycle, approvals, interruption, late steering, process loss, settings readback and tolerance
  exercise the actual conversation actor and production bounded process streams. The launcher supplies
  byte pipes only. The common corpus helper owns both-direction matching, request-id bindings and clock
  movement. A real observation hook request/reply supplies the final native barrier. Unexpected later
  native input fails the fixture; shutdown must close the same process's stdin.
- The canonical steering case captures the actual user UUID, requires its exact native echo and a
  successful return, and forbids cancellation. The late-steer supplement covers a turn ending before
  acknowledgement: it requires the real queued outcome and cancellation of that exact native message UUID. End waits for the actual stopped task, not its control acknowledgement.
- Session close and unload share the same conversation Close command used by the adapter's resource
  close path. No fictitious native unload method or session-authority event is introduced.

Every expected event uses an explicit subset for stable semantic members. Irrelevant event kinds are
listed individually; item/error/approval/terminal counts and order remain strict. Neither raw logging
nor an invented turn is needed for an idle observation. Unknown private bodies and settings sources
never enter the expected public history. The ten-second waits are test deadlock guards, not product caps.

The current 2.1.319 corpus has 55 managed-pin entries, including the managed feature-flags
switch; the older 2.1.312 dialect has 54. Its readback explicitly reports the missing newer switch as
incomplete, and runtime policy refuses that older build. Every declared pin is reported changed in
`automatic_divergence`, and every one must produce its own completed
`harness_diverged` error through the real settings reader. The test also compares each expected value
with the actual launch settings. Native background work is explicitly enabled for an issued thread
whose background-wake setting is on; its unrelated automatic and MCP background switches remain pinned.
Optional prompt-cache lifetime pins stay covered by their separate configured/unset launch tests.

Protocol replay does not qualify an installed binary or broaden its accepted version range. Native
startup, real settings readback and provider qualification are separate controls.

Run centrally:

```sh
cargo test -p orch-adapter-claude-code --test upgrade_compatibility --test upgrade_tolerance
cargo test -p orch-adapter-claude-code --lib published_protocol_tests
cargo test -p orch-adapter-claude-code --test sessions --test background_control --test version
```
