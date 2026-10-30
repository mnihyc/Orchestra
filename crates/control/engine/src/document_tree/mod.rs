//! Document-only containers and atomic organization using current person or agent authority.

mod access;
mod audience;
mod copying;
mod create;
mod error;
mod operations;
mod prepare;
mod read;
mod retention;
mod types;

pub use create::{create_folder_in, create_text_in};
pub use error::Error;
pub use operations::{copy_text_in, execute_in};
pub use prepare::plan_delete_in;
pub use read::{can_create_in, metadata_in, project_entries_in};
pub use read::{entry_in, list_in, load_entries_in, read_in};
pub use retention::restore_deadline_from;
pub use retention::restore_deadline_in;
pub use types::*;
