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
