//! The only platform mapping boundary; its caller retains the unique private backing handle.
use std::{fs::File, io};
pub(super) fn map(file: &File) -> io::Result<memmap2::Mmap> {
    // SAFETY: the sole caller creates an anonymous process-private tempfile, hides its handle
    // behind an opaque Write sink, flushes and drops that sink before mapping, and never exposes
    // any handle/path/mutable map afterwards. The caller owns the backing file until after the
    // read-only map drops, so no safe caller can truncate or modify the mapped bytes.
    unsafe { memmap2::MmapOptions::new().map(file) }
}
