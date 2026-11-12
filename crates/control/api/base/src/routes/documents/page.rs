//! The permission page of a document, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice.
//!
//! A document's page is its own and stores almost nothing: its Owners row
//! and whatever somebody wrote on it. Every other row follows the row of the
//! same name on the page of the node it is beside. Making it private cuts the
//! rows loose; resetting it makes them follow again. The engine decides who
//! may read and who may change the page and what a change does; what a
//! document adds is its home: the policy of the node it is beside on naming
//! somebody from outside the root org, and the stamp a change leaves on the
//! document, so that whoever has it open is told its page changed.

orch_api_base::api_family_callback! {
    bind __api_bind_cb888d4d4b, __api_install_cb888d4d4b, __api_callback_cb888d4d4b;
    slot __ApiCallbackcb888d4d4b, __API_CALLBACK_CB888D4D4B, __api_set_cb888d4d4b;
    base orch_api_base::routes::documents::page;

    async fn get<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPage>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_33e74fbd40, __api_install_33e74fbd40, __api_callback_33e74fbd40;
    slot __ApiCallback33e74fbd40, __API_CALLBACK_33E74FBD40, __api_set_33e74fbd40;
    base orch_api_base::routes::documents::page;

    async fn update<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_api_contract::access::PageChange) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPageWriteResponse>;
}
