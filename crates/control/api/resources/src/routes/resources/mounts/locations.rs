//! Current location admission, once-only native data creation and completed Mount projection.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::http_origin;

use crate::key_operations::native::resource_reply;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use crate::registry::OperationFuture;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::mount_location_sources as sources;

use orch_api_contract::mount_location_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::Resource;

use orch_authz::Caller;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::mount_locations::Location;

use orch_engine::operation_scope::mount_locations::Set as NativeSet;

use orch_engine::operation_scope::mount_locations::SetSource;

use orch_engine::operation_scope::mount_locations::Delete as NativeDelete;

use orch_engine::operation_scope::mount_locations::Purge as NativePurge;

use orch_engine::operation_scope::mount_locations::Removal;

use orch_engine::operation_scope::mount_locations::RemoveSource;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::http_origin::HttpOrigin;
const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(sources::GROUPS),
        nodes: [
            (ObjectKind::Org, sources::NODES[0]),
            (ObjectKind::Project, sources::NODES[1]),
            (ObjectKind::Topic, sources::NODES[2]),
            (ObjectKind::Thread, sources::NODES[3]),
        ],
        containers: None,
    },
    sources: sources::REFERENCES,
};

fn mapped(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match error {
        orch_engine::operation_scope::continuing::Error::Native(error) => {
            crate::key_operations::snapshot_read::engine_error(error)
        }
        _ => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original mount location source is no longer current",
        ),
    }
}

async fn selected(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    body: MountLocationSetBody,
) -> ApiResult<LocationRow> {
    let id = ctx.id("id")?;
    let machine = ctx.id("machine_id")?;
    let located = orch_db::queries::environments::location_machine(tx, machine)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (kind, source, storage) = match body {
        MountLocationSetBody::Path { source } => {
            check_target(&source).map_err(|error| invalid("source", &error.to_string()))?;
            ("path".to_owned(), source, None)
        }
        MountLocationSetBody::Volume { storage } => {
            ("volume".to_owned(), format!("orch-mount-{id}"), storage)
        }
    };
    let location = LocationRow {
        mount_id: id,
        machine_id: machine,
        host_id: located.host_id,
        host_name: None,
        kind,
        source,
        storage,
        data_kind: None,
        created_by: principal.user(),
        created_at: principal.now().get(),
    };
    locating(tx, principal, &location).await?;
    if location.kind == "path" {
        let machine = orch_db::queries::resources::machine(tx, location.machine_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let policy = super::super::machines::reported_policy(tx, &machine)
            .await?
            .ok_or_else(|| {
                invalid(
                    "source",
                    "this machine has not offered any environment.roots",
                )
            })?;
        policy
            .check_root(std::path::Path::new(&location.source))
            .map_err(|_| {
                invalid(
                    "source",
                    "the mount path is outside this machine's environment.roots",
                )
            })?;
    }

    Ok(location)
}

pub(super) fn resolve_set<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: MountLocationSetBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let facts = facts(conn, ctx.id("id")?, ctx.id("machine_id")?).await?;
        let plan = native::bind_required(ctx, &sources::SET, &facts)?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        selected(ctx, conn, &principal, body).await?;
        Ok(plan)
    })
}

async fn facts(conn: &mut Conn, mount: Id, machine: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        native::Facts::one(
            Source::Mount,
            KeyTargetKind::Object(ObjectKind::Mount),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: mount,
                },
            )
            .await?,
        ),
        native::Facts::one(
            Source::Machine,
            KeyTargetKind::Object(ObjectKind::Machine),
            targets::machine_subject(conn, machine).await?,
        ),
    ])
}

fn receipt(id: Id, machine: Id) -> api::MountLocationSetResponse {
    api::MountLocationSetResponse::Receipt(api::MountLocationSetReceipt {
        projection: ReceiptProjection::Receipt,
        mount_id: id,
        machine_id: machine,
    })
}

pub(super) async fn set(
    ctx: Ctx,
    (): (),
    body: MountLocationSetBody,
) -> ApiResult<api::MountLocationSetResponse> {
    let id = ctx.id("id")?;
    let machine = ctx.id("machine_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut location = selected(&ctx, &mut tx, guard.principal(), body).await?;
    let previous = rows::locations(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|location| location.machine_id == machine);
    if let Some(previous) = &previous {
        location.data_kind = previous.data_kind.clone();
    }
    let same = previous.as_ref().is_some_and(|old| {
        old.kind == location.kind
            && old.source == location.source
            && old.storage == location.storage
    });
    if let Some(old) = previous.as_ref().filter(|_| !same) {
        locating(&mut tx, guard.principal(), old).await?;
        if old.kind == "volume" {
            return Err(invalid(
                "kind",
                "remove the old volume location with its confirmation first",
            ));
        }
    }
    let source = SetSource::capture(&mut tx, guard.principal().caller(), &location)
        .await
        .map_err(ApiError::engine)?;
    let raw = http_origin::capture_operation(&ctx, &mut tx, source.origin()).await?;
    let frame = raw
        .prepare::<NativeSet>(&mut tx, ctx.app.clock.as_ref(), source.clone())
        .await
        .map_err(mapped)?;
    let raw = frame
        .http_origin()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native location lost its HTTP source"))?;
    let accepted = frame.principal().caller().clone();
    if same {
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        frame
            .check_deadline(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        return reply(
            &ctx,
            &accepted,
            Completed {
                mount: id,
                machine,
                expected: previous.as_ref().map(Location::from_row),
            },
        )
        .await;
    }
    if location.kind == "volume" {
        let prepared = volume::prepare(&mut tx, &location, false)
            .await?
            .with_deadline(Time::new(
                ctx.app.clock.now()
                    + chrono::Duration::seconds(i64::from(
                        orch_link_contract::commands::CommandKind::NodeVolumeEnsure.deadline_secs(),
                    )),
            ));
        let command = prepared.command()?;
        let authority = http_origin::bind_source(
            &raw,
            &command,
            source.command(&command).map_err(ApiError::engine)?,
        )?;
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        let dispatcher = ctx.app.link.dispatcher();
        let pending = prepared
            .queue(
                &dispatcher,
                Some(authority.clone()),
                Box::new(|| {
                    frame
                        .check_deadline(ctx.app.clock.as_ref())
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }),
            )
            .await?;
        pending.wait(&dispatcher).await?;
        crate::key_operations::publication::receipt(&ctx, &receipt(id, machine))?;
        // The exact successful accepted command owns only this native bookkeeping tail.
        // It cannot admit another command or lend a current content/read permission.
        reconcile_set(&ctx, &source, &command, &authority).await?;
        let mut tx = ctx.app.db.begin().await?;
        if let Ok((_delivered, current)) =
            http_origin::delivery(&ctx, &mut tx, &authority, command.id).await
        {
            http_origin::check(&ctx, &current)?;
            tx.commit().await.map_err(DbError::from)?;
        }
    } else {
        frame
            .into_parts()
            .first_effect
            .check(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        rows::set_location(&mut tx, &location).await?;
        changed(&ctx, &mut tx, &row, &location).await?;
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        guard.check_deadline(&ctx)?;
    }
    if let Err(error) = ctx.app.engine.mount_resource_changed(id).await {
        tracing::warn!(mount=%id,%error,"mount location changes remain pending");
    }
    reply(
        &ctx,
        &accepted,
        Completed {
            mount: id,
            machine,
            expected: Some(Location::from_row(&location)),
        },
    )
    .await
}

async fn changed(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &MountRow,
    location: &LocationRow,
) -> ApiResult<()> {
    let candidates = rows::candidates(conn, row.id).await?;
    rows::changed(conn, &candidates, ctx.app.clock.now()).await?;
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::MountLocationAdded),
        row.id,
        home(row),
        "Added a mount location",
        json!({"mount":row.id,"machine":location.machine_id,"kind":location.kind}),
    )
    .await
}

async fn reconcile_set(
    ctx: &Ctx,
    source: &SetSource,
    command: &Outgoing,
    authority: &CommandAuthority,
) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    let row = rows::lock(&mut tx, source.location().mount).await?;
    successful(&mut tx, command, authority).await?;
    if let Some(row) = row {
        let current = rows::locations(&mut tx, &[row.id])
            .await?
            .into_iter()
            .find(|row| row.machine_id == source.location().machine);
        if source.can_record(current.as_ref()) {
            let location = source.location().row();
            rows::set_location(&mut tx, &location).await?;
            changed(ctx, &mut tx, &row, &location).await?;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[derive(Clone)]
struct Completed {
    mount: Id,
    machine: Id,
    expected: Option<Location>,
}

struct Projection;

impl resource_reply::ResourceProjection for Projection {
    type Source = Source;
    type Input = Completed;
    type Value = Completed;
    const READ_SOURCE: Source = Source::Mount;
    const ROLES: &'static single::Roles = &ROLES;
    fn declaration(_ctx: &Ctx, _input: &Completed) -> ApiResult<&'static Declaration<Source>> {
        Ok(&sources::SET)
    }
    fn load<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Completed,
    ) -> BoxFuture<'a, ApiResult<(Completed, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let current = rows::locations(conn, &[input.mount])
                .await?
                .into_iter()
                .find(|row| row.machine_id == input.machine);
            if input
                .expected
                .as_ref()
                .is_none_or(|old| current.as_ref().is_none_or(|row| !old.matches(row)))
            {
                return Err(ApiError::not_found());
            }
            Ok((
                input.clone(),
                facts(conn, input.mount, input.machine).await?,
            ))
        })
    }
    fn object(value: &Completed) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Mount,
            id: value.mount,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Completed,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Mount,
            value.mount,
            Permission::MountView,
        ))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        old: &'a Completed,
        fresh: &'a Completed,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.mount == fresh.mount
                && old.machine == fresh.machine
                && old.expected == fresh.expected)
        })
    }
}

async fn reply(
    ctx: &Ctx,
    accepted: &Caller,
    completed: Completed,
) -> ApiResult<api::MountLocationSetResponse> {
    let sparse = receipt(completed.mount, completed.machine);
    crate::key_operations::publication::receipt(ctx, &sparse)?;
    Ok(
        match resource_reply::with_input::<Projection>(ctx, accepted, completed).await? {
            Some(Resource::Mount(full)) => api::MountLocationSetResponse::Full(Box::new(full)),
            Some(_) => {
                return Err(ApiError::internal(
                    "a mount location returned another resource",
                ));
            }
            None => sparse,
        },
    )
}

enum RemovalFrame {
    Active(continuing::Frame<NativeDelete>),
    Kept(continuing::Frame<NativePurge>),
}

impl RemovalFrame {
    fn origin(&self) -> ApiResult<HttpOrigin> {
        match self {
            Self::Active(frame) => frame.http_origin().cloned(),
            Self::Kept(frame) => frame.http_origin().cloned(),
        }
        .ok_or_else(|| ApiError::internal("the native removal lost its HTTP source"))
    }
    fn deadline(&self, ctx: &Ctx) -> ApiResult<()> {
        match self {
            Self::Active(frame) => frame.check_deadline(ctx.app.clock.as_ref()),
            Self::Kept(frame) => frame.check_deadline(ctx.app.clock.as_ref()),
        }
        .map_err(mapped)
    }
    fn first(self, ctx: &Ctx) -> ApiResult<()> {
        match self {
            Self::Active(frame) => frame.into_parts().first_effect,
            Self::Kept(frame) => frame.into_parts().first_effect,
        }
        .check(ctx.app.clock.as_ref())
        .map_err(mapped)
    }
}

async fn prepare_removal(
    ctx: &Ctx,
    conn: &mut Conn,
    raw: &HttpOrigin,
    source: &RemoveSource,
) -> ApiResult<RemovalFrame> {
    match source.removal() {
        Removal::Active => raw
            .prepare::<NativeDelete>(conn, ctx.app.clock.as_ref(), source.clone())
            .await
            .map(RemovalFrame::Active),
        Removal::Kept(_) => raw
            .prepare::<NativePurge>(conn, ctx.app.clock.as_ref(), source.clone())
            .await
            .map(RemovalFrame::Kept),
    }
    .map_err(mapped)
}

async fn active(ctx: &Ctx, conn: &mut Conn) -> ApiResult<LocationRow> {
    let machine = ctx.id("machine_id")?;
    rows::locations(conn, &[ctx.id("id")?])
        .await?
        .into_iter()
        .find(|row| row.machine_id == machine)
        .ok_or_else(ApiError::not_found)
}

async fn validate_removal(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    location: &LocationRow,
    removal: &Removal,
    confirm: Option<&str>,
) -> ApiResult<()> {
    locating(conn, principal, location).await?;
    if location.kind == "volume" {
        let row = rows::mounts(conn, &[location.mount_id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        if confirm != Some(row.name.as_str()) {
            return Err(invalid(
                "confirm",
                if *removal == Removal::Active {
                    "type the mount's name before deleting its volume and data"
                } else {
                    "type the mount's name before deleting its kept volume and data"
                },
            ));
        }
        if *removal == Removal::Active {
            let uses = running_uses(conn, location.mount_id, location.machine_id).await?;
            if !uses.is_empty() {
                return Err(ApiError::new(ErrorCode::ResourceInUse,"environments still hold this data on the machine; detach it and remove any retained unit before purging it").with("environments",uses.len()));
            }
        }
    }
    let _ = ctx;
    Ok(())
}

pub(super) fn resolve_delete<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: api::MountLocationDeleteQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let location = active(ctx, conn).await?;
        let plan = native::bind_required(
            ctx,
            &sources::DELETE,
            &facts(conn, location.mount_id, location.machine_id).await?,
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        validate_removal(
            ctx,
            conn,
            &principal,
            &location,
            &Removal::Active,
            query.confirm.as_deref(),
        )
        .await?;
        Ok(plan)
    })
}

pub(super) async fn delete(
    ctx: Ctx,
    query: api::MountLocationDeleteQuery,
    (): (),
) -> ApiResult<()> {
    remove(ctx, query, Removal::Active).await
}

async fn remove(ctx: Ctx, query: api::MountLocationDeleteQuery, removal: Removal) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let location = match removal {
        Removal::Active => active(&ctx, &mut tx).await?,
        Removal::Kept(kept) => kept_location(&mut tx, id, kept).await?,
    };
    validate_removal(
        &ctx,
        &mut tx,
        guard.principal(),
        &location,
        &removal,
        query.confirm.as_deref(),
    )
    .await?;
    let source = RemoveSource::capture(
        &mut tx,
        guard.principal().caller(),
        &location,
        removal,
        query.confirm,
    )
    .await
    .map_err(ApiError::engine)?;
    let raw = http_origin::capture_operation(&ctx, &mut tx, source.origin()).await?;
    let frame = prepare_removal(&ctx, &mut tx, &raw, &source).await?;
    let raw = frame.origin()?;
    if location.kind == "volume" {
        let prepared = volume::prepare(&mut tx, &location, true)
            .await?
            .with_deadline(Time::new(
                ctx.app.clock.now()
                    + chrono::Duration::seconds(i64::from(
                        orch_link_contract::commands::CommandKind::NodeVolumeRemove.deadline_secs(),
                    )),
            ));
        let command = prepared.command()?;
        let authority = http_origin::bind_source(
            &raw,
            &command,
            source.command(&command).map_err(ApiError::engine)?,
        )?;
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        let dispatcher = ctx.app.link.dispatcher();
        let pending = prepared
            .queue(
                &dispatcher,
                Some(authority.clone()),
                Box::new(|| {
                    frame
                        .deadline(&ctx)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }),
            )
            .await?;
        pending.wait(&dispatcher).await?;
        crate::key_operations::publication::receipt(&ctx, &())?;
        let mut tx = ctx.app.db.begin().await?;
        let row = rows::lock(&mut tx, id).await?;
        successful(&mut tx, &command, &authority).await?;
        if let Some(row) = row
            && source
                .matches_current(&mut tx)
                .await
                .map_err(ApiError::engine)?
        {
            forget(&ctx, &mut tx, &row, &source).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
    } else {
        frame.first(&ctx)?;
        forget(&ctx, &mut tx, &row, &source).await?;
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        crate::key_operations::publication::receipt(&ctx, &())?;
    }
    if matches!(source.removal(), Removal::Active)
        && let Err(error) = ctx.app.engine.mount_resource_changed(id).await
    {
        tracing::warn!(mount=%id,%error,"mount removal remains pending");
    }
    Ok(())
}

async fn forget(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &MountRow,
    source: &RemoveSource,
) -> ApiResult<()> {
    let location = source.location();
    let (summary, details) = match source.removal() {
        Removal::Active => {
            rows::remove_location(conn, row.id, location.machine).await?;
            let candidates = rows::candidates(conn, row.id).await?;
            rows::changed(conn, &candidates, ctx.app.clock.now()).await?;
            (
                "Removed a mount location",
                json!({"mount":row.id,"machine":location.machine,"kind":location.kind}),
            )
        }
        Removal::Kept(kept) => {
            rows::forget_kept(conn, *kept).await?;
            (
                "Purged a mount's kept data",
                json!({"mount":row.id,"machine":location.machine,"kind":location.kind,"kept":kept}),
            )
        }
    };
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::MountLocationRemoved),
        row.id,
        home(row),
        summary,
        details,
    )
    .await
}

async fn kept_location(conn: &mut Conn, mount: Id, kept: Id) -> ApiResult<LocationRow> {
    let row = rows::kept(conn, &[mount])
        .await?
        .into_iter()
        .find(|row| row.id == kept)
        .ok_or_else(ApiError::not_found)?;
    Ok(LocationRow {
        mount_id: mount,
        machine_id: row.machine_id,
        host_id: None,
        host_name: None,
        kind: row.kind,
        source: row.source,
        storage: row.storage,
        data_kind: row.data_kind,
        created_by: row.kept_by,
        created_at: row.kept_at,
    })
}

async fn successful(
    conn: &mut Conn,
    command: &Outgoing,
    authority: &CommandAuthority,
) -> ApiResult<()> {
    let saved = orch_db::queries::link::lock_command_with_authority(conn, command.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let stored: CommandAuthority =
        serde_json::from_value(saved.authority.ok_or_else(ApiError::not_found)?)
            .map_err(ApiError::internal)?;
    if !stored.same_origin(authority)
        || saved.command.state != "succeeded"
        || saved.command.id != command.id
        || saved.command.machine_id != command.machine
        || saved.command.kind != command.kind.as_str()
        || saved.command.body != command.body
        || saved.command.serial_key() != command.key.wire().as_deref()
        || !orch_db::queries::link::command_authority_required(conn, command.id).await?
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

pub(super) fn resolve_purge<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: api::MountLocationDeleteQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let kept = ctx.id("kept_id")?;
        let location = kept_location(conn, id, kept).await?;
        let plan = native::bind_required(
            ctx,
            &sources::PURGE,
            &facts(conn, id, location.machine_id).await?,
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        validate_removal(
            ctx,
            conn,
            &principal,
            &location,
            &Removal::Kept(kept),
            query.confirm.as_deref(),
        )
        .await?;
        Ok(plan)
    })
}

pub(super) async fn purge(ctx: Ctx, query: api::MountLocationDeleteQuery, (): ()) -> ApiResult<()> {
    let kept = ctx.id("kept_id")?;
    remove(ctx, query, Removal::Kept(kept)).await
}
