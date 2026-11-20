//! Native template copying and atomic owner-only cross-root moves share their preparation and writers.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::targets;

use crate::routes::resources;

use crate::routes::resources::templates;

use crate::routes::resources::templates::creation;

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

use orch_api_contract::duplicates::MoveOutBody;

use orch_api_contract::duplicates::MoveOutDeleteEffect;

use orch_api_contract::duplicates::MoveOutDeletePlan;

use orch_api_contract::duplicates::TemplateMoveOut;

use orch_api_contract::duplicates::TemplateMoveOutPreview;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::TemplateCreateBody;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::resources::grouping;

use orch_db::queries::resources::routes as rows;

use orch_db::queries::resources::routes::TemplateRow;
pub(super) struct Templates;

pub(super) struct Moving;

struct Preview;

impl Configuration for Templates {
    const KIND: DuplicateKind = DuplicateKind::Template;
    const REPLAY: &'static str = "template_duplicate";
}

impl Configuration for Moving {
    const KIND: DuplicateKind = DuplicateKind::Template;
    const REPLAY: &'static str = "template_move_out";
    const MOVING: bool = true;
}

pub(super) fn mount_moves(registry: &mut crate::registry::Registry) {
    register::<Moving, _>(registry, TemplateMoveOut);
    registry.on_snapshot_body_read_with_optional_operation(
        TemplateMoveOutPreview,
        preview_prepare,
        preview,
    );
}

pub(super) struct Prepared {
    row: TemplateRow,
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
            Self::Copy(_) => &sources::TEMPLATE,
            Self::Preview(_) => &sources::TEMPLATE_MOVE_PREVIEW,
            Self::Move(_) => &sources::TEMPLATE_MOVE,
        }
    }
}

fn administration(restricted: bool) -> native::Facts<Source> {
    native::Facts::collection(
        Source::DriverAdministration,
        if restricted {
            vec![(
                KeyTargetKind::Object(ObjectKind::Template),
                vec![targets::system()],
            )]
        } else {
            Vec::new()
        },
    )
}

fn deletion_plan(
    row: &TemplateRow,
    principal: &Principal,
    body: &DuplicateBody,
    home: Option<Id>,
) -> ApiResult<MoveOutDeletePlan> {
    moving::plan(
        CopySubject {
            kind: CopySubjectKind::Template,
            id: row.id,
        },
        home.map_or(DuplicateDestination::System, |id| {
            DuplicateDestination::Node { id }
        }),
        row.revision,
        principal.user(),
        body,
        serde_json::json!({"source_home":row.home_id,"source_org":row.org_id,
            "driver":row.driver,"kind":row.kind,"spec":row.spec,"units":row.units}),
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
    if !preview {
        let locked: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM env_templates WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
        if locked.is_none() {
            return Err(ApiError::not_found());
        }
    }
    let row = rows::templates(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let home = destination(&body, row.home_id);
    let mut facts = origins(conn, ObjectKind::Template, id, home).await?;
    facts.push(administration(
        templates::restricted_driver(conn, &row.driver).await?,
    ));
    if preview {
        facts.push(native::Facts::one(
            Source::PreviewContent,
            KeyTargetKind::Object(ObjectKind::Template),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Template,
                    id,
                },
            )
            .await?,
        ));
    }
    let plan = native::bind(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    resources::require(
        conn,
        &principal,
        ResourceKind::Template,
        id,
        Permission::TemplateView,
    )
    .await?;
    revision(&body, row.revision)?;
    templates::creation_authority(conn, &principal, home, &row.driver).await?;
    let org = copy_to(conn, &principal, subject(DuplicateKind::Template, id), home).await?;
    let delete = if moving {
        if org == row.org_id {
            return Err(ApiError::invalid(
                "destination",
                "use the existing Move within this organization",
            ));
        }
        if !owns(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Template,
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
            ResourceKind::Template,
            id,
            Permission::TemplateManage,
        )
        .await?;
        templates::deletion_allowed(ctx, &row)?;
        let plan = deletion_plan(&row, &principal, &body, home)?;
        if let Some(confirmation) = &confirmation {
            moving::confirm(&plan, confirmation)?;
        }
        Some(plan)
    } else {
        None
    };
    let new = creation::prepare_native(
        conn,
        &principal,
        TemplateCreateBody {
            name: display_name(&body, &row.name),
            description: Some(row.description.clone()),
            home_id: home,
            kind: Some(
                serde_json::from_value(serde_json::json!(row.kind)).map_err(ApiError::internal)?,
            ),
            driver: row.driver.clone(),
            spec: row.spec.clone(),
            access: None,
        },
    )
    .await?;
    facts[2] = administration(new.restricted());
    let tags = copy_tags(conn, ResourceKind::Template, id, home).await?;
    current(ctx, principal.caller())?;
    Ok((
        Prepared {
            row,
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
        ResourceKind::Template,
        value.row.id,
        Permission::TemplateView,
    )
    .await?;
    copy_to(
        conn,
        principal,
        subject(DuplicateKind::Template, value.row.id),
        value.home,
    )
    .await?;
    if value.delete.is_some() {
        if !owns(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Template,
                id: value.row.id,
            },
        )
        .await?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "only a current owner can complete the cross-organization move",
            ));
        }
        resources::require(
            conn,
            principal,
            ResourceKind::Template,
            value.row.id,
            Permission::TemplateManage,
        )
        .await?;
        let row = rows::templates(conn, &[value.row.id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        if row.revision != value.row.revision {
            return Err(moving::stale());
        }
        templates::deletion_allowed(ctx, &row)?;
    }
    value.new.require(conn, principal).await
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
            subject(DuplicateKind::Template, value.row.id),
            value.home,
        )
        .await?
    } else {
        None
    };
    let deadline = before.deadline();
    let made = creation::insert(ctx, conn, principal, &value.new, move || before.check()).await?;
    deadline.check()?;
    let id = made.id();
    if !value.tags.is_empty() {
        grouping::set_tags(conn, ResourceKind::Template, id, &value.tags, ctx.now).await?;
        deadline.check()?;
    }
    if value.delete.is_some() {
        let boundary = boundary.ok_or_else(|| {
            ApiError::internal("a native cross-root move lost its source boundary")
        })?;
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
        templates::delete_native(ctx, conn, &value.row).await?;
        deadline.check()?;
    }
    Ok(effects::Outcome::Applied(id))
}

macro_rules! template_effect {
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

template_effect!(Templates, DuplicateBody, &sources::TEMPLATE, Copy);

template_effect!(Moving, MoveOutBody, &sources::TEMPLATE_MOVE, Move);

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
    const DECLARATION: &'static Declaration<Source> = &sources::TEMPLATE_MOVE_PREVIEW;
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
                .ok_or_else(|| ApiError::internal("the native move preview has no delete plan"))
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
