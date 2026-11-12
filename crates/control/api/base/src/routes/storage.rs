//! One authorized storage report from existing retention records, content metadata and node inventories.

orch_api_base::api_family_callback! {
    bind __api_bind_9fa9b3986a, __api_install_9fa9b3986a, __api_callback_9fa9b3986a;
    slot __ApiCallback9fa9b3986a, __API_CALLBACK_9FA9B3986A, __api_set_9fa9b3986a;
    base orch_api_base::routes::storage;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
