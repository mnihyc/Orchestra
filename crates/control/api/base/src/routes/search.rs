//! Authorized transcript and node search, the search inside one conversation, and the finder's names.
//!
//! Names are checked in ranked batches, and paths are built with the same
//! redaction rules as the tree. A request sees one database snapshot, so a
//! permission change cannot race the content its answer carries.

orch_api_base::api_family_callback! {
    bind __api_bind_737f400002, __api_install_737f400002, __api_callback_737f400002;
    slot __ApiCallback737f400002, __API_CALLBACK_737F400002, __api_set_737f400002;
    base orch_api_base::routes::search;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
