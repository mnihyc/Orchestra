//! Images and files sent from a browser, whole or in pieces, and a file attached to a thread so that it reaches the agent when the input is delivered.
//!
//! An image goes up with one request and comes back as a hash to name in an `image` part; a file
//! larger than one request carries goes up in pieces ([`crate::uploads`]) and is ended by a request
//! that makes it a blob. A file is attached to a thread from either: its bytes in the body, or the
//! hash of an upload that was ended. Attaching writes nothing into the workspace: the file is kept as
//! a blob, pinned so that it stays, and the path it will have is told; the engine writes it there
//! when the input is delivered, so that attaching needs no environment to be running. What a thread
//! carries comes back by its hash to whoever may read the thread: the images and files of its
//! inputs, forwarded ones included, with the type they were stored with; nothing else the store
//! holds is found through a thread.

orch_api_base::api_family_callback! {
    bind __api_bind_0658d2fd50, __api_install_0658d2fd50, __api_callback_0658d2fd50;
    slot __ApiCallback0658d2fd50, __API_CALLBACK_0658D2FD50, __api_set_0658d2fd50;
    base orch_api_base::routes::threads::uploads;

    fn filename_of<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> Option<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_02b2e50501, __api_install_02b2e50501, __api_callback_02b2e50501;
    slot __ApiCallback02b2e50501, __API_CALLBACK_02B2E50501, __api_set_02b2e50501;
    base orch_api_base::routes::threads::uploads;

    async fn serve_completed<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_types::Hash, arg2: String, arg3: orch_api_contract::threads::UploadBlobGetQuery) -> orch_api_base::middleware::requests::ApiResult<axum::response::Response>;
}
