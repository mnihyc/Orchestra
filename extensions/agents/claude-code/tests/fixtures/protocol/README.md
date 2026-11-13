# Published protocol evidence

These are functional declaration names and primitive type facts extracted from the
published Agent SDK archives for three program builds. They are **not native
recordings**. They prove names and public shapes, not that a running program honors
a setting. Native conformance and effective-settings readback remain required.

| Program | SDK | Archive SHA-256 |
| --- | --- | --- |
| 2.1.312 | 0.3.312 | `98a6c27b00b273fa0598b80540aa152d9fb58b6a1ac885b5993381e98469926d` |
| 2.1.319 | 0.3.319 | `6bfb9ccb6a5d15686f5c7dd12d7a7da6d7b486ca6e1331ac8a7ab98a2d302001` |
| 2.1.322 | 0.3.322 | `5db6881a4da75539d8b090f0a4b791043f9cbdbae439e5f9cf27356b28c59a93` |

Each snapshot records the immutable package URL, SDK version, archive hash,
declaration-file hash and runtime-source hashes. It retains field optionality,
literal discriminators and primitive kinds for the consumed message/control types
and Settings. Unrelated account-plan types are excluded; these are no account observations.
Referenced imported types remain references. A managed setting omitted from the
public interface must have an explicit recognized runtime schema in a named,
hashed package source file; ordinary symbol presence does not prove its type.
The extractor does not guess imported types or implement another native parser. Runtime symbol presence checks only
the managed environment variable names; it is not evidence of their runtime effect.
No vendor implementation or prose is copied into the snapshots.
The conditional feature-flag qualification switch is recorded separately: it is
absent in SDK 0.3.312 and present in 0.3.319. A name check does not qualify it for
production without the native conformance run.

The adapter's tests compare its actual managed settings/environment and real frame
and control-envelope helpers against this independent source evidence. A removed
or renamed consumed field fails the drift guard. Unknown optional fields remain
harmless. These checks supplement the shared bidirectional replay corpus, whose
recorded and published-protocol provenance must remain distinct.

To refresh a snapshot, obtain the named archive from its `evidence.references` URL,
verify the archive hash, then run this source-only extractor from the repository:

```sh
python3 extensions/agents/claude-code/tests/fixtures/protocol/extract.py \
  --package .tmp/sdk/claude-agent-sdk-0.3.319.tgz \
  --native-version 2.1.319 --sdk-version 0.3.319 \
  --output extensions/agents/claude-code/tests/fixtures/protocol/2.1.319/sdk.json
```

The extractor reads archive members without extracting paths, installing anything
or executing vendor code. Record the source review and run the adapter's protocol,
pin and replay tests centrally before changing the supported candidate.
