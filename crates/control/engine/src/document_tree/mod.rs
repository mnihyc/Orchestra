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

pub use create::{
    TextCreationPlan, apply_text_creation_in, create_folder_in, create_text_in,
    prepare_text_creation_in, require_text_creation_in,
};
pub use error::Error;
pub use operations::require_folder_creation_in;
pub use operations::{
    DocumentCopySources, prepare_document_copy_material_in, prepare_document_copy_sources_in,
};
pub use operations::{
    DocumentCopying, apply_document_copy_in, prepare_document_copy_in, require_document_copy_in,
};
pub use operations::{DocumentMovement, apply_document_movement_in, prepare_document_movement_in};
pub use operations::{
    DocumentRestoration, apply_document_restoration_in, prepare_document_restoration_in,
};
pub use operations::{FolderCreation, apply_folder_creation_in, prepare_folder_creation_in};
pub use operations::{Operation, apply_operation_in, complete_operation_in, require_operation_in};
pub use operations::{
    OperationSources, prepare_operation_sources_in, require_operation_sources_in,
};
pub use operations::{copy_text_in, execute_confirmed_in, execute_in};
pub use prepare::{
    DeletionSources, check_deletion_plan_in, complete_deletion_plan_in, deletion_roots_in,
    plan_delete_in, prepare_deletion_sources_in, require_deletion_sources_in,
};
pub use read::{
    EntrySelection, can_create_in, metadata_in, project_entries_in, project_selected_in,
    select_entries_in,
};
pub use read::{entry_in, list_in, load_entries_in, read_in};
pub use retention::restore_deadline_from;
pub use retention::restore_deadline_in;
pub use types::*;
