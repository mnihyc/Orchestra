//! Permanent-loss previews and typed confirmations under fresh resource authority.

orch_api_base::api_family_callback! {
    bind __api_bind_34f314958c, __api_install_34f314958c, __api_callback_34f314958c;
    slot __ApiCallback34f314958c, __API_CALLBACK_34F314958C, __api_set_34f314958c;
    base orch_api_base::routes::loss;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
