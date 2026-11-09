//! A document's Links section: which tabs it shows, what refers to it, what it refers to, and the graph around it.
//!
//! Each answer is read in one snapshot of the database and checked for the reader inside it: a source, a target or a
//! node the reader may not read is never named, counted, drawn or left in a cursor. Cursors are sealed under the
//! deployment key and bound to the reader and the list, so a position that fell on a hidden source tells its holder
//! nothing. The tabs a document shows are display only; nothing here reads them to decide what to answer.

orch_api_base::api_family_callback! {
    bind __api_bind_e3004a2033, __api_install_e3004a2033, __api_callback_e3004a2033;
    slot __ApiCallbacke3004a2033, __API_CALLBACK_E3004A2033, __api_set_e3004a2033;
    base orch_api_base::routes::links;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_66b5eee4f2, __api_install_66b5eee4f2, __api_callback_66b5eee4f2;
    slot __ApiCallback66b5eee4f2, __API_CALLBACK_66B5EEE4F2, __api_set_66b5eee4f2;
    base orch_api_base::routes::links;

    async fn tabs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::documents::DocumentRow) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::links::DocumentLinks>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9a1ffcc6eb, __api_install_9a1ffcc6eb, __api_callback_9a1ffcc6eb;
    slot __ApiCallback9a1ffcc6eb, __API_CALLBACK_9A1FFCC6EB, __api_set_9a1ffcc6eb;
    base orch_api_base::routes::links;

    fn related_cursor(arg0: orch_engine::links::RelatedPosition, arg1: orch_types::Hash) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fa613b3b55, __api_install_fa613b3b55, __api_callback_fa613b3b55;
    slot __ApiCallbackfa613b3b55, __API_CALLBACK_FA613B3B55, __api_set_fa613b3b55;
    base orch_api_base::routes::links;

    fn related_after<'__api>(arg0: &'__api str) -> orch_api_base::middleware::requests::ApiResult<(orch_engine::links::RelatedPosition, orch_types::Hash)>;
}
