//! The actual affected audience and explicit notification choice of a resource stop or deletion.

use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{common::ErrorCode, resource_facts::ResourceNotificationQuery};
use orch_authz::Principal;
use orch_db::{
    Conn,
    queries::{jobs, notifications::operations},
};
use orch_engine::notifications::{self, NotificationOperation, OperationRecipient};
use orch_types::{Id, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Private reviewed subjects; the people list never grants authority to carry out an act.
#[derive(Serialize, Deserialize)]
pub struct Review {
    /// The native resource whose effect was reviewed.
    pub object: ObjectRef,
    /// The exact eligible people and the subjects each may read.
    pub audience: Vec<OperationRecipient>,
}

/// The same affected managers and thread owners used at confirmation.
pub async fn audience(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
) -> ApiResult<Vec<OperationRecipient>> {
    if object.kind == ObjectKind::Provider {
        return notifications::operation_audience(conn, principal, &[], &[object.id])
            .await
            .map_err(ApiError::engine);
    }
    let permission = match object.kind {
        ObjectKind::McpServer => Permission::McpManage,
        ObjectKind::Mount => Permission::MountManage,
        _ => return Ok(Vec::new()),
    };
    let mut people = Vec::new();
    notifications::extend_operation_audience(conn, principal, &mut people, &[(object, permission)])
        .await
        .map_err(ApiError::engine)?;
    Ok(people)
}

/// Persist only the already-produced audience, after its successful native read commits.
pub async fn retain(ctx: &Ctx, user: Id, id: Id, review: &Review) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    jobs::forks::save_preflight(
        &mut tx,
        &jobs::forks::PreflightRow {
            id,
            kind: "resource_stop".into(),
            node_id: None,
            environment_id: None,
            created_by: user,
            plan: serde_json::to_value(review).map_err(ApiError::internal)?,
            expires_at: orch_engine::updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
    operations::preview(
        &mut tx,
        id,
        &review
            .audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Accept the current cohort only after ordinary resource management admission in this action.
pub async fn accept(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    name: &str,
    kind: &str,
    query: &ResourceNotificationQuery,
) -> ApiResult<NotificationOperation> {
    let notify = query.notify.unwrap_or(false);
    let audience = if notify {
        audience(conn, principal, object).await?
    } else {
        Vec::new()
    };
    if notify {
        let Some(id) = query.notification_review else {
            return Err(ApiError::invalid(
                "notification_review",
                "review the affected people before notifying them",
            ));
        };
        let record = jobs::changes::lock_plan(conn, id)
            .await?
            .filter(|row| {
                row.created_by == principal.user()
                    && row.kind == "resource_stop"
                    && row.expires_at > ctx.now
            })
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::PreflightExpired,
                    "Review the affected people again",
                )
            })?;
        let review: Review = serde_json::from_value(record.plan).map_err(ApiError::internal)?;
        if review.object != object || review.audience != audience {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "The affected people changed; review them again",
            ));
        }
        let (_, resource, _) = orch_authz::resources::site(conn, object)
            .await
            .map_err(ApiError::internal)?;
        let place = resource
            .home
            .map_or(orch_settings::Place::System, orch_settings::Place::Node);
        let max = orch_settings::get::<i64>(
            conn,
            place,
            orch_settings_contract::registry::keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(0);
        let asked = i64::try_from(audience.len()).map_err(ApiError::internal)?;
        let since = ctx.now - chrono::Duration::hours(1);
        let used =
            operations::person_rate_used(conn, principal.user(), resource.root, since).await?;
        if max > 0 && used.saturating_add(asked) > max {
            let mut error = ApiError::new(
                ErrorCode::RateLimited,
                "The person notification rate is reached",
            )
            .with("limit", max)
            .with("asked", asked)
            .with(
                "setting",
                orch_settings_contract::registry::keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
            );
            if let Some(at) = operations::person_rate_release(
                conn,
                principal.user(),
                resource.root,
                since,
                used.saturating_add(asked).saturating_sub(max),
            )
            .await?
            {
                let seconds = u64::try_from(
                    (at + chrono::Duration::hours(1) - ctx.now)
                        .num_seconds()
                        .max(1),
                )
                .map_err(ApiError::internal)?;
                error = error.retry_after(seconds);
            }
            return Err(error);
        }
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            orch_db::audit::name!("person.notified"),
            object.kind.as_str(),
            Some(object.id),
            "Notified affected people of a resource operation",
        );
        entry.org_id = resource.root;
        entry.node_id = resource.home;
        entry.details = json!({"count":asked,"operation_kind":kind});
        orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
        jobs::forks::forget_preflight(conn, id).await?;
    }
    Ok(NotificationOperation {
        id: new_id(),
        kind: kind.into(),
        name: name.into(),
        notify,
        path: String::new(),
        subject: Some((object.kind.as_str().into(), object.id)),
        recipients: audience,
    })
}
