//! Registration withdrawal consumes the native ledger packet and the shared stored-source row.

use super::{live_machine, mutations, require_machine};
use crate::certificates::CertificateService;
use crate::key_operations::{native, publication};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::resources::projection::single;
use crate::sign_in::origin;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::machine_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::resources::{MachineRevokeReceipt, MachineRevokeResponse, Resource};
use orch_authz::Principal;
use orch_db::audit::Actor;
use orch_db::{Conn, DbError};
use orch_types::Id;

pub(super) struct Machines;

impl native::Stored for Machines {
    type Source = Source;
    type Value = mutations::Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::REVOKE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        <mutations::Machines as native::Stored>::load(ctx, conn, declaration)
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_machine(
            conn,
            principal,
            <Self as native::resource_reply::StoredResource>::object(value).id,
            Permission::MachineManage,
        ))
    }
}

impl native::resource_reply::StoredResource for Machines {
    const READ_SOURCE: Source = Source::Governor;
    const ROLES: &'static single::Roles =
        <mutations::Machines as native::resource_reply::StoredResource>::ROLES;

    fn object(value: &mutations::Subject) -> ObjectRef {
        <mutations::Machines as native::resource_reply::StoredResource>::object(value)
    }

    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a mutations::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <mutations::Machines as native::resource_reply::StoredResource>::require_read(
            ctx, conn, principal, value,
        )
    }

    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        opened: &'a mutations::Subject,
        fresh: &'a mutations::Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        <mutations::Machines as native::resource_reply::StoredResource>::check_opened(
            ctx, conn, principal, opened, fresh,
        )
    }
}

fn receipt(id: Id) -> MachineRevokeResponse {
    MachineRevokeResponse::Receipt(MachineRevokeReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
    })
}

pub(super) async fn revoke(ctx: Ctx, (): (), (): ()) -> ApiResult<MachineRevokeResponse> {
    let id = ctx.id("id")?;
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let row = live_machine(&mut tx, id).await?;
    require_machine(
        &mut tx,
        initial.principal(),
        row.id,
        Permission::MachineManage,
    )
    .await?;
    let prepared = CertificateService::prepare_revocation(&mut tx, row.id).await?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    // The same physical and facade identities were held before restoring the original caller.
    // The native packet owns the command footprint; the shared resolver binds this stored subject.
    if native::modern(guard.principal().caller()) {
        let current = guard.operation_data::<native::Prepared<mutations::Subject>>()?;
        if <Machines as native::resource_reply::StoredResource>::object(&current.value).id != id {
            return Err(ApiError::internal("the prepared Machine subject changed"));
        }
    }
    require_machine(&mut tx, guard.principal(), id, Permission::MachineManage).await?;
    CertificateService::apply_revocation(
        &mut tx,
        prepared,
        Actor::User(guard.principal().user()),
        &origin(&ctx),
        guard.now().get(),
        || guard.check_deadline(&ctx),
    )
    .await??;
    guard.check_deadline(&ctx)?;
    let accepted = guard.principal().caller().clone();
    tx.commit().await.map_err(DbError::from)?;
    publication::receipt(&ctx, &receipt(id))?;
    // Disconnect is accepted-effect cleanup and runs exactly once after the native commit.
    ctx.app.link.let_go(id).await;
    Ok(
        match native::resource_reply::stored::<Machines>(&ctx, &accepted).await? {
            Some(Resource::Machine(machine)) => MachineRevokeResponse::full(machine),
            None => receipt(id),
            Some(_) => {
                return Err(ApiError::internal(
                    "the native Machine reply has another resource kind",
                ));
            }
        },
    )
}
