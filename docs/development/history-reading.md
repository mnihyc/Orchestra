# Full history suppliers

Before-import records use an owned page cursor through the same selected continuation and retained
revision decoder as other history. There is no synthetic turn for those records. An empty page ends
the cursor; a shorter byte-buffered page is followed by another page.

The forward and backward readers keep their ordinary turn grouping, empty turns and loose-line
ownership. Unfinished turns and pending loose lines spill to a private reversible spool. The memory
buffer holds at most one ordinary 500-row page and 32 MiB of serialized rows. SQL decoding uses the
same 32 MiB payload target plus its boundary item, so one large item always makes progress. These
are buffering thresholds, not content limits. A single stored JSON item remains an atomic decode;
payloads do not move to chunk storage.

The written form has one incremental turn renderer. Its existing string API and file-backed history
consumers use that same renderer. Only author/reference metadata and bounded index previews stay
beside a page. Rendered text spills at the existing 64 KiB transcript copy quantum. A bounded carry
reads only its admitted UTF-8 prefix from that text, and the existing turn-read and handoff limits
continue to apply. Explicit file exports retain complete content.

Redaction preserves ordered literal-secret and regular-expression replacements over complete text,
including arbitrary multiline matches. It uses immutable process-private file-backed views and
writes each replacement pass to another private file, then applies the same assignment-word rule.
No chunk overlap estimate or line boundary changes a match. The platform primitive creates its own
anonymous file, exposes only an opaque writer during construction, validates UTF-8, and retains the
private file and read-only map until drop. The control crates continue to forbid unsafe code; the
mapping primitive confines its single documented unsafe operation to its platform module.

All spools are ephemeral and close on completion, error or cancellation. History membership,
retained source authority, artifact custody and publication remain with their existing consumers.
