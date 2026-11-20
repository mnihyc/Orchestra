//! Agent profile copies retain native configuration validation and physical registration checks.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::targets;

use crate::routes::admin::catalogue;

use crate::routes::resources;

use crate::routes::resources::agents::creation;

use crate::routes::resources::creation_home;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::duplicate_sources as sources;

use orch_api_contract::duplicate_sources::Source;

use orch_api_contract::duplicates::AgentMoveOut;

use orch_api_contract::duplicates::AgentMoveOutPreview;

use orch_api_contract::duplicates::CopySubject;

use orch_api_contract::duplicates::CopySubjectKind;

use orch_api_contract::duplicates::MoveOutBody;

use orch_api_contract::duplicates::MoveOutDeleteEffect;

use orch_api_contract::duplicates::MoveOutDeletePlan;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::OrgAgentCreateBody;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::resources::grouping;

use orch_db::queries::resources::routes as rows;
pub(super) struct Agents;

struct Moving;

struct Preview;

impl Configuration for Moving {
    const KIND: DuplicateKind = DuplicateKind::Agent;
    const REPLAY: &'static str = "agent_move_out";
    const MOVING: bool = true;
    fn committed(ctx: &Ctx) {
        if let Ok(Some(deleted)) = ctx.optional_retained_source::<resources::agents::DeletedAgent>()
        {
            for thread in &deleted.wakes {
                ctx.app.engine.wake(*thread);
            }
        }
        ctx.app.engine.kick();
    }
}

pub(super) fn mount_moves(registry: &mut crate::registry::Registry) {
    register::<Moving, _>(registry, AgentMoveOut);
    registry.on_snapshot_body_read_with_optional_operation(
        AgentMoveOutPreview,
        preview_prepare,
        preview,
    );
}

impl Configuration for Agents {
    const KIND: DuplicateKind = DuplicateKind::Agent;
    const REPLAY: &'static str = "agent_duplicate";
}

enum New {
    Placed(creation::Proposal),
    System(catalogue::GlobalProfile),
}

pub(super) struct Prepared {
    source: Id,
    home: Option<Id>,
    machine: Option<Id>,
    harness: String,
    new: New,
    tags: Vec<Id>,
    row: rows::AgentRow,
    delete: Option<MoveOutDeletePlan>,
}

async fn require_home(
    conn: &mut Conn,
    principal: &Principal,
    home: Option<Id>,
) -> ApiResult<resources::Home> {
    creation_home::require(
        conn,
        principal,
        ResourceKind::Agent,
        home,
        "an agent profile is at home in an org",
        "only a system admin makes a system profile",
    )
    .await
}

async fn machine(conn: &mut Conn, id: Option<Id>) -> ApiResult<native::Facts<Source>> {
    let groups = match id {
        None => Vec::new(),
        Some(id) => vec![(
            KeyTargetKind::Object(ObjectKind::Machine),
            vec![
                targets::machine_subjects(conn, &[id])
                    .await?
                    .remove(&id)
                    .ok_or_else(ApiError::not_found)?,
            ],
        )],
    };
    Ok(native::Facts::collection(Source::Machine, groups))
}

enum Intent {
    Copy(DuplicateBody),
    Preview(DuplicateBody),
    Move(MoveOutBody),
}

impl Intent {
    fn declaration(&self) -> &'static Declaration<Source> {
        match self {
            Self::Copy(_) => &sources::AGENT,
            Self::Preview(_) => &sources::AGENT_MOVE_PREVIEW,
            Self::Move(_) => &sources::AGENT_MOVE,
        }
    }
}

async fn require_owner(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::AgentRow,
    home: Option<Id>,
) -> ApiResult<()> {
    let org = copy_to(conn, principal, subject(DuplicateKind::Agent, row.id), home).await?;
    if org == row.org_id {
        return Err(ApiError::invalid(
            "destination",
            "use the existing Move within this organization",
        ));
    }
    if !owns(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Agent,
            id: row.id,
        },
    )
    .await?
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "only an actual owner can move this profile into another organization",
        ));
    }
    resources::require(
        conn,
        principal,
        ResourceKind::Agent,
        row.id,
        Permission::AgentManage,
    )
    .await?;
    Ok(())
}

async fn move_plan(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::AgentRow,
    body: &DuplicateBody,
    home: Option<Id>,
) -> ApiResult<MoveOutDeletePlan> {
    let target = orch_api_contract::resource_facts::FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Agent,
            id: row.id,
        },
        model_id: None,
    };
    let mut work = orch_engine::resource_facts::activity::read(conn, &target)
        .await
        .map_err(ApiError::engine)?;
    work.sort_by(|a, b| (&a.kind, a.id, a.thread).cmp(&(&b.kind, b.id, b.thread)));
    let memberships: Vec<_> = work
        .iter()
        .map(|row| {
            serde_json::json!({"kind":row.kind,"id":row.id,
        "count":row.count,"thread":row.thread,"segment":row.segment,"since":row.since})
        })
        .collect();
    let requests = orch_engine::resource_facts::activity::requests(conn, &target)
        .await
        .map_err(ApiError::engine)?;
    let mut requests = requests;
    requests.sort_unstable();
    moving::plan(
        CopySubject {
            kind: CopySubjectKind::Agent,
            id: row.id,
        },
        home.map_or(DuplicateDestination::System, |id| {
            DuplicateDestination::Node { id }
        }),
        row.revision,
        principal.user(),
        body,
        serde_json::json!({"home":row.home_id,"org":row.org_id,"machine":row.machine_id,
            "harness":row.harness_kind,"profile":row.profile,"work":memberships,"requests":requests}),
        vec![
            MoveOutDeleteEffect::StopBoundWork,
            MoveOutDeleteEffect::RemoveSource,
        ],
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
    let locked: Option<Id> = sqlx::query_scalar(if preview {
        "SELECT id FROM agents WHERE id=$1 AND deleted_at IS NULL"
    } else {
        "SELECT id FROM agents WHERE id=$1 AND deleted_at IS NULL FOR UPDATE"
    })
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if locked.is_none() {
        return Err(ApiError::not_found());
    }
    let row = rows::agents(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let home = destination(&body, row.home_id);
    let mut facts = origins(conn, ObjectKind::Agent, id, home).await?;
    facts.push(machine(conn, row.machine_id).await?);
    if preview {
        facts.push(native::Facts::one(
            Source::PreviewContent,
            KeyTargetKind::Object(ObjectKind::Agent),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Agent,
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
        ResourceKind::Agent,
        id,
        Permission::AgentView,
    )
    .await?;
    revision(&body, row.revision)?;
    require_home(conn, &principal, home).await?;
    copy_to(conn, &principal, subject(DuplicateKind::Agent, id), home).await?;
    let delete = if moving {
        require_owner(conn, &principal, &row, home).await?;
        let plan = move_plan(conn, &principal, &row, &body, home).await?;
        if let Some(confirmation) = &confirmation {
            moving::confirm(&plan, confirmation)?;
        }
        Some(plan)
    } else {
        None
    };
    if !preview {
        creation_home::lock(conn, home).await?;
    }
    let place = require_home(conn, &principal, home).await?;
    let request = OrgAgentCreateBody {
        name: display_name(&body, &row.name),
        harness_kind: row.harness_kind.clone(),
        profile: row.profile.clone(),
    };
    let new = match home {
        Some(home) => {
            let physical = match row.machine_id {
                Some(id) => Some((
                    id,
                    creation::duplicate_machine(conn, &principal, id, home, &row.harness_kind)
                        .await?,
                )),
                None => None,
            };
            New::Placed(creation::proposal(
                home,
                place.org.ok_or_else(ApiError::not_found)?,
                request,
                physical,
            )?)
        }
        None => {
            if row.machine_id.is_some() {
                return Err(crate::routes::nodes::invalid(
                    "destination",
                    "a physical harness profile needs its native organization home",
                ));
            }
            New::System(catalogue::prepare_global_profile(request)?)
        }
    };
    facts[2] = machine(conn, row.machine_id).await?;
    let tags = copy_tags(conn, ResourceKind::Agent, id, home).await?;
    current(ctx, principal.caller())?;
    Ok((
        Prepared {
            source: id,
            home,
            machine: row.machine_id,
            harness: row.harness_kind.clone(),
            new,
            tags,
            row,
            delete,
        },
        facts,
    ))
}

async fn require_native(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
) -> ApiResult<()> {
    resources::require(
        conn,
        principal,
        ResourceKind::Agent,
        value.source,
        Permission::AgentView,
    )
    .await?;
    copy_to(
        conn,
        principal,
        subject(DuplicateKind::Agent, value.source),
        value.home,
    )
    .await?;
    require_home(conn, principal, value.home).await?;
    if let Some(machine) = value.machine {
        creation::duplicate_machine(
            conn,
            principal,
            machine,
            value.home.ok_or_else(ApiError::not_found)?,
            &value.harness,
        )
        .await?;
    }
    if value.delete.is_some() {
        require_owner(conn, principal, &value.row, value.home).await?;
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
            subject(DuplicateKind::Agent, value.source),
            value.home,
        )
        .await?
    } else {
        None
    };
    let deadline = before.deadline();
    let id = match &value.new {
        New::Placed(proposal) => {
            creation::insert(ctx, conn, principal, proposal, move || before.check())
                .await?
                .id()
        }
        New::System(proposal) => {
            let id = catalogue::insert_global_profile(ctx, conn, proposal, move || before.check())
                .await?;
            deadline.check()?;
            orch_authz::ownership::start_resource(
                conn,
                ObjectRef {
                    kind: ObjectKind::Agent,
                    id,
                },
                principal.user(),
                orch_authz::ownership::NewAccess::Defaults,
                orch_types::Time::new(ctx.now),
            )
            .await
            .map_err(ApiError::internal)?;
            id
        }
    };
    deadline.check()?;
    if !value.tags.is_empty() {
        grouping::set_tags(conn, ResourceKind::Agent, id, &value.tags, ctx.now).await?;
        deadline.check()?;
    }
    if value.delete.is_some() {
        let boundary = boundary.ok_or_else(|| {
            ApiError::internal("the native moving profile source boundary disappeared")
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
        let deleted = resources::agents::delete_native(ctx, conn, principal, &value.row).await?;
        ctx.retain_source(deleted)?;
        deadline.check()?;
    }
    Ok(effects::Outcome::Applied(id))
}

macro_rules! agent_effect {
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

agent_effect!(Agents, DuplicateBody, &sources::AGENT, Copy);

agent_effect!(Moving, MoveOutBody, &sources::AGENT_MOVE, Move);

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
    const DECLARATION: &'static Declaration<Source> = &sources::AGENT_MOVE_PREVIEW;
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
                .ok_or_else(|| ApiError::internal("the native profile move has no delete plan"))
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
            Ok(opened.source == fresh.source
                && opened.home == fresh.home
                && opened.delete == fresh.delete)
        })
    }
}
