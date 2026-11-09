//! The actual Thing and optional native content of the ownership writer family.

use crate::key_operations::targets;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::{operations::NameRoles, subject};
use crate::routes::nodes::{projection::Projection, view};
use crate::routes::resources::projection::single as native;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::ownership::{self as api, TransferBody};
use orch_api_contract::ownership_sources::TransferSource as Role;
use orch_authz::things::{self, Thing};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError, Tx};

struct PreparedTarget {
    thing: Thing,
    kind: ObjectKind,
}
fn authz_error(error: orch_authz::Error) -> ApiError {
    match error {
        orch_authz::Error::NotFound(_) => ApiError::not_found(),
        other => authorization::restoration_error(other),
    }
}
async fn base(ctx: &Ctx, conn: &mut Conn, thing: Thing) -> ApiResult<OperationPlan> {
    let (graph, site) = things::site(conn, thing).await.map_err(authz_error)?;
    if let Thing::Other(object) = thing
        && object.kind != site.kind
    {
        return Err(ApiError::not_found());
    }
    let target = targets::from_site(&graph, &site)?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, target.object.kind, std::slice::from_ref(&target))?;
    if let Thing::Other(object) = thing {
        match native::subject(conn, object).await {
            Ok(read) => plan.bind(
                1,
                orch_access_contract::key_scope::KeyTargetKind::Object(object.kind),
                &[read],
            )?,
            Err(error) if matches!(error.problem.status, 404 | 410) => {}
            Err(error) => return Err(error),
        }
    } else {
        plan.bind(1, target.object.kind, std::slice::from_ref(&target))?;
    }
    Ok(plan.with_data(PreparedTarget {
        thing,
        kind: site.kind,
    }))
}
pub(crate) fn node<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    _body: TransferBody,
) -> OperationFuture<'a> {
    Box::pin(async move { base(ctx, conn, Thing::Node(ctx.id("id")?)).await })
}
pub(super) fn object<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    _body: TransferBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let Thing::Other(object) = subject::named(ctx)?.thing else {
            return Err(ApiError::not_found());
        };
        super::require_transfer_kind(object.kind)?;
        base(ctx, conn, Thing::Other(object)).await
    })
}

/// NodeTransfer's native page writer always locks this same Node. The existing
/// unsupported Thread path still returns its native refusal without a new row wait.
pub(crate) async fn node_action(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Guard)> {
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let prepared = initial.operation_data::<PreparedTarget>()?;
    if prepared.kind == ObjectKind::Thread {
        let current = ctx
            .for_operation_checkpoint(initial.user().clone(), initial.now().get())
            .with_admitted_caller(initial.principal().caller().clone())?;
        return Ok((current, tx, initial));
    }
    orch_authz::pages::lock_change_target(&mut tx, prepared.thing.target())
        .await
        .map_err(authz_error)?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let fresh = guard.operation_data::<PreparedTarget>()?;
    if prepared.thing != fresh.thing || prepared.kind != fresh.kind {
        return Err(DbError::Retry.into());
    }
    Ok((current, tx, guard))
}
pub(crate) fn receipt(
    ctx: &Ctx,
    owner: orch_types::Id,
    changed: bool,
) -> ApiResult<api::OwnershipTransferReceipt> {
    let prepared = ctx.operation_data::<PreparedTarget>()?;
    Ok(api::OwnershipTransferReceipt {
        projection: ReceiptProjection::Receipt,
        object_id: prepared.thing.id(),
        kind: prepared.kind.as_str().to_owned(),
        owner_id: owner,
        changed,
    })
}
pub(crate) async fn node_reply(
    ctx: &Ctx,
    principal: &Principal,
    receipt: api::OwnershipTransferReceipt,
) -> ApiResult<api::NodeTransferResponse> {
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    let work = async {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let Some(identity) = crate::key_operations::admission::response_caller(
            ctx,
            &mut tx,
            principal.caller(),
            ctx.route.name,
            principal.now(),
        )
        .await?
        else {
            return Ok::<_, ApiError>(None);
        };
        let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
        let target = targets::node(&mut tx, receipt.object_id).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        for index in [0, 1] {
            plan.bind(index, target.object.kind, std::slice::from_ref(&target))?;
        }
        let Some(reader) = crate::routes::access::page_operations::response_principal(
            &current,
            &mut tx,
            &identity,
            &plan,
            1,
            target.object.kind,
            &target,
        )
        .await?
        else {
            return Ok(None);
        };
        let current = current.with_admitted_caller(reader.caller().clone())?;
        let policy = crate::routes::nodes::policy_at(&mut tx, receipt.object_id).await?;
        let Some(node) = view::one(
            &mut tx,
            &reader,
            receipt.object_id,
            identity.now.get(),
            policy,
        )
        .await?
        else {
            return Ok(None);
        };
        let projection = Projection::response(&current, reader.caller(), plan, 2);
        let mut nodes = projection.project(&mut tx, &reader, vec![node]).await?;
        let Some(node) = nodes.pop() else {
            return Ok(None);
        };
        let plan = projection
            .finish_response(&mut tx, std::slice::from_ref(&node))
            .await?;
        let value = plan
            .read_response(&current, &mut tx, reader.caller(), move |_, _| {
                Box::pin(async move { Ok(node) })
            })
            .await?;
        tx.commit().await.map_err(DbError::from)?;
        identity.check_deadline(&ctx.app)?;
        Ok(value)
    };
    let value = crate::key_operations::completed_projection::run(ctx, async {
        match work.await {
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(None),
            other => other,
        }
    })
    .await?;
    Ok(match value {
        Some(node) => api::NodeTransferResponse::Full(Box::new(node)),
        None => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            api::NodeTransferResponse::Receipt(receipt)
        }
    })
}

const OBJECT_ROLES: native::Roles = native::Roles {
    names: NameRoles {
        groups: Some("transfer_groups"),
        nodes: [
            (ObjectKind::Org, "transfer_names_orgs"),
            (ObjectKind::Project, "transfer_names_projects"),
            (ObjectKind::Topic, "transfer_names_topics"),
            (ObjectKind::Thread, "transfer_names_threads"),
        ],
        containers: Some("transfer_names_containers"),
    },
    sources: "transfer_sources",
};

struct Transferred;
impl crate::key_operations::native::resource_reply::ResourceProjection for Transferred {
    type Source = Role;
    type Input = orch_access_contract::decisions::ObjectRef;
    type Value = orch_access_contract::decisions::ObjectRef;
    const READ_SOURCE: Self::Source = Role::Content;
    const ROLES: &'static native::Roles = &OBJECT_ROLES;
    fn declaration(
        _: &Ctx,
        _: &Self::Input,
    ) -> ApiResult<&'static orch_api_contract::operation_sources::Declaration<Self::Source>> {
        Ok(&orch_api_contract::ownership_sources::OBJECT_TRANSFER)
    }
    fn load<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        object: &'a Self::Input,
    ) -> crate::key_operations::native::SourceFuture<'a, Self::Value, Self::Source> {
        Box::pin(async move {
            let (graph, site) = things::site(conn, Thing::Other(*object))
                .await
                .map_err(authz_error)?;
            if object.kind != site.kind {
                return Err(ApiError::not_found());
            }
            let target = targets::from_site(&graph, &site)?;
            let read = native::subject(conn, *object).await?;
            let family = orch_access_contract::key_scope::KeyTargetKind::Object(object.kind);
            Ok((
                *object,
                vec![
                    crate::key_operations::native::Facts::one(Role::Target, family, target),
                    crate::key_operations::native::Facts::one(Role::Content, family, read),
                ],
            ))
        })
    }
    fn object(value: &Self::Value) -> orch_access_contract::decisions::ObjectRef {
        *value
    }
    fn require_read<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> futures::future::BoxFuture<'a, ApiResult<()>> {
        Box::pin(subject::require_view(
            conn,
            principal,
            Thing::Other(*value),
            orch_authz::Policy::default(),
        ))
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        value: &'a Self::Value,
        read: &'a crate::key_operations::native::resource_reply::ResponseRead,
    ) -> futures::future::BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if value.kind == ObjectKind::Skill {
                crate::routes::resources::skills::after_single_response(ctx, read).await?;
            }
            Ok(())
        })
    }
}

/// Keep the shared completed singleton's native styles, current Read, maintenance and publication.
pub(super) async fn object_reply(
    ctx: &Ctx,
    principal: &Principal,
    object: orch_access_contract::decisions::ObjectRef,
    receipt: api::OwnershipTransferReceipt,
) -> ApiResult<api::ObjectTransferResponse> {
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    Ok(
        match crate::key_operations::native::resource_reply::with_input::<Transferred>(
            ctx,
            principal.caller(),
            object,
        )
        .await?
        {
            Some(value) => api::ObjectTransferResponse::Full(Box::new(value)),
            None => api::ObjectTransferResponse::Receipt(receipt),
        },
    )
}
