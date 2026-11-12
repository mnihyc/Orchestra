//! The jobs of environments over HTTP: reading one with how far it has come, and asking that one be stopped.
//!
//! A job is work that takes a while and is carried out apart from the request that asked for it: an
//! environment being made or started, a unit being removed. Its row is the truth about how far it has
//! come, so that a client that reads it after the API restarted sees the same bar. A job is read by
//! those who may see what it is about (the thread, the environment or the provider), and stopped by
//! those who could have started it, until its commit phase; a removal is not stopped, it is withdrawn
//! before it begins.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use orch_api_contract::common::ErrorCode;

pub use orch_api_contract::workspace::Job;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_db::queries::jobs as rows;

pub use orch_db::queries::jobs::JobRow;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_c328160ae9, __api_install_c328160ae9, __api_callback_c328160ae9;
    slot __ApiCallbackc328160ae9, __API_CALLBACK_C328160AE9, __api_set_c328160ae9;
    base orch_api_base::routes::environments::jobs;

    async fn view<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::jobs::JobRow) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::workspace::Job>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_08462586ba, __api_install_08462586ba, __api_callback_08462586ba;
    slot __ApiCallback08462586ba, __API_CALLBACK_08462586BA, __api_set_08462586ba;
    base orch_api_base::routes::environments::jobs;

    fn waiting_machine<'__api>(arg0: &'__api orch_db::queries::jobs::JobRow) -> Option<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e31437c66b, __api_install_e31437c66b, __api_callback_e31437c66b;
    slot __ApiCallbacke31437c66b, __API_CALLBACK_E31437C66B, __api_set_e31437c66b;
    base orch_api_base::routes::environments::jobs;

    async fn machine_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_db::queries::jobs::JobRow]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, String>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1e8912e3ab, __api_install_1e8912e3ab, __api_callback_1e8912e3ab;
    slot __ApiCallback1e8912e3ab, __API_CALLBACK_1E8912E3AB, __api_set_1e8912e3ab;
    base orch_api_base::routes::environments::jobs;

    async fn view_with_machine_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::jobs::JobRow, arg2: Option<&'__api std::collections::HashMap<orch_types::Id, String>>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::workspace::Job>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5fe1762c39, __api_install_5fe1762c39, __api_callback_5fe1762c39;
    slot __ApiCallback5fe1762c39, __API_CALLBACK_5FE1762C39, __api_set_5fe1762c39;
    base orch_api_base::routes::environments::jobs;

    async fn read_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::workspace::Job>;
}

#[path = "jobs/creation.rs"]
pub mod creation;

#[path = "jobs/subject.rs"]
pub mod subject;
