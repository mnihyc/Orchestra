//! Native capability operands and optional readback through the common stored adapter.

use super::*;
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    port_sources::{self as sources, Source},
};
use orch_authz::Caller;

pub(super) struct Capability;

#[derive(Clone)]
pub(super) struct Subject {
    environment: EnvironmentFull,
    grant: port_capabilities::PreparedGrant,
}

impl native::Stored for Capability {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::CAPABILITY_GRANT];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let environment = environment(conn, id).await?;
            let grant = port_capabilities::prepare_grant(conn, id, kind_in_path(ctx)?)
                .await
                .map_err(ApiError::internal)?;
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?;
            Ok((
                Subject { environment, grant },
                vec![native::Facts::one(
                    Source::CapabilityEnvironment,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    target,
                )],
            ))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                value.environment.id,
                Permission::PortsManage,
            )
            .await?;
            match value.grant.require(conn, principal).await {
                Ok(()) => Ok(()),
                Err(error) => Err(failure(conn, error).await),
            }
        })
    }
}

pub(super) async fn grant(ctx: Ctx) -> ApiResult<PortCapabilityGrantResponse> {
    let result = change(&ctx).await;
    result.map_err(|error| {
        if ctx.key_receipt_required() {
            error.without_content()
        } else {
            error
        }
    })
}

async fn change(original: &Ctx) -> ApiResult<PortCapabilityGrantResponse> {
    let id = original.id("id")?;
    let kind = kind_in_path(original)?;
    let (mut tx, initial) = transaction_guard::begin(original).await?;
    // Keep the native ordinary refusal before waiting on the actual environment row.
    require(&mut tx, initial.principal(), id, Permission::PortsManage).await?;
    initial.check_deadline(original)?;
    rows::lock_environment(&mut tx, id).await?;
    let (ctx, guard) = transaction_guard::after_wait(original, &mut tx).await?;
    let subject = if native::modern(guard.principal().caller()) {
        guard
            .operation_data::<native::Prepared<Subject>>()?
            .value
            .clone()
    } else {
        <Capability as native::Stored>::load(&ctx, &mut tx, &sources::CAPABILITY_GRANT)
            .await?
            .0
    };
    <Capability as native::Stored>::require(&ctx, &mut tx, guard.principal(), &subject).await?;
    let accepted = guard.principal().caller().clone();
    // The native prepared writer's first statement is the UPDATE of this locked row.
    guard.check_deadline(&ctx)?;
    if let Err(error) = subject
        .grant
        .apply(&mut tx, guard.principal(), &crate::sign_in::origin(&ctx))
        .await
    {
        return Err(failure(&mut tx, error).await);
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let receipt = PortCapabilityGrantReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: id,
        key: port_capabilities::key(kind).into(),
        granted: true,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    match response(&ctx, &accepted).await? {
        Some((value, opened)) => {
            retain(&ctx, &accepted, opened)?;
            Ok(PortCapabilityGrantResponse::Full(value))
        }
        None => Ok(PortCapabilityGrantResponse::Receipt(receipt)),
    }
}

async fn response(
    ctx: &Ctx,
    accepted: &Caller,
) -> ApiResult<Option<(PortCapability, port_capabilities::Gate)>> {
    let accepted_projection = accepted.clone();
    native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let (value, facts) =
                <Capability as native::Stored>::load(ctx, conn, &sources::CAPABILITY_GRANT).await?;
            if !native::modern(current.caller()) {
                require(conn, current, value.environment.id, Permission::PortsManage).await?;
                let gate = value.grant.gate().clone();
                return Ok(Some((
                    capability_view(conn, current, &value.environment, &gate).await?,
                    gate,
                )));
            }
            let response = native::bind(ctx, &sources::CAPABILITY_GRANT, &facts)?;
            response
                .read_response(ctx, conn, &accepted_projection, move |conn, reader| {
                    Box::pin(async move {
                        require(conn, reader, value.environment.id, Permission::PortsManage)
                            .await?;
                        let gate = value.grant.gate().clone();
                        Ok((
                            capability_view(conn, reader, &value.environment, &gate).await?,
                            gate,
                        ))
                    })
                })
                .await
        })
    })
    .await
}

fn retain(ctx: &Ctx, accepted: &Caller, opened: port_capabilities::Gate) -> ApiResult<()> {
    if !native::modern(accepted) {
        return Ok(());
    }
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (opened, accepted.clone()),
        |ctx, conn, _current, (opened, accepted)| {
            Box::pin(async move {
                let (fresh, facts) =
                    <Capability as native::Stored>::load(ctx, conn, &sources::CAPABILITY_GRANT)
                        .await?;
                if fresh.grant.gate() != opened {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the capability response changed",
                    ));
                }
                let response = native::bind(ctx, &sources::CAPABILITY_GRANT, &facts)?;
                response
                    .read_response(ctx, conn, accepted, move |conn, reader| {
                        Box::pin(async move {
                            require(conn, reader, fresh.environment.id, Permission::PortsManage)
                                .await
                        })
                    })
                    .await?
                    .ok_or_else(|| {
                        ApiError::new(ErrorCode::AuthzDenied, "the capability response is closed")
                    })
            })
        },
    )
}
