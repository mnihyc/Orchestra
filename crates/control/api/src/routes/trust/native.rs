//! The existing native trust subject, consumed by shared stored-subject adapters.

use super::*;
use crate::key_operations::native::{Stored, resource_reply::StoredResource};
use crate::key_operations::{native, targets};
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::projection::single;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::environment_sources::{TRUST_SET, TrustSource};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Principal;

#[derive(Clone)]
pub(super) struct Subject {
    pub environment: Id,
    pub harness: String,
}

pub(super) struct Trust;

impl Stored for Trust {
    type Source = TrustSource;
    type Value = Subject;
    const ROWS: &'static [Declaration<TrustSource>] = &[TRUST_SET];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<TrustSource>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<TrustSource>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            orch_db::queries::environments::thread_engine::environment(conn, environment)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let subject = Subject {
                environment,
                harness: ctx.param("harness")?.to_owned(),
            };
            let target = targets::page_object(conn, Self::object(&subject)).await?;
            Ok((
                subject,
                vec![native::Facts::one(
                    TrustSource::Environment,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    target,
                )],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .may_answer_config_trust(conn, principal, subject.environment)
                .await
                .map_err(ApiError::engine)?
            {
                Ok(()) => Ok(()),
                Err(refusal) => Err(denied(ctx, conn, *refusal).await?),
            }
        })
    }
}

impl StoredResource for Trust {
    const READ_SOURCE: TrustSource = TrustSource::Environment;
    const ROLES: &'static single::Roles = &single::Roles {
        names: NameRoles {
            groups: Some("trust_reply_groups"),
            nodes: [
                (ObjectKind::Org, "trust_reply_orgs"),
                (ObjectKind::Project, "trust_reply_projects"),
                (ObjectKind::Topic, "trust_reply_topics"),
                (ObjectKind::Thread, "trust_reply_threads"),
            ],
            containers: None,
        },
        sources: "trust_reply_sources",
    };

    fn object(subject: &Subject) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Environment,
            id: subject.environment,
        }
    }

    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // The original response was the manager's Environment view. A completed write
        // cannot lend that ordinary authority after it has been lost.
        Self::require(ctx, conn, principal, subject)
    }
}

async fn subject(
    ctx: &Ctx,
    conn: &mut Conn,
    guard: &transaction_guard::Guard,
) -> ApiResult<Subject> {
    if native::modern(guard.principal().caller()) {
        Ok(guard
            .operation_data::<native::Prepared<Subject>>()?
            .value
            .clone())
    } else {
        Ok(Trust::load(ctx, conn, &TRUST_SET).await?.0)
    }
}

/// Take the actual native Environment lock before preparing the final operation operands.
pub(super) async fn after_wait(
    ctx: &Ctx,
    conn: &mut Conn,
    initial: &transaction_guard::Guard,
) -> ApiResult<(Ctx, transaction_guard::Guard, Subject)> {
    let before = subject(ctx, conn, initial).await?;
    let gate = Trust::require(ctx, conn, initial.principal(), &before).await;
    initial.check_deadline(ctx)?;
    gate?;
    orch_db::queries::environments::hosting::lock_environment(conn, before.environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, guard) = transaction_guard::after_wait(ctx, conn).await?;
    let subject = subject(&ctx, conn, &guard).await?;
    let gate = Trust::require(&ctx, conn, guard.principal(), &subject).await;
    guard.check_deadline(&ctx)?;
    gate?;
    Ok((ctx, guard, subject))
}
