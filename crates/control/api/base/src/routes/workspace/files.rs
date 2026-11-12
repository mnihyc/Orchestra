//! The files of an environment's workspace: listed live or from the last list of the tree, described, brought to the store and read from it in ranges, written on the version the writer saw, made, renamed, copied and deleted.
//!
//! **Reading is two steps.** A fetch says whether that exact version of a file (its size and its time
//! of last change) is in the store, and otherwise asks the machine to upload it and says that it is
//! on its way, or why it cannot be had; then the content is read by its hash, which has to belong to
//! that path in this environment. A machine that is away is answered from what was kept: the list of
//! its tree, and the content of the files that were read before.
//!
//! **One writer at a time, by version.** A write names the hash of the file the writer saw, or says
//! that the file must not exist yet, and writes only if the file is still that; if it is another, the
//! write is refused with the file as it stands and nothing is written. Writes need the environment's
//! own switch and the machine to be there. Each write, rename, copy and deletion is one audit row that
//! names the path and never the content.

orch_api_base::api_family_callback! {
    bind __api_bind_c3e4d76c51, __api_install_c3e4d76c51, __api_callback_c3e4d76c51;
    slot __ApiCallbackc3e4d76c51, __API_CALLBACK_C3E4D76C51, __api_set_c3e4d76c51;
    base orch_api_base::routes::workspace::files;

    fn serve<'__api>(arg0: orch_blob_contract::store::BlobRead, arg1: bool, arg2: &'__api http::HeaderMap, arg3: &'__api orch_types::Hash, arg4: &'__api str, arg5: &'__api str, arg6: bool) -> axum::response::Response;
}

orch_api_base::api_family_callback! {
    bind __api_bind_223f89e0d4, __api_install_223f89e0d4, __api_callback_223f89e0d4;
    slot __ApiCallback223f89e0d4, __API_CALLBACK_223F89E0D4, __api_set_223f89e0d4;
    base orch_api_base::routes::workspace::files;

    async fn read_for<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_types::Hash, arg2: &'__api str, arg3: &'__api str, arg4: bool) -> orch_api_base::middleware::requests::ApiResult<axum::response::Response>;
}
