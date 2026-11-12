//! Chats, messages, states, attachments, inspection and forwarding.
//!
//! The rules of chats are `orch-chat`'s and who may do what in one is the access
//! engine's; the handlers here read a request, begin the transaction that a
//! change and everything it causes are made in, ask [`chats::load`] once what
//! the caller may do in the chat, and turn the answer into the types of the
//! API and the refusal into the problem a client acts on. A direct or a group
//! chat that the caller may not read does not exist for them.
//!
//! The bytes of images and files are in `files`: a file stored to be named by
//! an `attachment` part, and the images and files of a chat's messages given
//! back to whoever reads the chat. A forward to an agent's thread is an input
//! of the forwarder's, taken by the thread engine in the transaction of the
//! forward.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use orch_api_contract::chats::Chat;

pub use orch_api_contract::chats::ChatDraft;

pub use orch_api_contract::chats::ChatMessage;

pub use orch_api_contract::chats::ChatState;

pub use orch_authz::Principal;

pub use orch_chat::chats;

pub use orch_db::queries::chats::ChatRow;

pub use orch_db::Conn;
orch_api_base::api_family_callback! {
    bind __api_bind_9015882134, __api_install_9015882134, __api_callback_9015882134;
    slot __ApiCallback9015882134, __API_CALLBACK_9015882134, __api_set_9015882134;
    base orch_api_base::routes::chats;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_2ec54615a9, __api_install_2ec54615a9, __api_callback_2ec54615a9;
    slot __ApiCallback2ec54615a9, __API_CALLBACK_2EC54615A9, __api_set_2ec54615a9;
    base orch_api_base::routes::chats;

    async fn failed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: orch_chat::Error) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6acffcfdba, __api_install_6acffcfdba, __api_callback_6acffcfdba;
    slot __ApiCallback6acffcfdba, __API_CALLBACK_6ACFFCFDBA, __api_set_6acffcfdba;
    base orch_api_base::routes::chats;

    async fn people_label<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::chats::ChatRow, arg3: &'__api [orch_db::queries::chats::MemberRow]) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fd7285312b, __api_install_fd7285312b, __api_callback_fd7285312b;
    slot __ApiCallbackfd7285312b, __API_CALLBACK_FD7285312B, __api_set_fd7285312b;
    base orch_api_base::routes::chats;

    async fn chat_view<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::chats::ChatRow, arg3: &'__api [orch_db::queries::chats::MemberRow], arg4: &'__api orch_authz::chats::Standing) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::chats::Chat>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9f835fb32e, __api_install_9f835fb32e, __api_callback_9f835fb32e;
    slot __ApiCallback9f835fb32e, __API_CALLBACK_9F835FB32E, __api_set_9f835fb32e;
    base orch_api_base::routes::chats;

    async fn pin<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::chats::ChatMessageMutationResponse>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fde78fd2f0, __api_install_fde78fd2f0, __api_callback_fde78fd2f0;
    slot __ApiCallbackfde78fd2f0, __API_CALLBACK_FDE78FD2F0, __api_set_fde78fd2f0;
    base orch_api_base::routes::chats;

    async fn state_candidates<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_db::queries::chats::ChatRow>>;
}

pub struct NativeState {
    pub chat: orch_db::queries::chats::ChatRow,
    pub state: ChatState,
}

orch_api_base::api_family_callback! {
    bind __api_bind_a49a3349ce, __api_install_a49a3349ce, __api_callback_a49a3349ce;
    slot __ApiCallbacka49a3349ce, __API_CALLBACK_A49A3349CE, __api_set_a49a3349ce;
    base orch_api_base::routes::chats;

    async fn prepare_state_rows<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_db::queries::chats::ChatRow]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_base::routes::chats::NativeState>>;
}

#[path = "chats/duplicate.rs"]
pub mod duplicate;

#[path = "chats/read_operation.rs"]
pub mod read_operation;

#[path = "chats/state_collection.rs"]
pub mod state_collection;
