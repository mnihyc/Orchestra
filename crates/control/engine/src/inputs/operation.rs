//! Current native/bypass operand binding shared by the HTTP input producer and deferred input work.
//! This does not authorize a resource or infer authority from the caller's identity.

use crate::operation_scope::{
    BindingError, Prepared,
    targets::{self, TargetError},
};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::native::NativePermission;
use orch_api_contract::operations::{CatalogueError, FacetUsage, TargetResolver};
use orch_authz::caps::Home;
use orch_db::Conn;

/// Original catalogue, binding and target lookup failures remain separate for the HTTP adapter.
#[derive(Debug, thiserror::Error)]
pub enum InputOperationError {
    /// The declared operation or its scope binding refused.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The existing target reader could not resolve a current operand.
    #[error(transparent)]
    Target(#[from] TargetError),
}

fn invalid(plan: &Prepared, reason: &str) -> BindingError {
    BindingError::Catalogue(CatalogueError::Invalid {
        operation: plan.variant().operation.clone(),
        reason: reason.into(),
    })
}

const BYPASS_ROLES: &[(&str, KeyTargetKind)] = &[
    (
        "input_bypass_providers",
        KeyTargetKind::Object(ObjectKind::Provider),
    ),
    ("input_bypass_orgs", KeyTargetKind::Object(ObjectKind::Org)),
    (
        "input_bypass_projects",
        KeyTargetKind::Object(ObjectKind::Project),
    ),
    (
        "input_bypass_topics",
        KeyTargetKind::Object(ObjectKind::Topic),
    ),
    (
        "input_bypass_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
    ),
    (
        "input_bypass_system",
        KeyTargetKind::Identity(KeyIdentityKind::System),
    ),
];

/// Bind the actual thread to each explicitly declared thread subject slot.
pub fn bind_thread(plan: &mut Prepared, target: &KeyResolvedTarget) -> Result<(), BindingError> {
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
        return Err(BindingError::Refused);
    }
    let slots: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| (facet.target == TargetResolver::Path("id")).then_some(index))
        .collect();
    for index in slots {
        plan.bind(
            index,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(target),
        )?;
    }
    Ok(())
}

/// Bind the current stored native action's actual Manage requirement.
pub fn bind_native(
    plan: &mut Prepared,
    target: &KeyResolvedTarget,
    permission: Option<NativePermission>,
) -> Result<(), BindingError> {
    bind_native_collection(plan, "input_native_manage", target, permission)
}

/// Bind the declared native-management collection to the permission of the actual selected
/// stored action. Schedule producers use the same classifier and their declared collection role.
pub fn bind_native_collection(
    plan: &mut Prepared,
    role: &'static str,
    target: &KeyResolvedTarget,
    permission: Option<NativePermission>,
) -> Result<(), BindingError> {
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.target == TargetResolver::ResolvedCollection(role)
                && facet.usage == FacetUsage::Required
                && facet.action == KeyAction::Manage
                && facet.resources == [KeyTargetKind::Object(ObjectKind::Thread)]
        })
        .ok_or_else(|| {
            invalid(
                plan,
                "the native input authority collection is not declared",
            )
        })?;
    if permission == Some(NativePermission::Manage) {
        plan.bind(
            index,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(target),
        )?;
    } else {
        plan.bind_empty(index)?;
    }
    Ok(())
}

/// Bind every actual cap/provider home reached by the existing bypass producer.
pub async fn bind_bypass(
    plan: &mut Prepared,
    conn: &mut Conn,
    homes: &[Home],
) -> Result<(), InputOperationError> {
    let mut actual = Vec::new();
    for home in homes {
        let target = match *home {
            Home::Provider { provider, .. } => {
                targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: provider,
                    },
                )
                .await?
            }
            Home::Node(node) => targets::node(conn, node).await?,
            Home::System => targets::system(),
        };
        if !actual
            .iter()
            .any(|known: &KeyResolvedTarget| known.object == target.object)
        {
            actual.push(target);
        }
    }
    for (role, kind) in BYPASS_ROLES {
        let index = plan
            .variant()
            .facets
            .iter()
            .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            .ok_or_else(|| invalid(plan, "the input bypass collection is not declared"))?;
        let found: Vec<_> = actual
            .iter()
            .filter(|target| target.object.kind == *kind)
            .cloned()
            .collect();
        if found.is_empty() {
            plan.bind_empty(index)?;
        } else {
            plan.bind(index, *kind, &found)?;
        }
    }
    Ok(())
}

/// The existing input writer's exact reached-cap selection, shared with continuing input work.
pub(crate) async fn caps_to_pass(
    unit: &mut crate::unit::Unit<'_>,
    thread: &orch_db::queries::threads::ThreadRow,
    author: orch_types::Id,
    named: &[orch_types::Id],
) -> Result<Vec<orch_db::usage::caps::CapRow>, super::InputError> {
    if named.is_empty() {
        return Ok(Vec::new());
    }
    let settings = crate::resolved::load(&mut *unit.conn, thread.node_id).await?;
    let Some(ask) = crate::caps::ask_from_settings(unit.conn, thread, &settings).await? else {
        return Ok(Vec::new());
    };
    let ask = crate::caps::Ask {
        person: Some(author),
        ..ask
    };
    let now = unit.now();
    let applied = crate::caps::covering(&mut *unit.conn, &ask, now).await?;
    let mut reached: Vec<orch_db::usage::caps::CapRow> = Vec::new();
    for id in named {
        if reached.iter().any(|cap| cap.id == *id) {
            continue;
        }
        let Some(each) = applied.iter().find(|each| {
            each.cap.id == *id && each.replaced_by.is_none() && each.cap.amount.is_some()
        }) else {
            continue;
        };
        let standing = crate::caps::standing(unit.conn, &each.cap, Some(author), now).await?;
        if !standing.has_room() {
            reached.push(each.cap.clone());
        }
    }
    Ok(reached)
}

/// The native plan-answer policy selects its actual key action; no caller/body can relabel it.
pub async fn plan_decision(
    conn: &mut Conn,
    thread: orch_types::Id,
) -> Result<(Prepared, orch_access_contract::permissions::Permission), super::InputError> {
    if orch_db::queries::threads::thread(conn, thread)
        .await?
        .is_none()
    {
        return Err(super::InputError::NotFound);
    }
    let permission = crate::approvals::deciding_permission(
        conn,
        thread,
        &serde_json::json!({"request_kind":"plan"}),
    )
    .await?;
    let selection = if permission == orch_access_contract::permissions::Permission::ThreadManage {
        "ThreadPlanDecide.plan_manage"
    } else {
        "ThreadPlanDecide.plan_interact"
    };
    let mut plan = Prepared::for_selection(selection).map_err(crate::EngineError::unreadable)?;
    let target = targets::node_identity(conn, thread)
        .await
        .map_err(|error| match error {
            TargetError::Missing => super::InputError::NotFound,
            TargetError::Authz(error) => error.into(),
            TargetError::Db(error) => error.into(),
        })?;
    bind_thread(&mut plan, &target).map_err(crate::EngineError::unreadable)?;
    Ok((plan, permission))
}
