//! The nodes of a whole tree for a view of it, what somebody let the caller into by name, and the topics of a project with the filters of a list.
//!
//! A tree view is every node of a root org that the caller may view, or the
//! ones that changed since a sequence they hold. The engine says which they
//! may view for all of them in the statements it takes for one. A node
//! somebody named the caller on, whose parent they cannot view, is listed
//! apart, so that it can be found without the tree above it. Document-only
//! folders use their own current Read and never become hierarchy nodes. The topics of
//! a project are filtered, ordered and paged in the database, and the ones
//! the caller may not view are dropped afterwards, a batch at a time, until
//! the page is full.

orch_api_base::api_family_callback! {
    bind __api_bind_89b9752c11, __api_install_89b9752c11, __api_callback_89b9752c11;
    slot __ApiCallback89b9752c11, __API_CALLBACK_89B9752C11, __api_set_89b9752c11;
    base orch_api_base::routes::nodes::tree;

    fn unviewed<'__api>(arg0: &'__api [orch_db::queries::hierarchy::TreeRow], arg1: &'__api std::collections::HashMap<orch_types::Id, orch_authz::nodes::Listed>) -> Vec<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_610ad28454, __api_install_610ad28454, __api_callback_610ad28454;
    slot __ApiCallback610ad28454, __API_CALLBACK_610AD28454, __api_set_610ad28454;
    base orch_api_base::routes::nodes::tree;

    fn owners_of<'__api>(arg0: &'__api [orch_db::queries::hierarchy::TreeRow], arg1: &'__api [orch_authz::nodes::Listed]) -> Vec<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0890f40f66, __api_install_0890f40f66, __api_callback_0890f40f66;
    slot __ApiCallback0890f40f66, __API_CALLBACK_0890F40F66, __api_set_0890f40f66;
    base orch_api_base::routes::nodes::tree;

    fn hidden_rows<'__api>(arg0: &'__api [orch_db::queries::hierarchy::TreeRow], arg1: &'__api [orch_authz::nodes::Listed], arg2: &'__api std::collections::HashMap<orch_types::Id, orch_api_contract::users::UserRef>) -> std::collections::HashMap<orch_types::Id, orch_api_contract::common::HiddenItem>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a30b746301, __api_install_a30b746301, __api_callback_a30b746301;
    slot __ApiCallbacka30b746301, __API_CALLBACK_A30B746301, __api_set_a30b746301;
    base orch_api_base::routes::nodes::tree;

    fn thread_of<'__api>(arg0: &'__api orch_db::queries::hierarchy::TreeRow, arg1: Option<&'__api orch_api_contract::nodes::TreeWaiting>, arg2: Option<&'__api orch_api_contract::threads::ThreadDisplay>) -> Option<orch_api_contract::nodes::TreeThread>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e99356e299, __api_install_e99356e299, __api_callback_e99356e299;
    slot __ApiCallbacke99356e299, __API_CALLBACK_E99356E299, __api_set_e99356e299;
    base orch_api_base::routes::nodes::tree;

    async fn private_rows<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::common::HiddenItem>>;
}
