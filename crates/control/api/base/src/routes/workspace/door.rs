//! How a request gets into an environment's workspace: by the permission of the environment's own page, whichever thread the caller works beside.
//!
//! The files and the shells of an environment are given on its page, for all of it: `files.view`
//! and `files.manage` list, read and write anywhere in the workspace, a thread's directory being
//! where its agent starts and its panel opens, and no fence. The request's declaration names the
//! permission, and the server holds the caller to it on the environment before a handler runs;
//! whoever manages the environment holds it without being named. A thread named in a request gives
//! nothing: it says beside which thread the caller works, which is where a path or a search starts
//! when none is given. A caller who holds everything on a thread and nothing on its environment is
//! refused, with what is missing named on the environment.

orch_api_base::api_family_callback! {
    bind __api_bind_f1c62865f5, __api_install_f1c62865f5, __api_callback_f1c62865f5;
    slot __ApiCallbackf1c62865f5, __API_CALLBACK_F1C62865F5, __api_set_f1c62865f5;
    base orch_api_base::routes::workspace::door;

    async fn beside<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<String>;
}
