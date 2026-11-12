//! Native usage subjects consumed by the common stored-source adapter.

pub use super::scope;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use orch_authz::Principal;

pub use orch_db::Conn;
orch_api_base::api_family_callback! {
    bind __api_bind_37016ebbd9, __api_install_37016ebbd9, __api_callback_37016ebbd9;
    slot __ApiCallback37016ebbd9, __API_CALLBACK_37016EBBD9, __api_set_37016ebbd9;
    base orch_api_base::routes::usage::native;

    async fn names<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: &'static orch_api_contract::operation_sources::Declaration<orch_api_contract::usage_sources::Source>, arg4: orch_api_base::routes::access::page::NameSources) -> orch_api_base::middleware::requests::ApiResult<(
    orch_api_base::routes::access::page::NameBounds,
    orch_engine::operation_scope::sources::Facts<orch_api_contract::usage_sources::Source>,
)>;
}

#[derive(Clone)]
pub struct Disclosure {
    pub scope: scope::Scope,
    pub depth: orch_api_contract::usage::UsageDepth,
    pub names: super::names::Cut,
    pub records: Vec<orch_db::usage::ledger::EventRow>,
}

impl Disclosure {
    pub async fn require(&self, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
        use orch_api_contract::usage::UsageDepth::{Own, Reports, Summary};
        let current = scope::open(conn, principal, self.scope).await?;
        let enough = matches!(
            (self.depth, current.depth),
            (Own, _) | (Summary, Summary | Reports) | (Reports, Reports)
        );
        let all_places = scope::all_record_places(conn, principal, &current).await?;
        let visible =
            scope::visible_records(conn, principal, all_places, self.records.clone()).await?;
        if !enough
            || visible.len() != self.records.len()
            || !self.names.current(conn, principal).await?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the original usage projection is no longer readable",
            ));
        }
        Ok(())
    }
}
