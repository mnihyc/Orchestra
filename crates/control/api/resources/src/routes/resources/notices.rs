//! Explicit provider lifecycle notices share the operation audience and child-job grouping.

use crate::{
    middleware::{
        authorization::refusals,
        requests::{ApiError, ApiResult},
    },
    registry::Ctx,
};
use orch_access_contract::permissions::ObjectKind;
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
use std::collections::BTreeSet;

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

pub(super) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    target: &FactTarget,
    visible_threads: &BTreeSet<Id>,
) -> ApiResult<Option<PreparedNotice>> {
    if target.object.kind != ObjectKind::Provider || target.model_id.is_some() {
        return Ok(None);
    }
    let people = provider_audience(conn, principal, target.object.id).await?;
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
    let default_on = notifications::operation_default(
        conn,
        orch_settings::Place::Provider(target.object.id),
        blocking,
    )
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
