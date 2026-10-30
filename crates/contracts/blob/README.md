# orch-blob-contract

BlobStore trait seam; local storage is the v0.1 implementation.

Implemented: the `BlobStore` trait with the five operations (`put`, `get` with a range, `exists`,
`size`, `delete`), the errors a store answers with (`BlobError`), the check that content is what its name says
while it passes (`Verifier`), the path a blob has below the storage root (`relative_path`), and the range
arithmetic of a read once for every store (`ByteRange` read from an HTTP `Range` header, `ResolvedRange`, the
`Content-Range` texts). The local directory store is written after this package, against the semantics the
tests here state with a store in memory. No I/O is done here.

| Module | Planned responsibility |
|--------|------------------------|
| [store](src/store.rs) | Content-addressed put, get, exists, delete and size operations. |
| [range](src/range.rs) | Range reads and content metadata. |
