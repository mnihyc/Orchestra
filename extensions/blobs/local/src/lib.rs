//! Local directory implementation of content-addressed storage.
//!
//! [`LocalBlobStore`] keeps every blob as one file, named by the BLAKE3 hash of its content, at
//! `<root>/blobs/<hex[0:2]>/<hex[2:4]>/<hex>`. A write goes to a temporary file below `<root>/tmp`, is checked against the
//! hash and the size it claims while it passes, and is moved into place in one step, so a name never holds anything but
//! the content it names, whatever happens to the writer. Reads give the bytes as a stream, whole or by a range.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`store`] | The store, its options and the operations that need no content moved |
//! | [`ranges`] | Reading, whole or by a range, as a stream |
//! | [`writes`] | Writing and removing |

#![forbid(unsafe_code)]

pub mod ranges;
pub mod store;
pub mod writes;

pub use store::{LocalBlobStore, StoreOptions};
