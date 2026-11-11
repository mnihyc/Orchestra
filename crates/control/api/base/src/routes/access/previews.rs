//! What a change of a page, of the defaults of a place or of the defaults of the system would do, said before anything is changed: how many pages it reaches and who would gain and lose what on them.
//!
//! The change is the access engine's own, made inside a transaction that is never committed, so
//! that a preview refuses what the change would refuse and leaves nothing behind. A person is named
//! as far as the caller may know them and counted otherwise. The page of a place is one page
//! and opens nothing inside it; the defaults of a place reach the pages inside it that follow them.

orch_api_base::api_family_callback! {
    bind __api_bind_7a9ea67528, __api_install_7a9ea67528, __api_callback_7a9ea67528;
    slot __ApiCallback7a9ea67528, __API_CALLBACK_7A9EA67528, __api_set_7a9ea67528;
    base orch_api_base::routes::access::previews;

    async fn worded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_authz::previews::Preview) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PagePreview>;
}
