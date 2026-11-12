//! Current document reads and existing feed projections share one scope/source preparation.

use orch_api_contract::documents::DocumentRef;
orch_api_base::api_family_callback! {
    bind __api_bind_3b390eb0a3, __api_install_3b390eb0a3, __api_callback_3b390eb0a3;
    slot __ApiCallback3b390eb0a3, __API_CALLBACK_3B390EB0A3, __api_set_3b390eb0a3;
    base orch_api_base::routes::documents::read_operation;

    async fn root_plan<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::key_operations::admission::Prepared>;
}

pub struct FeedProjection {
    pub document: Option<DocumentRef>,
    pub blocks: Vec<orch_api_contract::documents::DocumentBlock>,
    pub discussions: Vec<orch_api_contract::documents::DocumentDiscussion>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_25013cd151, __api_install_25013cd151, __api_callback_25013cd151;
    slot __ApiCallback25013cd151, __API_CALLBACK_25013CD151, __api_set_25013cd151;
    base orch_api_base::routes::documents::read_operation;

    async fn read_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::documents::DocumentRow, arg3: &'__api [orch_db::queries::documents::BlockRow], arg4: &'__api [orch_db::queries::documents::discussions::Full], arg5: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::documents::read_operation::FeedProjection>;
}
