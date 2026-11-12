//! Shared business-operation admission for an existing caller's key restrictions.

#[path = "activity.rs"]
pub mod activity;

#[path = "activity_store.rs"]
pub mod activity_store;

#[path = "admin.rs"]
pub mod admin;

#[path = "admission.rs"]
pub mod admission;

#[path = "background_read.rs"]
pub mod background_read;

#[path = "blob_sources.rs"]
pub mod blob_sources;

#[path = "completed_projection.rs"]
pub mod completed_projection;

#[path = "creation_stage.rs"]
pub mod creation_stage;

#[path = "credentials.rs"]
pub mod credentials;

#[path = "direct.rs"]
pub mod direct;

#[path = "native.rs"]
pub mod native;

#[path = "publication.rs"]
pub mod publication;

#[path = "resolver.rs"]
pub mod resolver;

#[path = "snapshot_read.rs"]
pub mod snapshot_read;

#[path = "targets.rs"]
pub mod targets;

#[cfg(any(test, feature = "test-support"))]
#[path = "completed_projection_tests.rs"]
pub mod completed_projection_tests;
