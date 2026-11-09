//! Host and Machine moves keep their registration, under native ownership and empty custody.
use super::*;
use crate::{key_operations::targets, middleware::transaction_guard, routes::resources};
use futures::future::BoxFuture;
use orch_access_contract::{
    audit::AuditAction,
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission, ResourceKind},
};
use orch_api_contract::{
    common::ErrorCode,
    duplicate_sources::{self as sources, Source},
    duplicates::{
        CopySubject, CopySubjectKind, HostMoveOut, HostMoveOutPreview, MachineMoveOut,
        MachineMoveOutPreview, MoveOutBody, MoveOutConfirmation, MoveOutDeleteEffect,
        MoveOutDeletePlan,
    },
    operation_sources::Declaration,
    resource_groups::ResourceHomeSetReceipt,
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::{access, hierarchy, loss, resources::grouping},
};
use std::marker::PhantomData;

trait Family: Send + Sync + 'static {
    const KIND: ResourceKind;
    const SUBJECT: CopySubjectKind;
    const DECLARATION: &'static Declaration<Source>;
    const PREVIEW: &'static Declaration<Source>;
}
struct Hosts;
struct Machines;
struct Moving<F>(PhantomData<F>);
struct Preview<F>(PhantomData<F>);
impl Family for Hosts {
    const KIND: ResourceKind = ResourceKind::Host;
    const SUBJECT: CopySubjectKind = CopySubjectKind::Host;
    const DECLARATION: &'static Declaration<Source> = &sources::HOST_MOVE;
    const PREVIEW: &'static Declaration<Source> = &sources::HOST_MOVE_PREVIEW;
}
impl Family for Machines {
    const KIND: ResourceKind = ResourceKind::Machine;
    const SUBJECT: CopySubjectKind = CopySubjectKind::Machine;
    const DECLARATION: &'static Declaration<Source> = &sources::MACHINE_MOVE;
    const PREVIEW: &'static Declaration<Source> = &sources::MACHINE_MOVE_PREVIEW;
}
pub(super) fn mount(registry: &mut crate::registry::Registry) {
    registry
        .on_with_operation(HostMoveOut, effects::prepare::<Moving<Hosts>>, run::<Hosts>)
        .on_snapshot_body_read_with_optional_operation(
            HostMoveOutPreview,
            preview_prepare::<Hosts>,
            preview::<Hosts>,
        )
        .on_with_operation(
            MachineMoveOut,
            effects::prepare::<Moving<Machines>>,
            run::<Machines>,
        )
        .on_snapshot_body_read_with_optional_operation(
            MachineMoveOutPreview,
            preview_prepare::<Machines>,
            preview::<Machines>,
        );
}
struct Prepared {
    row: grouping::Row,
    object: ObjectRef,
    destination: hierarchy::NodeRow,
    inventory: loss::Inventory,
    plan: MoveOutDeletePlan,
}
async fn source_fact(
    conn: &mut Conn,
    object: ObjectRef,
    source: Source,
) -> ApiResult<native::Facts<Source>> {
    let target = if object.kind == ObjectKind::Machine {
        targets::machine_subject(conn, object.id).await?
    } else {
        targets::resource_identity(conn, object).await?
    };
    Ok(native::Facts::one(
        source,
        KeyTargetKind::Object(object.kind),
        target,
    ))
}
/// The same exact native loss/deletion inventory; no child is moved or silently forgotten.
fn require_empty(inventory: &loss::Inventory) -> ApiResult<()> {
    if !inventory.environments.is_empty()
        || !inventory.runtimes.is_empty()
        || !inventory.providers.is_empty()
        || !inventory.jobs.is_empty()
        || !inventory.mounts.is_empty()
        || !inventory.mount_kept.is_empty()
        || !inventory.threads.is_empty()
        || !inventory.provider_threads.is_empty()
    {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "Remove or move the environments, units and retained data before moving this registration.",
        ));
    }
    Ok(())
}
pub(super) async fn deletion_allowed(conn: &mut Conn, kind: ResourceKind, id: Id) -> ApiResult<()> {
    let inventory = loss::inventory(conn, kind.as_str(), id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require_empty(&inventory)
}
async fn require_native(conn: &mut Conn, principal: &Principal, value: &Prepared) -> ApiResult<()> {
    let kind = value
        .object
        .kind
        .resource()
        .ok_or_else(ApiError::not_found)?;
    if kind == ResourceKind::Machine {
        resources::machines::require_machine(
            conn,
            principal,
            value.object.id,
            Permission::MachineManage,
        )
        .await?;
    } else {
        resources::require(
            conn,
            principal,
            kind,
            value.object.id,
            Permission::HostManage,
        )
        .await?;
    }
    if !orch_engine::copy_out::owns(
        conn,
        principal,
        orch_engine::copy_out::Subject::Object(value.object),
    )
    .await
    .map_err(copy_error)?
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "Only a current owner can move this registration to another org.",
        ));
    }
    resources::creation_home::require(
        conn,
        principal,
        kind,
        Some(value.destination.id),
        "Choose an organization home for this registration.",
        "Choose an organization home for this registration.",
    )
    .await?;
    let (_, org) = orch_engine::copy_out::source_place(
        conn,
        orch_engine::copy_out::Subject::Object(value.object),
    )
    .await
    .map_err(copy_error)?;
    if org == Some(value.destination.org_id) {
        return Err(ApiError::invalid(
            "destination",
            "Use the existing Move within this organization.",
        ));
    }
    orch_engine::copy_out::authorize(
        conn,
        principal,
        orch_engine::copy_out::Subject::Object(value.object),
        Some(value.destination.id),
    )
    .await
    .map_err(copy_error)?;
    let inventory = loss::inventory(conn, kind.as_str(), value.object.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require_empty(&inventory)?;
    if inventory != value.inventory {
        return Err(moving::stale());
    }
    Ok(())
}
async fn prepare<F: Family>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: DuplicateBody,
    confirmation: Option<MoveOutConfirmation>,
    preview: bool,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    if body.name.is_some() {
        return Err(ApiError::invalid(
            "name",
            "A registration move keeps the existing machine and its name.",
        ));
    }
    let object = ObjectRef {
        kind: F::KIND.into(),
        id: ctx.id("id")?,
    };
    let declaration = if preview { F::PREVIEW } else { F::DECLARATION };
    if !preview
        && orch_db::queries::transaction_guard::in_transaction(conn)
        && !grouping::lock(conn, F::KIND, object.id).await?
    {
        return Err(ApiError::not_found());
    }
    let row = grouping::among(conn, &[(F::KIND.as_str().to_owned(), object.id)])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let home = destination(&body, row.home_id).ok_or_else(|| {
        ApiError::invalid(
            "destination",
            "Choose an organization home for this registration.",
        )
    })?;
    let destination = hierarchy::node(conn, home)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let mut facts = vec![
        source_fact(conn, object, Source::Subject).await?,
        native::Facts::one(
            Source::Destination,
            KeyTargetKind::Object(object.kind),
            targets::node(conn, home).await?,
        ),
    ];
    if preview {
        facts.push(source_fact(conn, object, Source::PreviewContent).await?);
    }
    let plan = native::bind(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    revision(&body, row.revision)?;
    let inventory = loss::inventory(conn, F::KIND.as_str(), object.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let plan = moving::plan(
        CopySubject {
            kind: F::SUBJECT,
            id: object.id,
        },
        DuplicateDestination::Node { id: home },
        row.revision,
        principal.user(),
        &body,
        serde_json::json!({"home":row.home_id,"owner":row.owner_id,"machine":row.origin_machine_id,"inventory":inventory}),
        vec![MoveOutDeleteEffect::KeepRegistration],
    )?;
    let value = Prepared {
        row,
        object,
        destination,
        inventory,
        plan,
    };
    require_native(conn, &principal, &value).await?;
    if let Some(confirmation) = confirmation {
        moving::confirm(&value.plan, &confirmation)?;
    }
    if !preview {
        resources::creation_home::lock(conn, Some(home)).await?;
    }
    current(ctx, principal.caller())?;
    Ok((value, facts))
}
impl<F: Family> effects::Effect for Moving<F> {
    type Source = Source;
    type Query = ();
    type Body = MoveOutBody;
    type Value = Prepared;
    type Output = ResourceHomeSetReceipt;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        F::DECLARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: MoveOutBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare::<F>(
            ctx,
            conn,
            caller,
            body.copy,
            Some(body.delete_plan),
            false,
        ))
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_native(conn, principal, value))
    }
}
async fn fresh_page(
    conn: &mut Conn,
    object: ObjectRef,
    by: Id,
    now: orch_types::Time,
) -> ApiResult<()> {
    let page = access::PageKey::new(object.kind.as_str(), object.id, "live");
    access::reset_page(conn, &page, by, now.get()).await?;
    for entry in access::owner_entries(conn, std::slice::from_ref(&page)).await? {
        access::remove_entry(conn, entry.id, by, now.get()).await?;
    }
    let host_owner = if object.kind == ObjectKind::Machine {
        sqlx::query_scalar::<_, Option<Id>>("SELECT owner_id FROM hosts WHERE machine_id=$1")
            .bind(object.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
    } else {
        None
    };
    if let Some(owner) = host_owner {
        // A Host's physical Machine is not a separately creatable resource. Its reset
        // mirrors the actual governing Host owner without a new-resource lookup.
        let owner = owner.ok_or_else(|| ApiError::internal("the governing Host has no owner"))?;
        orch_db::queries::resources::set_resource_owner(
            conn,
            object.kind.as_str(),
            object.id,
            owner,
            now.get(),
        )
        .await?;
        let options = serde_json::json!({});
        access::insert_entry(
            conn,
            &access::NewEntry {
                id: orch_types::new_id(),
                page: &page,
                profile: "any",
                permission: Permission::AccessOwn.as_str(),
                holder_kind: "user",
                holder_id: Some(owner),
                holder_role: None,
                options: &options,
                expires_at: None,
                note: "",
                source: "creation",
                created_by: Some(by),
                at: now.get(),
            },
        )
        .await?;
        access::bump_epoch(conn).await?;
    } else {
        orch_db::queries::resources::set_resource_owner(
            conn,
            object.kind.as_str(),
            object.id,
            by,
            now.get(),
        )
        .await?;
        orch_authz::ownership::start_resource(
            conn,
            object,
            by,
            orch_authz::ownership::NewAccess::Defaults,
            now,
        )
        .await
        .map_err(ApiError::internal)?;
    }
    grouping::set_tags(
        conn,
        object.kind.resource().ok_or_else(ApiError::not_found)?,
        object.id,
        &[],
        now.get(),
    )
    .await?;
    Ok(())
}
impl<F: Family> effects::Apply for Moving<F> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<ResourceHomeSetReceipt>>> {
        Box::pin(async move {
            let boundary = orch_engine::copy_out::authorize(
                conn,
                principal,
                orch_engine::copy_out::Subject::Object(value.object),
                Some(value.destination.id),
            )
            .await
            .map_err(copy_error)?
            .ok_or_else(|| {
                ApiError::internal("the registration's cross-root source boundary disappeared")
            })?;
            let deadline = before.deadline();
            before.check()?;
            orch_engine::copy_out::record(
                conn,
                boundary,
                orch_db::audit::Actor::User(principal.user()),
                &crate::sign_in::origin(ctx),
                value.object.id,
                ctx.now,
            )
            .await
            .map_err(ApiError::engine)?;
            deadline.check()?;
            grouping::set_home(
                conn,
                F::KIND,
                value.object.id,
                value.destination.id,
                value.destination.org_id,
                ctx.now,
            )
            .await?;
            let counterpart = match F::KIND {
                ResourceKind::Host => value.inventory.machine.map(|id| ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                }),
                ResourceKind::Machine => value.inventory.host.map(|id| ObjectRef {
                    kind: ObjectKind::Host,
                    id,
                }),
                _ => None,
            };
            // Reset the governing Host before mirroring its owner to the physical Machine,
            // including when the move entered through the Machine operation.
            let (first, second) = match counterpart {
                Some(host) if host.kind == ObjectKind::Host => (host, Some(value.object)),
                other => (value.object, other),
            };
            fresh_page(conn, first, principal.user(), orch_types::Time::new(ctx.now)).await?;
            if let Some(object) = second {
                fresh_page(
                    conn,
                    object,
                    principal.user(),
                    orch_types::Time::new(ctx.now),
                )
                .await?;
            }
            let action = orch_db::audit::action!(AuditAction::ResourceHomeChanged);
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(principal.user()),
                action.as_str(),
                F::KIND.as_str(),
                Some(value.object.id),
                "Moved a registration to another organization",
            );
            entry.history = action.is_history_line();
            entry.node_id = Some(value.destination.id);
            entry.org_id = Some(value.destination.org_id);
            entry.details = serde_json::json!({"from_home":value.row.home_id,"home_id":value.destination.id,"kept_registration":true});
            orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(ResourceHomeSetReceipt {
                projection: ReceiptProjection::Receipt,
                object_id: value.object.id,
                kind: F::KIND.as_str().to_owned(),
                home_id: value.destination.id,
                changed: true,
            }))
        })
    }
}
async fn run<F: Family>(ctx: Ctx, (): (), _: MoveOutBody) -> ApiResult<ResourceHomeSetReceipt> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Moving<F>>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&done.ctx, &done.output)?;
    Ok(done.output)
}
fn preview_prepare<'a, F: Family>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DuplicateBody,
) -> crate::registry::OptionalOperationFuture<'a> {
    native::selected::<Preview<F>>(ctx, conn, caller, body, ())
}
async fn preview<F: Family>(ctx: Ctx, (): (), body: DuplicateBody) -> ApiResult<MoveOutDeletePlan> {
    native::selected_read::<Preview<F>>(ctx, body, ()).await
}
impl<F: Family> native::Selected for Preview<F> {
    type Source = Source;
    type Query = DuplicateBody;
    type Value = Prepared;
    type Reply = MoveOutDeletePlan;
    const DECLARATION: &'static Declaration<Source> = F::PREVIEW;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        body: &'a DuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare::<F>(ctx, conn, caller, body.clone(), None, true))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<MoveOutDeletePlan>> {
        Box::pin(async move {
            require_native(conn, principal, value).await?;
            Ok(value.plan.clone())
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a DuplicateBody,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.plan == fresh.plan) })
    }
}
