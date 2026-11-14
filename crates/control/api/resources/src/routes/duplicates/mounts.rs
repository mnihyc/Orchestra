//! Native mount copies and atomic moves reuse creation, uses and physical deletion guards.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::targets;

use crate::routes::resources;

use crate::routes::resources::mounts;

use crate::routes::resources::mounts::creation;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::duplicate_sources as sources;

use orch_api_contract::duplicate_sources::Source;

use orch_api_contract::duplicates::CopySubject;

use orch_api_contract::duplicates::CopySubjectKind;

use orch_api_contract::duplicates::MountMoveOut;

use orch_api_contract::duplicates::MountMoveOutPreview;

use orch_api_contract::duplicates::MoveOutBody;

use orch_api_contract::duplicates::MoveOutDeleteEffect;

use orch_api_contract::duplicates::MoveOutDeletePlan;

use orch_api_contract::mounts::MountCreateBody;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::resources::mounts as rows;

use std::collections::BTreeSet;
pub(super) struct Mounts;

struct Moving;

struct Preview;

impl Configuration for Mounts {
    const KIND: DuplicateKind = DuplicateKind::Mount;
    const REPLAY: &'static str = "mount_duplicate";
}

impl Configuration for Moving {
    const KIND: DuplicateKind = DuplicateKind::Mount;
    const REPLAY: &'static str = "mount_move_out";
    const MOVING: bool = true;
}

pub(super) fn mount_moves(registry: &mut crate::registry::Registry) {
    register::<Moving, _>(registry, MountMoveOut);
    registry.on_snapshot_body_read_with_optional_operation(
        MountMoveOutPreview,
        preview_prepare,
        preview,
    );
}

pub(super) struct Prepared {
    row: rows::MountRow,
    home: Option<Id>,
    new: creation::Proposal,
    tags: Vec<Id>,
    delete: Option<MoveOutDeletePlan>,
}

enum Intent {
    Copy(DuplicateBody),
    Preview(DuplicateBody),
    Move(MoveOutBody),
}

impl Intent {
    fn declaration(&self) -> &'static Declaration<Source> {
        match self {
            Self::Copy(_) => &sources::MOUNT,
            Self::Preview(_) => &sources::MOUNT_MOVE_PREVIEW,
            Self::Move(_) => &sources::MOUNT_MOVE,
        }
    }
}

async fn machine_facts(
    conn: &mut Conn,
    locations: &[rows::LocationRow],
) -> ApiResult<native::Facts<Source>> {
    let ids = locations
        .iter()
        .map(|location| location.machine_id)
        .collect::<BTreeSet<_>>();
    let targets = targets::machine_subjects(conn, &ids.into_iter().collect::<Vec<_>>())
        .await?
        .into_values()
        .collect::<Vec<_>>();
    Ok(native::Facts::collection(
        Source::Machine,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Machine), targets)]
        },
    ))
}

fn delete_plan(
    row: &rows::MountRow,
    principal: &Principal,
    body: &DuplicateBody,
    home: Option<Id>,
    locations: &[rows::LocationRow],
) -> ApiResult<MoveOutDeletePlan> {
    let locations = locations
        .iter()
        .map(|location| {
            serde_json::json!({"machine":location.machine_id,"kind":location.kind,
        "source":location.source,"storage":location.storage,"data_kind":location.data_kind})
        })
        .collect::<Vec<_>>();
    moving::plan(
        CopySubject {
            kind: CopySubjectKind::Mount,
            id: row.id,
        },
        home.map_or(DuplicateDestination::System, |id| {
            DuplicateDestination::Node { id }
        }),
        row.revision,
        principal.user(),
        body,
        serde_json::json!({"source_home":row.home_id,"source_org":row.org_id,"target":row.target,"read_only":row.read_only,
            "fate":row.default_fate,"locations":locations}),
        vec![MoveOutDeleteEffect::RemoveSource],
    )
}

async fn prepare_native(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: Intent,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let declaration = intent.declaration();
    let preview = matches!(&intent, Intent::Preview(_));
    let moving = !matches!(&intent, Intent::Copy(_));
    let (body, confirmation) = match intent {
        Intent::Copy(body) | Intent::Preview(body) => (body, None),
        Intent::Move(body) => (body.copy, Some(body.delete_plan)),
    };
    let id = ctx.id("id")?;
    let source = if preview {
        rows::mounts(conn, &[id]).await?.pop()
    } else {
        rows::lock(conn, id).await?
    }
    .ok_or_else(ApiError::not_found)?;
    let home = destination(&body, source.home_id);
    let mut facts = origins(conn, ObjectKind::Mount, id, home).await?;
    let known = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    resources::require(
        conn,
        &principal,
        ResourceKind::Mount,
        id,
        Permission::MountView,
    )
    .await?;
    revision(&body, source.revision)?;
    creation::require_home(conn, &principal, home).await?;
    let org = copy_to(conn, &principal, subject(DuplicateKind::Mount, id), home).await?;
    let locations = if moving {
        if org == source.org_id {
            return Err(ApiError::invalid(
                "destination",
                "use the existing Move within this organization",
            ));
        }
        if !owns(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Mount,
                id,
            },
        )
        .await?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "only an actual owner can move this into another organization",
            ));
        }
        resources::require(
            conn,
            &principal,
            ResourceKind::Mount,
            id,
            Permission::MountManage,
        )
        .await?;
        let locations = mounts::deletion_inventory(ctx, conn, &principal, &source).await?;
        facts.push(machine_facts(conn, &locations).await?);
        locations
    } else {
        Vec::new()
    };
    if preview {
        facts.push(native::Facts::one(
            Source::PreviewContent,
            KeyTargetKind::Object(ObjectKind::Mount),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id,
                },
            )
            .await?,
        ));
    }
    let bound = native::bind(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &bound).await?;
    if moving {
        mounts::require_deletion_locations(conn, &principal, &locations).await?;
    }
    let delete = if moving {
        let plan = delete_plan(&source, &principal, &body, home, &locations)?;
        if let Some(confirmation) = &confirmation {
            moving::confirm(&plan, confirmation)?;
        }
        Some(plan)
    } else {
        None
    };
    let name = match &body.name {
        Some(name) => name.clone(),
        None => copy_name(ctx, conn, principal.caller(), org, &source.name).await?,
    };
    let new = creation::prepare_native(
        ctx,
        conn,
        &principal,
        MountCreateBody {
            name,
            description: Some(source.description.clone()),
            home_id: home,
            target: source.target.clone(),
            read_only: Some(source.read_only),
            default_fate: Some(
                serde_json::from_value(serde_json::json!(source.default_fate))
                    .map_err(ApiError::internal)?,
            ),
            access: None,
        },
    )
    .await?;
    let tags = copy_tags(conn, ResourceKind::Mount, id, home).await?;
    current(ctx, principal.caller())?;
    Ok((
        Prepared {
            row: source,
            home,
            new,
            tags,
            delete,
        },
        facts,
    ))
}

async fn require_native(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
) -> ApiResult<()> {
    resources::require(
        conn,
        principal,
        ResourceKind::Mount,
        value.row.id,
        Permission::MountView,
    )
    .await?;
    copy_to(
        conn,
        principal,
        subject(DuplicateKind::Mount, value.row.id),
        value.home,
    )
    .await?;
    creation::require_home(conn, principal, value.home).await?;
    if value.delete.is_some() {
        if !owns(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Mount,
                id: value.row.id,
            },
        )
        .await?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "only a current owner can complete the move",
            ));
        }
        resources::require(
            conn,
            principal,
            ResourceKind::Mount,
            value.row.id,
            Permission::MountManage,
        )
        .await?;
        let locations = mounts::deletion_inventory(ctx, conn, principal, &value.row).await?;
        mounts::require_deletion_locations(conn, principal, &locations).await?;
    }
    Ok(())
}

async fn apply_native<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    principal: &'a Principal,
    value: &'a Prepared,
    before: effects::FirstEffect<'a>,
) -> ApiResult<effects::Outcome<Id>> {
    let boundary = if value.delete.is_some() {
        copy_boundary(
            conn,
            principal,
            subject(DuplicateKind::Mount, value.row.id),
            value.home,
        )
        .await?
    } else {
        None
    };
    let deadline = before.deadline();
    let made = creation::insert(ctx, conn, principal, &value.new, move || before.check()).await?;
    let id = made.id();
    deadline.check()?;
    if !value.tags.is_empty() {
        orch_db::queries::resources::grouping::set_tags(
            conn,
            ResourceKind::Mount,
            id,
            &value.tags,
            ctx.now,
        )
        .await?;
        deadline.check()?;
    }
    if value.delete.is_some() {
        let boundary = boundary
            .ok_or_else(|| ApiError::internal("the native moving source boundary disappeared"))?;
        orch_engine::copy_out::record(
            conn,
            boundary,
            orch_db::audit::Actor::User(principal.user()),
            &crate::sign_in::origin(ctx),
            id,
            ctx.now,
        )
        .await
        .map_err(ApiError::engine)?;
        deadline.check()?;
        mounts::delete_native(ctx, conn, principal, &value.row).await?;
        deadline.check()?;
    }
    Ok(effects::Outcome::Applied(id))
}

macro_rules! mount_effect {
    ($family:ty,$body:ty,$declaration:expr,$intent:ident) => {
        impl effects::Effect for $family {
            type Source = Source;
            type Query = ();
            type Body = $body;
            type Value = Prepared;
            type Output = Id;
            fn declaration(_: &Prepared) -> &'static Declaration<Source> {
                $declaration
            }
            fn prepare<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                caller: &'a Caller,
                (): (),
                body: $body,
            ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
                Box::pin(prepare_native(ctx, conn, caller, Intent::$intent(body)))
            }
            fn require<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
            ) -> BoxFuture<'a, ApiResult<()>> {
                Box::pin(require_native(ctx, conn, principal, value))
            }
        }
        impl effects::Apply for $family {
            fn apply<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
                before: effects::FirstEffect<'a>,
            ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
                Box::pin(apply_native(ctx, conn, principal, value, before))
            }
        }
    };
}

mount_effect!(Mounts, DuplicateBody, &sources::MOUNT, Copy);

mount_effect!(Moving, MoveOutBody, &sources::MOUNT_MOVE, Move);

fn preview_prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DuplicateBody,
) -> crate::registry::OptionalOperationFuture<'a> {
    native::selected::<Preview>(ctx, conn, caller, body, ())
}

async fn preview(ctx: Ctx, (): (), body: DuplicateBody) -> ApiResult<MoveOutDeletePlan> {
    native::selected_read::<Preview>(ctx, body, ()).await
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = DuplicateBody;
    type Value = Prepared;
    type Reply = MoveOutDeletePlan;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = &sources::MOUNT_MOVE_PREVIEW;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        body: &'a DuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_native(
            ctx,
            conn,
            caller,
            Intent::Preview(body.clone()),
        ))
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
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<MoveOutDeletePlan>> {
        Box::pin(async move {
            require_native(ctx, conn, principal, value).await?;
            value
                .delete
                .clone()
                .ok_or_else(|| ApiError::internal("the native move preview has no deletion plan"))
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
        Box::pin(async move {
            Ok(opened.row.id == fresh.row.id
                && opened.home == fresh.home
                && opened.delete == fresh.delete)
        })
    }
}

async fn copy_name(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    org: Option<Id>,
    name: &str,
) -> ApiResult<String> {
    let mut number = 1_u64;
    loop {
        let suffix = if number == 1 {
            "-copy".to_owned()
        } else {
            format!("-copy-{number}")
        };
        let base: String = name.chars().take(32 - suffix.len()).collect();
        let candidate = format!("{base}{suffix}");
        rows::lock_creation_name(conn, org, &candidate).await?;
        current(ctx, caller)?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mounts WHERE org_id IS NOT DISTINCT FROM $1 AND name=$2)",
        )
        .bind(org)
        .bind(&candidate)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        current(ctx, caller)?;
        if !exists {
            return Ok(candidate);
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| ApiError::internal("the copy name space is exhausted"))?;
    }
}
