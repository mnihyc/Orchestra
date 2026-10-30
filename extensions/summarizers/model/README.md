# orch-summarizer-model

Model-backed title, description and tag generation selected by configuration.

The built-in model producer frames redacted values and activity as material,
calls the host's permission-checked API provider, and returns bounded text or
tags with actual usage. Its tests cover whole prompts, hostile material,
malformed answers and handoff bounds; the engine owns scheduling and writes.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Configuration schema and supported fields. |
| [prompts](src/prompts.rs) | Configurable prompt defaults. |
| [completion](src/completion.rs) | Provider-complete request and constrained output conversion. |
