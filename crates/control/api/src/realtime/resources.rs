//! Admission to a resource's native row feed, and private provider progress for those who manage it.
//!
//! The resource's page decides who may follow changes. Native rows use the GET's own View checks at delivery. Private progress
//! receives a second check at delivery, since a viewer may follow changes without managing it.

use super::subscriptions::{Access, principal_of};
use super::{Fail, Inner};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission, PermissionName};
use orch_authz::{Act, Policy};
use orch_realtime_contract::feeds::{Feed, ResourceKind};
use orch_types::Id;

fn object(feed: &Feed) -> Option<ObjectRef> {
    let Feed::Resource { kind, id } = feed else {
        return None;
    };
    let kind = match kind {
        ResourceKind::Provider => ObjectKind::Provider,
        ResourceKind::Agent => ObjectKind::Agent,
        ResourceKind::Host => ObjectKind::Host,
        ResourceKind::Machine => ObjectKind::Machine,
        ResourceKind::Template => ObjectKind::Template,
        ResourceKind::McpServer => ObjectKind::McpServer,
        ResourceKind::Mount => ObjectKind::Mount,
        ResourceKind::Skill => ObjectKind::Skill,
    };
    Some(ObjectRef { kind, id: *id })
}

/// Whether the caller holds any permission on the resource being followed.
pub(crate) async fn decide(inner: &Inner, user: Id, feed: &Feed) -> Result<Access, Fail> {
    let Some(object) = object(feed) else {
        return Ok(Access::NotFound);
    };
    let mut conn = inner.db.acquire().await?;
    if object.kind == ObjectKind::Machine {
        // The machine page of a node is its Host facade. Use the same permission supplier as
        // GET /machines/{id}, so a node's version/progress feed is neither lost nor widened.
        let Some(principal) = principal_of(inner, &mut conn, user).await? else {
            return Ok(Access::Forbidden);
        };
        return match orch_authz::resources::check_machine(
            &mut conn,
            &principal,
            object.id,
            Permission::MachineView,
            Policy::default(),
        )
        .await
        {
            Ok(decision) => Ok(if decision.is_allowed() {
                Access::Allowed
            } else {
                Access::Forbidden
            }),
            Err(orch_authz::Error::NotFound(_)) => Ok(Access::NotFound),
            Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::Archived { .. }) => {
                Ok(Access::Forbidden)
            }
            Err(error) => Err(error.into()),
        };
    }
    let rows = orch_db::queries::resources::resource_facts(&mut conn, &[object.id]).await?;
    if !rows
        .iter()
        .any(|row| row.kind == object.kind.as_str() && !row.deleted)
    {
        return Ok(Access::NotFound);
    }
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    match orch_authz::resources::effective(&mut conn, &principal, object, Policy::default()).await {
        Ok(effective) => Ok(if effective.held.is_empty() {
            Access::Forbidden
        } else {
            Access::Allowed
        }),
        Err(orch_authz::Error::NotFound(_)) => Ok(Access::NotFound),
        Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::Archived { .. }) => {
            Ok(Access::Forbidden)
        }
        Err(error) => Err(error.into()),
    }
}

/// Whether the caller currently holds provider Manage for private progress.
pub(crate) async fn may_manage(inner: &Inner, user: Id, feed: &Feed) -> Result<bool, Fail> {
    let Some(object) = object(feed) else {
        return Ok(false);
    };
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(false);
    };
    Ok(matches!(
        orch_authz::resources::check(
            &mut conn,
            &principal,
            object,
            orch_access_contract::permissions::Permission::ProviderManage,
            Policy::default()
        )
        .await,
        Ok(Decision::Allowed { .. })
    ))
}

/// Whether the caller may manage the thread whose diagnostics are followed.
pub(crate) async fn raw_access(inner: &Inner, user: Id, thread: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    if !orch_db::queries::feeds::gone_nodes(&mut conn, &[thread])
        .await?
        .is_empty()
    {
        return Ok(Access::Gone);
    }
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    match orch_authz::nodes::check_named(
        &mut conn,
        &principal,
        thread,
        PermissionName::Exact(Permission::ThreadManage),
        Act::Use,
        Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok(Access::Allowed),
        Ok(Decision::Refused(_)) => Ok(Access::Forbidden),
        Err(orch_authz::Error::NotFound(_)) => Ok(Access::NotFound),
        Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::Archived { .. }) => {
            Ok(Access::Forbidden)
        }
        Err(error) => Err(error.into()),
    }
}
