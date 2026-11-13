# Native transition regression corpus

`captured-open.json` and `captured-settings-status.jsonl` are safe projections of the same Codex 0.162.1 native capture as the [recorded request corpus](../performance/README.md). They retain the actual opening model, settings/status shapes, startup ordering and scoped MCP ownership. Thread ids and server names are fixed fixture references. Paths, instruction bodies, process/account data and unrelated native fields are omitted. These are deliberately projections, not a complete copy of the native session.

`schema-derived-boundaries.jsonl` is constructed from the official Codex 0.165.0 app-server TypeScript declarations. It covers the mismatch, failed/scopeless MCP startup, hook, streaming plan, both review verdicts, reroute, marker-only compaction and autonomous close boundaries that are absent from that capture. It contains disposable names/text, no login or credential, and does not claim a native observation of those cases. The hook label comes from its run metadata; prompt text is a negative disclosure sentinel. An absent compaction trigger remains absent.

The adapter replay uses the ordinary fake RPC pipes and both files, preserving each source item through its existing lifecycle. Marker and error results are read as their public API types. Engine ingestion/audit and bilingual timeline tests verify the next boundaries. Native conformance remains the separate pinned-binary proof; a constructed fixture cannot establish a build capability. The program manifest's existing interface checks include every consumed notification.

Schema source: https://github.com/openai/codex/tree/rust-v0.165.0/codex-rs/app-server-protocol/schema/typescript/v2


`0.167-approval-callbacks.json` is a constructed projection of the published 0.167.0 and 0.167.2 command approval schema (the same): two distinct human checkpoints on one parent item, followed by a managed-network prompt. It has no legacy `availableDecisions`. The replay asserts fresh person decisions in Ask, Auto and Bypass, and exact native reply routing. This is schema-derived coverage, not a claimed runtime capture of a new build.
