//! The ordinary Environment list consumes the shared native resource collection adapter.

orch_api_base::api_family_callback! {
    bind __api_bind_2caaba278c, __api_install_2caaba278c, __api_callback_2caaba278c;
    slot __ApiCallback2caaba278c, __API_CALLBACK_2CAABA278C, __api_set_2caaba278c;
    base orch_api_base::routes::resources::environment_read;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
