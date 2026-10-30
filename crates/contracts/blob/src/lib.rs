//! BlobStore trait seam; local storage is the v0.1 implementation.
//!
//! Files, session archives, snapshots, attachments and build contexts are kept as blobs: content named by
//! its hash. The store is the seam [`store::BlobStore`] (put, get with a range, exists, size, delete), so that
//! a store in the cloud needs no change to the schema or to what refers to a blob. The arithmetic of a range
//! read is here, once ([`range`]), together with the check that content is what its name says.

#![forbid(unsafe_code)]

pub mod range;
pub mod store;
