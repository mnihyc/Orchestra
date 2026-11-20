//! Reading a tree's visible nodes and document counts from the same permission data.

orch_api_base::api_family_callback! {
    bind __api_bind_f52fcb6b67, __api_install_f52fcb6b67, __api_callback_f52fcb6b67;
    slot __ApiCallbackf52fcb6b67, __API_CALLBACK_F52FCB6B67, __api_set_f52fcb6b67;
    base orch_api_base::routes::nodes::tree_docs;

    async fn tree_rows<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id]) -> Result<orch_authz::nodes::TreeRows, orch_authz::Error>;
}
