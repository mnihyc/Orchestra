//! Existing permission readers: current selector, actual source facts, and bounded ordinary projection.

pub use super::*;

pub use crate::registry::OperationPlan;

pub use orch_access_contract::key_scope::KeyTargetKind;

pub use orch_access_contract::permissions::ObjectKind;

pub use orch_authz::engine::Graph;

pub use orch_authz::Caller;

use std::collections::BTreeSet;
orch_api_base::api_family_callback! {
    bind __api_bind_2e84a8704d, __api_install_2e84a8704d, __api_callback_2e84a8704d;
    slot __ApiCallback2e84a8704d, __API_CALLBACK_2E84A8704D, __api_set_2e84a8704d;
    base orch_api_base::routes::access::operations;

    fn index<'__api>(arg0: &'__api orch_api_base::key_operations::admission::Prepared, arg1: &'__api str) -> orch_api_base::middleware::requests::ApiResult<usize>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b58b9c2537, __api_install_b58b9c2537, __api_callback_b58b9c2537;
    slot __ApiCallbackb58b9c2537, __API_CALLBACK_B58B9C2537, __api_set_b58b9c2537;
    base orch_api_base::routes::access::operations;

    fn bind<'__api>(arg0: &'__api mut orch_api_base::key_operations::admission::Prepared, arg1: &'__api str, arg2: orch_access_contract::key_scope::KeyTargetKind, arg3: &'__api [orch_access_contract::key_scope::KeyResolvedTarget]) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_009a849538, __api_install_009a849538, __api_callback_009a849538;
    slot __ApiCallback009a849538, __API_CALLBACK_009A849538, __api_set_009a849538;
    base orch_api_base::routes::access::operations;

    fn page<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: (), arg4: ()) -> orch_api_base::registry::OperationFuture<'__api>;
}

/// Existing protected projection sources, with role names supplied by the consuming operation.
/// This reuses the same factual Group/Node readers and scope binder; it grants no ordinary rights.
pub struct NameRoles {
    pub groups: Option<&'static str>,
    pub nodes: [(ObjectKind, &'static str); 4],
    pub containers: Option<&'static str>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_9eac307708, __api_install_9eac307708, __api_callback_9eac307708;
    slot __ApiCallback9eac307708, __API_CALLBACK_9EAC307708, __api_set_9eac307708;
    base orch_api_base::routes::access::operations;

    async fn group_targets<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_access_contract::key_scope::KeyResolvedTarget>>;
}

/// The existing optional protected-node name binder for the consuming operation's declared roles.
/// The ordinary reference projector still decides visibility and redaction.
pub fn projection_node_names(
    caller: &Caller,
    plan: &mut OperationPlan,
    graph: &Graph,
    roles: &[(ObjectKind, &str)],
) -> ApiResult<BTreeSet<Id>> {
    let mut allowed = BTreeSet::new();
    for &(kind, role) in roles {
        let mut targets = Vec::new();
        let slot = index(plan, role)?;
        for node in graph.nodes().filter(|n| ObjectKind::from(n.kind) == kind) {
            let Some(target) =
                orch_engine::operation_scope::targets::from_node_identity(graph, node.id)
            else {
                continue;
            };
            if plan.permits_binding(
                caller,
                slot,
                KeyTargetKind::Object(kind),
                std::slice::from_ref(&target),
            )? {
                allowed.insert(node.id);
                targets.push(target);
            }
        }
        bind(plan, role, KeyTargetKind::Object(kind), &targets)?;
    }
    Ok(allowed)
}

orch_api_base::api_family_callback! {
    bind __api_bind_237170b96a, __api_install_237170b96a, __api_callback_237170b96a;
    slot __ApiCallback237170b96a, __API_CALLBACK_237170B96A, __api_set_237170b96a;
    base orch_api_base::routes::access::operations;

    async fn projection_names<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Caller, arg2: &'__api mut orch_api_base::key_operations::admission::Prepared, arg3: orch_api_base::routes::access::page::NameSources, arg4: &'__api orch_api_base::routes::access::operations::NameRoles) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::access::page::NameBounds>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5e37b11ede, __api_install_5e37b11ede, __api_callback_5e37b11ede;
    slot __ApiCallback5e37b11ede, __API_CALLBACK_5E37B11EDE, __api_set_5e37b11ede;
    base orch_api_base::routes::access::operations;

    async fn read_page<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPage>;
}
