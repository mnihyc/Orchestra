# orch-types

Shared primitive types without service or transport behavior.

Implemented. An identifier is a UUID of version 7; a moment is UTC and is
written with milliseconds; a hash is BLAKE3-256 in lowercase hexadecimal.
Time is read through a clock that tests move by hand. The tests state the
wire forms and the rules of a field's answer.

| Module | Planned responsibility |
|--------|------------------------|
| [id](src/id.rs) | Typed UUID identifiers. |
| [time](src/time.rs) | Time values and the controllable clock boundary. |
| [hash](src/hash.rs) | Content hashes and their representation. |
| [revision](src/revision.rs) | Revision and sequence primitives. |
| [fields](src/fields.rs) | The one shape in which a person is asked for something, used by sign-in methods, endpoint options and imports. |
| [measurements](src/measurements.rs) | One bounded aggregate measurement, shared by service exporters and the installation's stored minute samples. |
| [media](src/media.rs) | File media types and inert text classification shared by stored attachments and workspaces. |
| [schema](src/schema.rs) | Local JSON Schema checks, bounded compiled grammar reuse and field faults that omit supplied values. |

Schema validation keeps the same local-only references, linear regex engine and format checks on
every call. The shared helpers retain compiled validators in a least recently used process cache,
keyed by the exact complete schema and, for a local choice, its pointer into that schema. Editing
any schema content creates a different entry, including when a local rule's referenced definition
changes. Supplied instances and accepted or refused outcomes are evaluated afresh and never cached.

The standard cache retains at most 128 validators and 8 MiB of serialized schema and pointer keys.
The entry limit bounds retained validator overhead; the byte budget prevents a few large schemas
from occupying the entire source cache. These are reuse budgets, not configuration size limits:
oversized or evicted schemas still compile and validate normally. Embedders can use
`schema::ValidatorCache::new(schema::CacheLimits { entries, schema_bytes })` for an independent
cache, including zero entries to disable retention. A retained entry compiles once across concurrent
first users; compilation and instance evaluation run outside the cache mutex. Compilation failures
use the same retention limits and still expose only the existing redacted schema fault.
