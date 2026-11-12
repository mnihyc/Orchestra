//! Exact operands for the existing thread creator, without changing its ordinary writer.
//!
//! Existing operands resolve before the writer. New Environments and This-thread
//! contributions complete against their actual inserted targets in a private transaction.

pub use crate::key_operations::admission::Prepared;

pub use orch_engine::binding::Birth;

pub use orch_engine::binding::NewThread;
#[derive(Clone)]
pub struct Creation {
    pub new: NewThread,
    pub birth: Birth,
    pub created: Option<orch_engine::binding::CreatedThread>,
    pub plan: Prepared,
}

orch_api_base::api_family_callback! {
    bind __api_bind_2835df0f88, __api_install_2835df0f88, __api_callback_2835df0f88;
    slot __ApiCallback2835df0f88, __API_CALLBACK_2835DF0F88, __api_set_2835df0f88;
    base orch_api_base::routes::threads::creation_operation;

    fn complete_topic<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: orch_types::Id, arg4: &'__api orch_api_contract::nodes::TopicCreateBody) -> orch_api_base::registry::OperationFuture<'__api>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5fe59f6144, __api_install_5fe59f6144, __api_callback_5fe59f6144;
    slot __ApiCallback5fe59f6144, __API_CALLBACK_5FE59F6144, __api_set_5fe59f6144;
    base orch_api_base::routes::threads::creation_operation;

    fn complete_staged<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: orch_types::Id, arg4: &'__api orch_api_contract::nodes::CreateThread) -> orch_api_base::registry::OperationFuture<'__api>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4ad2f8ac5f, __api_install_4ad2f8ac5f, __api_callback_4ad2f8ac5f;
    slot __ApiCallback4ad2f8ac5f, __API_CALLBACK_4AD2F8AC5F, __api_set_4ad2f8ac5f;
    base orch_api_base::routes::threads::creation_operation;

    async fn receipt<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: orch_types::Id, arg3: orch_types::Id, arg4: Option<&'__api orch_api_contract::nodes::FirstInput>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::nodes::ThreadCreateReceipt>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_87a2ecaa32, __api_install_87a2ecaa32, __api_callback_87a2ecaa32;
    slot __ApiCallback87a2ecaa32, __API_CALLBACK_87A2ECAA32, __api_set_87a2ecaa32;
    base orch_api_base::routes::threads::creation_operation;

    async fn read<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::threads::creation_operation::Creation, arg4: orch_types::Id, arg5: bool) -> orch_api_base::middleware::requests::ApiResult<Option<(orch_api_contract::threads::Thread, Option<orch_api_contract::nodes::Topic>)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_61ae43b5cf, __api_install_61ae43b5cf, __api_callback_61ae43b5cf;
    slot __ApiCallback61ae43b5cf, __API_CALLBACK_61AE43B5CF, __api_set_61ae43b5cf;
    base orch_api_base::routes::threads::creation_operation;

    async fn read_program<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_api_base::key_operations::admission::Prepared, arg5: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_contract::threads::Thread>>;
}
