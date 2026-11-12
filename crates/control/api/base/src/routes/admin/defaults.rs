//! The defaults page of the system, which every org starts from: read, changed, and put back to what Orch ships.
//!
//! The page is the access engine's like any other: it is read and changed by the very functions
//! that serve the defaults page of an org, with the system as the place. Whoever holds it as shipped
//! holds a role's worth of permissions in an org that has not changed its own page; a row of it that
//! is locked decides that row for every org below, whatever they put on theirs.
//!
//! What a change would reach (the request that previews one) is not bound: the engine counts the
//! pages that follow a changed row only inside a place that is a node, and says nobody who would
//! gain or lose, so there is nothing yet to answer it with for the system as the place.

orch_api_base::api_family_callback! {
    bind __api_bind_1b7dfb75e1, __api_install_1b7dfb75e1, __api_callback_1b7dfb75e1;
    slot __ApiCallback1b7dfb75e1, __API_CALLBACK_1B7DFB75E1, __api_set_1b7dfb75e1;
    base orch_api_base::routes::admin::defaults;

    async fn put_back_shipped<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<usize>;
}
