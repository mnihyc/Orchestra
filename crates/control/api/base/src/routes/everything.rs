//! The complete authorized hierarchy for every signed-in person, with a whole-system scope for administrators.

orch_api_base::api_family_callback! {
    bind __api_bind_c47513c681, __api_install_c47513c681, __api_callback_c47513c681;
    slot __ApiCallbackc47513c681, __API_CALLBACK_C47513C681, __api_set_c47513c681;
    base orch_api_base::routes::everything;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "everything/library.rs"]
pub mod library;
