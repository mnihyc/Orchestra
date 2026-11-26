//! The actual affected audience and explicit notification choice of a resource stop or deletion.

use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::{common::ErrorCode, resource_facts::ResourceNotificationQuery};
use orch_authz::Principal;
use orch_db::{
    Conn,
    queries::{jobs, notifications::operations},
};
use orch_engine::notifications::{NotificationOperation, OperationRecipient};
use orch_types::{Id, new_id};
use serde::{Deserialize, Serialize};

/// Private reviewed subjects; the people list never grants authority to carry out an act.
#[derive(Serialize, Deserialize)]
pub struct Review {
    /// The native resource whose effect was reviewed.
    pub object: ObjectRef,
    pub model_id: Option<Id>,
    /// The permission page for this resource, including an environment-owned server.
    pub governor: ObjectRef,
    /// The people whose association the initiating review could name.
    pub nameable: std::collections::BTreeSet<Id>,
    /// The exact eligible people and the subjects each may read.
    pub audience: Vec<OperationRecipient>,
}

/// Environment-owned tools keep the environment's native page, never a synthetic server page.
pub async fn subject(conn: &mut Conn, object: ObjectRef) -> ApiResult<ObjectRef> {
    if object.kind == ObjectKind::McpServer {
        let row = orch_db::queries::resources::mcp::subject(conn, object.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if let Some(environment) = row.environment_id {
            return Ok(ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            });
        }
    }
    Ok(object)
}

/// The same affected managers and thread owners used at confirmation.
pub async fn audience(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    now: orch_types::Time,
) -> ApiResult<Vec<OperationRecipient>> {
    if object.kind == ObjectKind::Provider {
        return super::notices::provider_audience(conn, principal, object.id).await;
    }
    let raw = orch_engine::resource_facts::target_dependents(
        conn,
        &orch_api_contract::resource_facts::FactTarget {
            object,
            model_id: None,
        },
        now,
    )
    .await
    .map_err(ApiError::engine)?;
    match object.kind {
        ObjectKind::Mount => super::notices::mount_audience(conn, principal, object.id, &raw).await,
        ObjectKind::McpServer => {
            let server = orch_db::queries::resources::mcp::servers(conn, &[object.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            super::notices::mcp_audience(conn, principal, &server, &raw).await
        }
        _ => Ok(Vec::new()),
    }
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
        audience(conn, principal, object, ctx.app.clock.time()).await?
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
        if review.object != object || review.model_id.is_some() || review.audience != audience {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "The affected people changed; review them again",
            ));
        }
        let governor = subject(conn, object).await?;
        orch_api_base::routes::notifications::operations::record_choice(
            ctx,
            conn,
            principal,
            object,
            governor,
            kind,
            audience.len(),
        )
        .await?;
        jobs::forks::forget_preflight(conn, id).await?;
    }
    Ok(NotificationOperation {
        actor: principal.user(),
        id: new_id(),
        kind: kind.into(),
        name: name.into(),
        notify,
        path: String::new(),
        subject: Some((object.kind.as_str().into(), object.id)),
        recipients: audience,
    })
}
