# Versioned Codex protocol source fixtures

The adapter's schema/<version> directory holds the native program's published generated JSON schemas, downloaded unchanged from the exact source commits named in each sources.json. They are source evidence, not program recordings, and no local program/schema generator was run. The upstream license is retained in UPSTREAM-LICENSE.txt.

The versions are the upgrade baseline 0.165.0, the previous shipped 0.167.0 and the shipped 0.167.2 (its app-server schema is byte-identical to 0.167.0's; its config schema adds the two feature keys incremental_tools_legacy and incremental_tools_legacy_gcp). app-server.schema.json is the published aggregate app-server protocol; config.schema.json is the native configuration contract. sources.json records every file's immutable source and SHA-256. The integration test verifies those bytes offline.

The fixture contracts consumed-fields.json and managed-pins.json refer to those public type declarations. consumed-fields.json lists concrete fields the RPC/client, event mapper and approval actor consume from native methods. upgrade_schema.rs checks those paths through the schemas' references and unions. Negative controls remove a real consumed field and rename a real request method; both must fail. The actual lite-RPC reader's literal field reads are checked against both native envelope definitions as well. This contract checks the inventoried consumed fields; it does not claim to infer every dynamically named JSON access in the adapter.

managed-pins.json freezes all thirty actual automation pins. The test compares the whole inventory with the same production declaration used at process and thread launch, checks each native spelling, and checks actual configuration values after a profile attempts to override them. plugin_discovery_prefetch was introduced in 0.167.0; the older schema must not pretend it exists. The current adapter correctly refuses to drive 0.165.0. Protocol compatibility replay and native build acceptance are separate assertions; older protocol data never turns that refusal into a supported old binary.

The complete two-direction replay corpora live under the shared tests/fixtures/harness/codex directory when their runner and expected events are committed. Schema checks here are one part of that qualification, not a claim that replay, every control actor or native execution has passed.

Integrator command, with no vendor process or network required:

```sh
cargo test -p orch-adapter-codex --test upgrade_schema
```
