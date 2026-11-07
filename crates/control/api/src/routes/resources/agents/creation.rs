//! Native profile creation operands and completed individual-resource projection.

use super::{api, clean_harness_kind, clean_profile, queries};
use crate::key_operations::native::{self, effects};
use crate::middleware::authorization::{self, exact};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::access::operations::NameRoles;
use crate::routes::nodes::{actor_name, placed};
use crate::routes::resources::projection::single;
use crate::routes::resources::{Home, clean_name, record};
use futures::future::BoxFuture;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::agent_sources::{self as sources, Source};
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::resources::{OrgAgentCreateReceipt, OrgAgentCreateResponse, Resource};
use orch_authz::{
    Act, Caller, Policy, Principal,
    ownership::{self, NewAccess},
};
use orch_db::{Conn, DbError};
use orch_engine::operation_scope::targets::{self, TargetError};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;

const AGENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Agent);

mod machine;
pub(super) use machine::{MachineProfiles, create as create_machine};

pub(super) struct Profiles;
pub(super) struct Proposal {
    id: Id,
    home: Id,
    root: Id,
    name: String,
    harness: String,
    profile: Json,
    machine: Option<(Id, String)>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Created {
    id: Id,
    home: Id,
    machine: Option<Id>,
}
pub(super) struct Subject {
    created: Created,
    home: Option<Id>,
    machine: Option<Id>,
}

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

fn target_error(error: TargetError) -> ApiError {
    match error {
        TargetError::Missing => ApiError::not_found(),
        TargetError::Db(error) => error.into(),
        TargetError::Authz(error) => ApiError::internal(error),
    }
}

async fn require_home(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    home: Id,
) -> ApiResult<()> {
    let (_, kind) = placed(conn, home).await?;
    if kind != NodeKind::Org {
        return Err(ApiError::not_found());
    }
    authorization::require_projected(
        conn,
        principal,
        home,
        exact(Permission::SettingsManage),
        Act::Use,
        Policy::default(),
        ctx.key_receipt_required(),
    )
    .await
}

impl effects::Effect for Profiles {
    type Source = Source;
    type Query = ();
    type Body = api::OrgAgentCreateBody;
    type Value = Proposal;
    type Output = Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let home = ctx.id("id")?;
            let name = clean_name("name", &body.name)?;
            let harness = clean_harness_kind("harness_kind", &body.harness_kind)?;
            let profile = clean_profile("profile", &body.profile, false)?;
            let (place, kind) = placed(conn, home).await?;
            if kind != NodeKind::Org {
                return Err(ApiError::not_found());
            }
            let target = targets::node(conn, home).await.map_err(target_error)?;
            let known = native::bind_known(
                ctx,
                &sources::CREATE,
                &[native::Facts::one(Source::Home, AGENT, target)],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_home(ctx, conn, &principal, home).await?;
            // Existing node updates and lifecycle changes serialize on this actual creation home.
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                .bind(home)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let (current, current_kind) = placed(conn, home).await?;
            if current_kind != NodeKind::Org || current.org != place.org {
                return Err(ApiError::not_found());
            }
            let target = targets::node(conn, home).await.map_err(target_error)?;
            Ok((
                Proposal {
                    id: new_id(),
                    home,
                    root: current.org,
                    name,
                    harness,
                    profile,
                    machine: None,
                },
                vec![native::Facts::one(Source::Home, AGENT, target)],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_home(ctx, conn, principal, value.home))
    }
}

/// The original native insert/page/audit writer; the check runs at its first SQL effect.
async fn insert<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Proposal,
    before: F,
) -> ApiResult<Created>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    before()?;
    let made = queries::insert_agent(
        conn,
        &queries::NewAgent {
            id: value.id,
            name: &value.name,
            owner_id: principal.user(),
            home_id: value.home,
            harness_kind: &value.harness,
            machine_id: value.machine.as_ref().map(|(id, _)| *id),
            profile: &value.profile,
        },
        ctx.now,
    )
    .await?;
    if !made {
        return Err(ApiError::not_found());
    }
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ObjectKind::Agent,
            id: value.id,
        },
        principal.user(),
        NewAccess::Defaults,
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut details = json!({"agent":value.id,"name":value.name,"harness_kind":value.harness});
    let summary = match &value.machine {
        None => format!(
            "{} added the agent profile {}",
            actor_name(ctx)?,
            value.name
        ),
        Some((id, machine)) => {
            details["machine"] = json!(id);
            format!(
                "{} added the agent profile {} for the harness {} on the machine {machine}",
                actor_name(ctx)?,
                value.name,
                value.harness
            )
        }
    };
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::AgentCreated),
        value.id,
        Home {
            node: Some(value.home),
            org: Some(value.root),
        },
        &summary,
        details,
    )
    .await?;
    Ok(Created {
        id: value.id,
        home: value.home,
        machine: value.machine.as_ref().map(|(id, _)| *id),
    })
}

impl effects::Apply for Profiles {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            Ok(effects::Outcome::Applied(
                insert(ctx, conn, principal, value, move || before.check()).await?,
            ))
        })
    }
}

impl native::Stored for Profiles {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::CREATE, sources::MACHINE_CREATE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            // Only the actual completed native output supplies this generated identity.
            let created = *ctx.retained_source::<Created>()?;
            let row = queries::agents(conn, &[created.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let home = targets::node_identity(conn, created.home)
                .await
                .map_err(target_error)?;
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Agent,
                    id: created.id,
                },
            )
            .await
            .map_err(target_error)?;
            if row.machine_id != created.machine {
                return Err(ApiError::not_found());
            }
            let mut facts = vec![
                native::Facts::one(Source::Home, AGENT, home),
                native::Facts::one(Source::Created, AGENT, target),
            ];
            if let Some(machine) = created.machine {
                facts.push(machine::machine_fact(conn, machine).await?);
            }
            Ok((
                Subject {
                    created,
                    home: row.home_id,
                    machine: row.machine_id,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if value.created.machine.is_some() {
                super::registration_home(conn, principal, value.created.home, "home_id").await?;
                Ok(())
            } else {
                require_home(ctx, conn, principal, value.created.home).await
            }
        })
    }
}

impl native::resource_reply::StoredResource for Profiles {
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn object(value: &Subject) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Agent,
            id: value.created.id,
        }
    }
    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match super::require_read(conn, principal, value.created.id).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if native::modern(principal.caller())
                        || !matches!(error.problem.status, 403 | 404) =>
                {
                    return Err(error);
                }
                Err(_) => {}
            }
            // The original creation response is also a generated result of its settings
            // act for a legacy caller. Keep that native gate for this request's real output,
            // while refusing a moved profile or one the creator no longer owns.
            if value.created.machine.is_some() {
                super::registration_home(conn, principal, value.created.home, "home_id").await?;
            } else {
                require_home(ctx, conn, principal, value.created.home).await?;
            }
            let held = orch_authz::resources::effective(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Agent,
                    id: value.created.id,
                },
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            if value.home != Some(value.created.home) || !held.owner {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.created == fresh.created
                && opened.home == fresh.home
                && opened.machine == fresh.machine)
        })
    }
}

fn receipt(id: Id) -> OrgAgentCreateResponse {
    OrgAgentCreateResponse::Receipt(OrgAgentCreateReceipt {
        projection: ReceiptProjection::Receipt,
        agent_id: id,
    })
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _body: api::OrgAgentCreateBody,
) -> ApiResult<OrgAgentCreateResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Profiles>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    finish_created(completed).await
}

async fn finish_created(
    completed: effects::Completed<Created>,
) -> ApiResult<OrgAgentCreateResponse> {
    crate::key_operations::publication::receipt(&completed.ctx, &receipt(completed.output.id))?;
    completed.ctx.retain_source(completed.output)?;
    Ok(
        match native::resource_reply::stored::<Profiles>(&completed.ctx, &completed.accepted)
            .await?
        {
            Some(Resource::Agent(agent)) => OrgAgentCreateResponse::Full(Box::new(agent)),
            None => receipt(completed.output.id),
            Some(_) => {
                return Err(ApiError::internal(
                    "the native created profile response has another resource kind",
                ));
            }
        },
    )
}
