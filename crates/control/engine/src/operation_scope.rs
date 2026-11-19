//! Engine preparation helpers over the shared transport-neutral operation binder.

pub use orch_operation_scope::*;

/// Current account second-step requirement shared with the HTTP sign-in hold.
pub mod account_policy;
/// Stored-byte source witnesses shared by requests and continuing work.
pub mod blob_sources;
/// Typed native current-source checkpoints for exact captured operations.
pub mod continuing;
pub mod creator_files;
pub mod creators;
/// Actual raw request credential association at native preparation and dispatch boundaries.
pub mod request_origin;
/// Current placement facts shared with the ordinary HTTP target resolver.
pub mod targets;
