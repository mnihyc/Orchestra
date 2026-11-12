//! Native mixed resource fleet consumption; resource candidate/window policy stays shared.

orch_api_base::api_family_callback! {
    bind __api_bind_da9ef22327, __api_install_da9ef22327, __api_callback_da9ef22327;
    slot __ApiCallbackda9ef22327, __API_CALLBACK_DA9EF22327, __api_set_da9ef22327;
    base orch_api_base::routes::resources::environment_fleet;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
