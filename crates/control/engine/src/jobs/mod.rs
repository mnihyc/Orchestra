//! Jobs components.
//!
//! Durable phases, commands, recovery and cleanup for workspace and runtime work.

pub mod after_up;
pub mod background_end;
pub mod batch;
pub mod capability_change;
pub mod cleanup;
pub mod continuation;
pub mod document_notices;
pub mod environment_move;
pub mod estimates;
pub mod export;
pub mod fork;
pub mod health;
pub mod host_delete;
pub mod import;
pub mod kept_images;
pub mod machine_move;
pub mod mcp_offering;
pub mod mcp_restart;
pub mod measurement;
pub mod provider_provision;
pub mod provision;
pub mod recreate;
pub mod remove;
pub mod restart;
pub mod restore;
pub mod retained;
pub mod runner;
pub mod snapshot;
pub mod source;
pub(crate) mod upload_watch;
pub mod workspace_upload;
