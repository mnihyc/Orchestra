//! Explicit resource lifecycle notices share the operation audience and child-job grouping.

use crate::{
    middleware::{
        authorization::refusals,
        requests::{ApiError, ApiResult},
    },
    registry::Ctx,
};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::{
    forks::UpdateAudience,
    resource_facts::{FactTarget, ResourceOperationNotification},
};
use orch_authz::Principal;
use orch_db::{
    Conn,
    queries::{notifications::operations, resources::routes::ProviderRow},
};
use orch_engine::notifications::{self, NotificationOperation, OperationRecipient};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct PreparedNotice {
    people: Vec<OperationRecipient>,
    visible: BTreeSet<Id>,
    default_on: bool,
}

pub(super) async fn provider_audience(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
) -> ApiResult<Vec<OperationRecipient>> {
    let mut people = notifications::operation_audience(conn, principal, &[], &[provider])
        .await
        .map_err(ApiError::internal)?;
    people.retain(|person| person.user != principal.user());
    Ok(people)
}

fn merge(people: &mut Vec<OperationRecipient>, extra: Vec<OperationRecipient>) {
    let mut all = people
        .drain(..)
        .map(|person| (person.user, person))
        .collect::<BTreeMap<_, _>>();
    for person in extra {
        let entry = all
            .entry(person.user)
            .or_insert_with(|| OperationRecipient {
                user: person.user,
                ..Default::default()
            });
        entry.environments.extend(person.environments);
        entry.threads.extend(person.threads);
        entry.defaults.extend(person.defaults);
        entry.resources.extend(person.resources);
        entry.providers.extend(person.providers);
    }
    people.extend(all.into_values());
}

pub(super) async fn mcp_audience(
    conn: &mut Conn,
    principal: &Principal,
    server: &orch_db::queries::resources::mcp::McpRow,
    raw: &orch_engine::resource_facts::RawFacts,
) -> ApiResult<Vec<OperationRecipient>> {
    let mut environments = raw
        .dependents
        .iter()
        .filter(|edge| {
            edge.relation != orch_api_contract::resource_facts::DependentRelation::Restore
        })
        .filter_map(|edge| {
            edge.object
                .filter(|object| object.kind == ObjectKind::Environment)
                .map(|object| object.id)
        })
        .collect::<BTreeSet<_>>();
    environments.extend(server.environment_id);
    let threads = raw
        .dependents
        .iter()
        .filter_map(|edge| {
            edge.object
                .filter(|object| object.kind == ObjectKind::Thread)
                .map(|object| object.id)
        })
        .collect::<BTreeSet<_>>();
    let mut people = notifications::operation_audience(
        conn,
        principal,
        &environments.into_iter().collect::<Vec<_>>(),
        &[],
    )
    .await
    .map_err(ApiError::internal)?;
    merge(
        &mut people,
        notifications::operation_thread_audience(
            conn,
            principal,
            &threads.into_iter().collect::<Vec<_>>(),
        )
        .await
        .map_err(ApiError::internal)?,
    );
    let manager = match server.environment_id {
        Some(id) => (
            orch_access_contract::decisions::ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            Permission::EnvironmentManage,
        ),
        None => (
            orch_access_contract::decisions::ObjectRef {
                kind: ObjectKind::McpServer,
                id: server.id,
            },
            Permission::McpManage,
        ),
    };
    notifications::extend_operation_audience(conn, principal, &mut people, &[manager])
        .await
        .map_err(ApiError::internal)?;
    people.retain(|person| person.user != principal.user());
    Ok(people)
}

pub(super) async fn notify_mcp_deletion(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    server: &orch_db::queries::resources::mcp::McpRow,
    people: Vec<OperationRecipient>,
) -> ApiResult<()> {
    let operation = NotificationOperation {
        actor: principal.user(),
        id: orch_types::new_id(),
        kind: "mcp_delete".into(),
        name: server.name.clone(),
        notify: true,
        path: "/resources/mcp".into(),
        subject: Some(("mcp_server".into(), server.id)),
        recipients: people,
    };
    notifications::attach_operation(conn, &[], &operation, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    notifications::notify_operation(conn, &operation, "done", ctx.now)
        .await
        .map_err(ApiError::internal)
}

pub(super) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    target: &FactTarget,
    raw: &orch_engine::resource_facts::RawFacts,
    visible_threads: &BTreeSet<Id>,
) -> ApiResult<Option<PreparedNotice>> {
    if target.model_id.is_some() {
        return Ok(None);
    }
    let (people, place) = match target.object.kind {
        ObjectKind::Provider => (
            provider_audience(conn, principal, target.object.id).await?,
            orch_settings::Place::Provider(target.object.id),
        ),
        ObjectKind::McpServer => {
            let server = orch_db::queries::resources::mcp::servers(conn, &[target.object.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let place = server
                .environment_id
                .map(orch_settings::Place::Environment)
                .unwrap_or_else(|| {
                    server
                        .home_id
                        .map_or(orch_settings::Place::System, orch_settings::Place::Node)
                });
            (mcp_audience(conn, principal, &server, raw).await?, place)
        }
        _ => return Ok(None),
    };
    let named = people
        .iter()
        .filter(|person| {
            person
                .threads
                .iter()
                .any(|thread| visible_threads.contains(thread))
        })
        .map(|person| person.user)
        .collect::<Vec<_>>();
    let visible = operations::visible_names(conn, principal.user(), &named)
        .await?
        .into_iter()
        .collect();
    let blocking = people.iter().any(|person| !person.threads.is_empty());
    let default_on = notifications::operation_default(conn, place, blocking)
        .await
        .map_err(ApiError::internal)?;
    Ok(Some(PreparedNotice {
        people,
        visible,
        default_on,
    }))
}

impl PreparedNotice {
    pub(super) async fn project(
        &self,
        conn: &mut Conn,
    ) -> ApiResult<ResourceOperationNotification> {
        let users =
            refusals::user_refs(conn, &self.visible.iter().copied().collect::<Vec<_>>()).await?;
        let people = self
            .visible
            .iter()
            .filter_map(|id| users.get(id).cloned())
            .collect::<Vec<_>>();
        Ok(ResourceOperationNotification {
            audience: UpdateAudience {
                hidden: i64::try_from(self.people.len().saturating_sub(people.len()))
                    .map_err(ApiError::internal)?,
                people,
            },
            default_on: self.default_on,
        })
    }
    pub(super) fn preserves(&self, fresh: &Self) -> bool {
        self.visible.is_subset(&fresh.visible)
    }
}

pub(super) async fn attach(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &ProviderRow,
    job: Id,
    people: Vec<OperationRecipient>,
    notify: bool,
    deleted: bool,
) -> ApiResult<()> {
    let operation = NotificationOperation {
        actor: ctx.user()?.id(),
        id: job,
        kind: if deleted {
            "provider_delete"
        } else {
            "provider_disable"
        }
        .into(),
        name: row.name.clone(),
        notify,
        path: format!("/resources/providers/{}", row.id),
        subject: Some(("provider".into(), row.id)),
        recipients: people,
    };
    notifications::attach_operation(conn, &[job], &operation, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    notifications::notify_operation(conn, &operation, "running", ctx.now)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}
