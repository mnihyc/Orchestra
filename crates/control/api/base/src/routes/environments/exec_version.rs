//! Preparing environment consent for exactly the machine program the manager saw.

orch_api_base::api_family_callback! {
    bind __api_bind_754fbc9cee, __api_install_754fbc9cee, __api_callback_754fbc9cee;
    slot __ApiCallback754fbc9cee, __API_CALLBACK_754FBC9CEE, __api_set_754fbc9cee;
    base orch_api_base::routes::environments::exec_version;

    fn confirmed<'__api>(arg0: &'__api orch_types::Json) -> Vec<orch_link_contract::session::MachineFactsHarness>;
}
