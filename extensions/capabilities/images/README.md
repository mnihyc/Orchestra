# Image capability interfaces

This extension contributes image bindings to the common capability registry. It installs no
executor and grants no provider, file or billing authority. `declarations()` is registered by
API and connector composition; account offers stay disabled until a manager enables them.

`decode` and `ImageStream` interpret the selected binding through the common projector. They
return actual bounded bytes and reported metadata/usage, without following response URLs or
guessing missing values. Equal buffered outputs retain their order and multiplicity. A stream
distinguishes previews from the first completed result; an identical final replay emits no
second result. Call-local stream state is not durable acceptance.

Call `finish()` when the transport ends, after passing its parsed events to `accept()`.
An empty or preview-only EOF closes the invocation with an error; a later frame cannot
make it complete. `finish()` is a terminal operation, so it must not be used to probe
whether more events are needed. Once a valid final has been emitted, a trailing failure
does not revoke its bytes or observed usage, and an identical final still emits nothing.

The existing service execution producer must supply the selected current route and its actual
source, recheck provider Use and offering lists/caps at its normal effect boundary, retain
observed work with the same call identity, store output through the existing blob writer,
and deliver it through the actual guarded environment destination writer. A file-publication
retry uses those retained bytes and must not generate or bill again. Reported total cost takes
precedence over an estimate. Native preview token cost and completed-image count are distinct.

Provider differences are data in their own modules. The compatible wire interface has an
independent bundle, so removing an account extension does not remove another account's format.
Multipart edits and streaming annotations do not imply that a transport is installed. The
common resolver may offer a route only after its real executor/interface is installed.

Central deciding cases: `cargo test -p orch-image-backends --lib`. No local run is claimed.

The managed `image_generation` tool consumes these bindings through the common paid runner.
It takes `prompt`, `output_paths` (one relative workspace filename per requested image), and
optional declared backend `options`. An API model comes from the selected offering. A hosted
model's image-tool options are validated and applied by its installed binding; they never
replace that tool's identity. Destination paths are not provider input.

Each completed output becomes an ordinary retained image item before optional file delivery.
Equal pixels from distinct output rows remain distinct items. A failed or conflicting file
does not erase an image or undo observed usage. With the returned `delivery_ref` and new
`output_paths`, the tool delivers those same retained bytes and never generates or bills again.
It replaces no different existing file. Pending accepted output that could not yet be decoded
or stored can be retried by that reference within its original execution. A later execution
may deliver already retained image artifacts only with its current result and destination access.

The native file command retains the actual tool source, input callers, original and observed
restrictions, exact blob/path, physical workspace and reply budget in the existing private
command journal. Queue and send repeat those checks. Input-image and multipart edit bindings
require their actual input materializer; an unsupported encoding is refused before spending,
and inputs are never silently omitted. Service eligibility, Use, caps, recovery and measured
accounting remain the common runner's responsibility.

`images` names source images in the thread's workspace. `mask`, where the chosen edit binding
declares it, names a mask there too. The native reader verifies the full uploaded bytes, retains
the images as ordinary protected items, and freezes the exact references for that logical call.
Every stat/fetch repeats the original source, physical workspace, current callers and budget at
queue/send. A model receives those exact image parts after its declared prompt; multipart
fields consume their request-local names. No local path or source URL reaches the provider.

The pure `materialize_inline_inputs` codec handles an extension-declared `x_input_images`
JSON parameter. It renders data URLs from verified bytes through the common template engine;
it opens no URL and creates no client, permission or ledger. Its allocation/output bound is
supplied by the consumer's existing request frame budget. The keeper receives complete JSON,
with no unresolved image part. The reference-array mapping follows the
[provider's documented input shape](https://openrouter.ai/blog/tutorials/image-generation/).
