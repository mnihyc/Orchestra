//! Shared business-operation admission for an existing caller's key restrictions.

pub(crate) mod activity;
pub(crate) mod activity_store;
pub(crate) mod admin;
pub(crate) mod admission;
pub(crate) mod background_read;
pub(crate) mod blob_sources;
pub(crate) mod completed_projection;
pub(crate) mod creation_stage;
pub(crate) mod credentials;
pub(crate) mod direct;
pub(crate) mod native;
pub(crate) mod resolver;
pub(crate) mod snapshot_read;
pub(crate) mod targets;

#[cfg(test)]
pub(crate) mod completed_projection_tests;
