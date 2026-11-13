# Experimental terminal controls

The published stable schema in this directory is unchanged. The adjacent
`experimental-controls.json` retains the two consumed request variants observed
from the 0.167.2 binary's `app-server generate-json-schema --experimental` output.
The adapter initializes that same experimental interface. This is a method
surface for the exact build, not a complete replacement schema or a version-range
assumption. Its parameter references keep their native `v2/` namespace.

The program's public protocol source declares the same methods and request
fields: `thread/backgroundTerminals/list` takes `threadId`, optional `cursor` and
`limit`; its response returns `data` and `nextCursor`. Each terminal identifies
both its item and process. `thread/backgroundTerminals/terminate` takes `threadId`
and `processId` and reports `terminated`. The source is the commit recorded by
`sources.json`, in `codex-rs/app-server-protocol/src/protocol/common.rs` and
`codex-rs/app-server-protocol/src/protocol/v2/thread.rs`.

The native schema case compares the consumed method variants with a fresh
generation. The byte replay tests the initialized interface, list/re-list/exact
End, native completion and an unverified build that sends no control probes.
An acknowledgement never fabricates a command completion. The real process case
still requires the command's reported end and the actual process to disappear
within its existing ten-second bounds. Unknown builds remain unavailable until
their own declarations are verified.
