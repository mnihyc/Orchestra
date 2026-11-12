//! Native job read branches and their factual subjects, before a Job is formatted.

pub use super::*;

use futures::future::BoxFuture;

pub use orch_access_contract::decisions::ObjectRef;

pub use orch_access_contract::permissions::ObjectKind;
/// One successful native branch; optional row links are never an ANDed source union.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub subject: Option<ObjectRef>,
    pub namespace: Option<ObjectRef>,
    pub provider_management: bool,
    pub cut: bool,
}

impl Choice {
    pub fn maker() -> Self {
        Self {
            subject: None,
            namespace: None,
            provider_management: false,
            cut: false,
        }
    }
    pub fn object(kind: ObjectKind, id: Id) -> Self {
        Self {
            subject: Some(ObjectRef { kind, id }),
            namespace: Some(ObjectRef { kind, id }),
            provider_management: false,
            cut: false,
        }
    }
    pub fn provider(id: Id) -> Self {
        Self {
            provider_management: true,
            ..Self::object(ObjectKind::Provider, id)
        }
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_302f5be720, __api_install_302f5be720, __api_callback_302f5be720;
    slot __ApiCallback302f5be720, __API_CALLBACK_302F5BE720, __api_set_302f5be720;
    base orch_api_base::routes::environments::jobs::subject;

    fn identity<'__api>(arg0: &'__api orch_db::queries::jobs::JobRow, arg1: Option<&'__api orch_access_contract::key_scope::KeyResolvedTarget>) -> orch_access_contract::key_scope::KeyResolvedTarget;
}

/// Scope selects genuine native alternatives; ordinary authorization stays in each original gate.
pub trait Selection: Send + Sync {
    fn accepts<'a>(
        &'a self,
        conn: &'a mut Conn,
        row: &'a JobRow,
        choice: Choice,
    ) -> BoxFuture<'a, ApiResult<bool>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0f6e4906bc, __api_install_0f6e4906bc, __api_callback_0f6e4906bc;
    slot __ApiCallback0f6e4906bc, __API_CALLBACK_0F6E4906BC, __api_set_0f6e4906bc;
    base orch_api_base::routes::environments::jobs::subject;

    async fn read<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::jobs::JobRow, arg3: Option<&'__api dyn orch_api_base::routes::environments::jobs::subject::Selection>) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::jobs::subject::Choice>;
}
