//! One native administrator-message writer for organization and installation audiences.

orch_api_base::api_family_callback! {
    bind __api_bind_796195948a, __api_install_796195948a, __api_callback_796195948a;
    slot __ApiCallback796195948a, __API_CALLBACK_796195948A, __api_set_796195948a;
    base orch_api_base::routes::admin_messages;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
