//! The permission page of a resource, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice.
//!
//! The page of a node is read by the routes of the hierarchy; the requests are the same for a
//! resource, and the page machinery is the access engine's for both. What a resource adds is its home:
//! its page follows the defaults of the org it belongs to, and the two policies that govern a change
//! (whether its owners may name somebody from outside its root org, and whether the subscription it
//! may be is kept to its first owner) are the settings of that org.

orch_api_base::api_family_callback! {
    bind __api_bind_d4b53ba781, __api_install_d4b53ba781, __api_callback_d4b53ba781;
    slot __ApiCallbackd4b53ba781, __API_CALLBACK_D4B53BA781, __api_set_d4b53ba781;
    base orch_api_base::routes::resources::page;

    async fn get<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPage>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_26051412f4, __api_install_26051412f4, __api_callback_26051412f4;
    slot __ApiCallback26051412f4, __API_CALLBACK_26051412F4, __api_set_26051412f4;
    base orch_api_base::routes::resources::page;

    async fn update<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_api_contract::access::PageChange) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPageWriteResponse>;
}
