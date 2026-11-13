//! Current chat reads and their live-name projection, shared by HTTP and existing chat feeds.

pub use super::*;
/// The same typed output cut feeds HTTP and the existing delta envelope.
pub struct ReadProjection {
    pub chat: Option<Chat>,
    pub messages: Vec<ChatMessage>,
    pub draft: Option<ChatDraft>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_042efd7a1c, __api_install_042efd7a1c, __api_callback_042efd7a1c;
    slot __ApiCallback042efd7a1c, __API_CALLBACK_042EFD7A1C, __api_set_042efd7a1c;
    base orch_api_base::routes::chats::read_operation;

    async fn root_plan<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::key_operations::admission::Prepared>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_632caf7569, __api_install_632caf7569, __api_callback_632caf7569;
    slot __ApiCallback632caf7569, __API_CALLBACK_632CAF7569, __api_set_632caf7569;
    base orch_api_base::routes::chats::read_operation;

    async fn read_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_chat::chats::Loaded, arg3: &'__api [orch_db::queries::chats::MessageRow], arg4: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::chats::read_operation::ReadProjection>;
}
