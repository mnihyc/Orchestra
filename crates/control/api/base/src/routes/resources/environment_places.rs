//! Native placement choices with resource windowing and factual source publication.

orch_api_base::api_family_callback! {
    bind __api_bind_5045ede9eb, __api_install_5045ede9eb, __api_callback_5045ede9eb;
    slot __ApiCallback5045ede9eb, __API_CALLBACK_5045EDE9EB, __api_set_5045ede9eb;
    base orch_api_base::routes::resources::environment_places;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
