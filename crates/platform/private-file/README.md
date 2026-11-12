# orch-private-file

An immutable UTF-8 view over an anonymous process-private temporary file. It supports whole-text
algorithms without a heap copy of their complete input. Construction exposes only an opaque writer;
the file handle, path and mapping stay private. The one platform mapping boundary documents that
ownership proof. Errors, cancellation through drop and normal completion release the file and map.

This is ephemeral IO storage, with no business identities, persistence policy or authorization.
