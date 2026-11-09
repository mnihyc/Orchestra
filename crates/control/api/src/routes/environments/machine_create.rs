//! The native directory registration factory, first effect and independent singleton response.

use super::*;
use crate::key_operations::native::effects::{Apply, Effect, FirstEffect, Outcome};
use crate::key_operations::native::resource_reply::ResourceProjection;
use crate::key_operations::{native, targets};
use crate::middleware::transaction_guard;
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::projection::single;
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{
    environment_sources::{MACHINE_CREATE, MachineCreateSource as Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_db::queries::resources::MachineRow;

pub(super) struct Create;
pub(super) struct Prepared {
    pub(super) body: api::MachineEnvironmentCreateBody,
    pub(super) machine: MachineRow,
    pub(super) name: String,
    pub(super) root: String,
    pub(super) home: Id,
    pub(super) namespace: Id,
    pub(super) org: Id,
    pub(super) created: Box<orch_authz::environments::Created>,
}
pub(super) struct Created {
    pub(super) environment: Id,
    revision: i64,
    owner: Id,
    org: Id,
    home: Id,
    machine: Id,
}
pub(super) enum ReplyInput {
    First(Created),
    Replay(Id),
}
impl ReplyInput {
    fn environment(&self) -> Id {
        match self {
            Self::First(created) => created.environment,
            Self::Replay(id) => *id,
        }
    }
}

fn sparse(caller: &Caller, error: ApiError) -> ApiError {
    if native::modern(caller) {
        authorization::problem_without_content(error)
    } else {
        error
    }
}
async fn facts(conn: &mut Conn, machine: Id, home: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        native::Facts::one(
            Source::Machine,
            KeyTargetKind::Object(ObjectKind::Machine),
            targets::machine_subject(conn, machine).await?,
        ),
        native::Facts::one(
            Source::Home,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::node(conn, home).await?,
        ),
    ])
}

/// Choose the actual default before binding creation authority; explicit legacy homes stay explicit.
async fn selected_home(
    conn: &mut Conn,
    machine: Id,
    body: &api::MachineEnvironmentCreateBody,
) -> ApiResult<(Id, Id)> {
    let namespace = match body.home_id {
        Some(home) => home,
        None => orch_db::queries::resources::machine_registration_home(conn, machine, None)
            .await?
            .map(|(home, _)| home)
            .ok_or_else(ApiError::not_found)?,
    };
    Ok((namespace, body.home_node_id.unwrap_or(namespace)))
}

/// Effect-free extraction of the original registration rules and actual native operands.
async fn resolve(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: api::MachineEnvironmentCreateBody,
) -> ApiResult<Prepared> {
    resolve_for_machine(conn, principal, ctx.id("id")?, body).await
}

/// The same native registration selector with its actual machine supplied explicitly.
/// The consuming operation owns its factual key-scope row and retains its own Ctx.
pub(super) async fn resolve_for_machine(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    body: api::MachineEnvironmentCreateBody,
) -> ApiResult<Prepared> {
    let name = clean_name("name", &body.name)?;
    let machine =
        crate::routes::resources::machines::registrable_machine_on(conn, principal, machine)
            .await?;
    if !machine.is_active() {
        return Err(ApiError::not_found());
    }
    let root = match body.data_sources.as_ref() {
        Some(sources) => {
            orch_engine::environments::check_machine_data_sources(conn, &machine, sources)
                .await
                .map_err(|why| invalid("data_sources", &why))?;
            sources.workspace.directory().unwrap_or("").to_owned()
        }
        None => clean_root("root", &body.root, machine.platform_os == "windows")?,
    };
    if body.data_sources.is_none()
        && let Some(policy) = reported_policy(conn, &machine).await?
        && !within_roots(&root, &policy.environment_roots)
    {
        return Err(invalid(
            "root",
            "the machine's local policy does not let an environment be there",
        ));
    }
    let (namespace, home) = selected_home(conn, machine.id, &body).await?;
    let (home_place, home_kind) = placed(conn, namespace).await?;
    let valid_kind =
        home_kind == NodeKind::Org || (body.home_id.is_none() && home_kind == NodeKind::Project);
    if !valid_kind {
        return Err(invalid("home_id", "an environment is registered in an org"));
    }
    let graph = Graph::load(conn, &[namespace])
        .await
        .map_err(ApiError::internal)?;
    let in_domain = graph.get(namespace).is_some_and(|node| {
        principal
            .memberships()
            .domain_in(node.root)
            .is_some_and(|domain| node.path.contains(&domain))
    });
    if !in_domain {
        return Err(invalid(
            "home_id",
            "an environment is registered in the caller's domain or an org inside it",
        ));
    }
    if home != namespace
        && orch_db::queries::resources::routes::homes_under(conn, namespace, &[home])
            .await?
            .is_empty()
    {
        return Err(invalid(
            "home_node_id",
            "this is no project or topic of that org",
        ));
    }
    let policy = policy_at(conn, home).await?;
    let policies = Policies::everywhere(ResourcePolicy {
        owners_name_outsiders: policy.owners_name_outsiders,
        shared_subscriptions: None,
    });
    let created = match check_creation(
        conn,
        principal,
        &NewEnvironment {
            home,
            place: Place::Machine {
                machine: machine.id,
            },
            mounts: Vec::new(),
        },
        &policies,
    )
    .await
    {
        Ok(Creating::Allowed(created)) => created,
        Ok(Creating::Refused(refusal)) => return Err(refusals::denied(conn, *refusal).await?),
        Ok(Creating::NotYourMachine | Creating::Exceeds(_)) => return Err(ApiError::not_found()),
        Err(error) => return Err(refusals::problem(conn, error).await),
    };
    Ok(Prepared {
        body,
        machine,
        name,
        root,
        home,
        namespace,
        org: home_place.org,
        created,
    })
}
pub(super) fn same_preparation(old: &Prepared, current: &Prepared) -> bool {
    old.machine.id == current.machine.id
        && old.machine.host_id == current.machine.host_id
        && old.name == current.name
        && old.root == current.root
        && old.home == current.home
        && old.namespace == current.namespace
        && old.org == current.org
        && old.created == current.created
}

impl Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = api::MachineEnvironmentCreateBody;
    type Value = Prepared;
    type Output = Created;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &MACHINE_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let machine = ctx.id("id")?;
            let initial = vec![native::Facts::one(
                Source::Machine,
                KeyTargetKind::Object(ObjectKind::Machine),
                targets::machine_subject(conn, machine).await?,
            )];
            let plan = native::bind_known(ctx, &MACHINE_CREATE, &initial)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            crate::routes::resources::machines::registrable_machine_on(conn, &principal, machine)
                .await
                .map_err(|error| sparse(caller, error))?;
            let (_, home) = selected_home(conn, machine, &body).await?;
            let known = facts(conn, machine, home).await?;
            let plan = native::bind_known(ctx, &MACHINE_CREATE, &known)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let mut value = resolve(ctx, conn, &principal, body.clone())
                .await
                .map_err(|error| sparse(caller, error))?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                // Pin the physical row and its real facade before reloading native policy.
                sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                    .bind(value.machine.id)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                if let Some(host) = value.machine.host_id {
                    sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                        .bind(host)
                        .execute(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                }
                let mut homes = vec![value.namespace, home];
                homes.sort_unstable();
                homes.dedup();
                sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
                    .bind(&homes)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                let current = facts(conn, ctx.id("id")?, home).await?;
                let plan = native::bind_known(ctx, &MACHINE_CREATE, &current)?;
                let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
                value = resolve(ctx, conn, &principal, body.clone())
                    .await
                    .map_err(|error| sparse(caller, error))?;
                if !orch_authz::environments::reserve_capacity(conn, &value.created)
                    .await
                    .map_err(ApiError::internal)?
                {
                    return Err(ApiError::not_found());
                }
                let plan = native::bind_known(
                    ctx,
                    &MACHINE_CREATE,
                    &facts(conn, ctx.id("id")?, home).await?,
                )?;
                let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
                let fresh = resolve(ctx, conn, &principal, body)
                    .await
                    .map_err(|error| sparse(caller, error))?;
                if !same_preparation(&value, &fresh) {
                    return Err(DbError::Retry.into());
                }
                value = fresh;
            }
            let facts = facts(conn, value.machine.id, value.home).await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let current = resolve(ctx, conn, principal, value.body.clone())
                .await
                .map_err(|error| sparse(principal.caller(), error))?;
            if same_preparation(value, &current) {
                Ok(())
            } else {
                Err(DbError::Retry.into())
            }
        })
    }
}
impl Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Created>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let created =
                insert_native(ctx, conn, principal, value, move || before.check()).await?;
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(principal.user()),
                CREATION,
                created.environment,
                json!({"revision":created.revision}),
            )
            .await?;
            deadline.check()?;
            Ok(Outcome::Applied(created))
        })
    }
}

/// The original native database writer and first-effect seam, without choosing a transport operation.
pub(super) async fn insert_native(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
    before: impl FnOnce() -> ApiResult<()> + Send,
) -> ApiResult<Created> {
    if let Some(existing) = queries::named(conn, principal.user(), value.org, &value.name).await? {
        return Err(sparse(principal.caller(), taken(Some(existing))));
    }
    let id = orch_types::new_id();
    before()?;
    match queries::insert_connected(
        conn,
        &NewConnected {
            id,
            name: &value.name,
            owner_id: principal.user(),
            machine_id: value.machine.id,
            home_node_id: value.home,
            workspace_root: &value.root,
            remote_write: value.body.remote_write,
            terminal: value.body.terminal,
        },
        principal.now().get(),
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return Err(ApiError::not_found()),
        Err(DbError::Unique { .. }) => return Err(sparse(principal.caller(), taken(None))),
        Err(error) => return Err(error.into()),
    }
    if value.body.home_id.is_some() || value.body.home_node_id.is_some() {
        sqlx::query("UPDATE environments SET home_overridden_at=$2 WHERE id=$1")
            .bind(id)
            .bind(principal.now().get())
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    hosting::set_entries(
        conn,
        id,
        value.created.host.as_ref().and_then(|(entry, _)| *entry),
        None,
    )
    .await?;
    if let Some(sources) = &value.body.data_sources {
        sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
            .bind(id)
            .bind(json!({"env.data_sources":sources}))
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
        principal.user(),
        match value.body.access {
            None | Some(NewEnvironmentAccess::Defaults) => NewAccess::Defaults,
            Some(NewEnvironmentAccess::OnlyMe) => NewAccess::OnlyMe,
        },
        principal.now(),
    )
    .await
    .map_err(ApiError::internal)?;
    let who = actor_name(ctx)?;
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::EnvironmentCreated),
        id,
        Home {
            node: Some(value.home),
            org: Some(value.org),
        },
        &format!(
            "{who} registered {} of the machine {} as the environment {}",
            value.root, value.machine.name, value.name
        ),
        json!({"environment":id,"name":value.name,"machine":value.machine.id,"root":value.root}),
    )
    .await?;
    let row = queries::environments(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    Ok(Created {
        environment: id,
        revision: row.revision,
        owner: row.owner_id,
        org: row.org_id,
        home: row.home_node_id,
        machine: value.machine.id,
    })
}

const CREATION: &str = "machine_environment";
pub(super) fn binding() -> crate::middleware::idempotency::Creation<
    (),
    api::MachineEnvironmentCreateBody,
    api::MachineEnvironmentCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: CREATION,
        identity: |value| {
            let id = match value {
                api::MachineEnvironmentCreateResponse::Full(value) => value.base.id,
                api::MachineEnvironmentCreateResponse::Receipt(value) => value.environment_id,
            };
            // The atomic observer retains the richer accepted job/revision tuple.
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION.into(),
                id,
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                if created.kind != CREATION {
                    return Err(ApiError::internal("the accepted environment kind changed"));
                }
                let receipt = api::MachineEnvironmentCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    environment_id: created.id,

                    revision: serde_json::from_value(created.data["revision"].clone())
                        .map_err(ApiError::internal)?,
                };
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                let account = ctx.authenticated_user()?.id();
                let full = crate::key_operations::completed_projection::run(
                    &ctx,
                    native::resource_reply::with_input_completed_replay::<Create>(
                        &ctx,
                        account,
                        ReplyInput::Replay(created.id),
                    ),
                )
                .await;
                match full {
                    Ok(Some(orch_api_contract::resources::Resource::Environment(value))) => {
                        Ok(api::MachineEnvironmentCreateResponse::Full(Box::new(value)))
                    }
                    Ok(None) => Ok(api::MachineEnvironmentCreateResponse::Receipt(receipt)),
                    Ok(Some(_)) => Err(ApiError::internal(
                        "the native creator returned another resource kind",
                    )),
                    Err(error)
                        if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) =>
                    {
                        Ok(api::MachineEnvironmentCreateResponse::Receipt(receipt))
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _: api::MachineEnvironmentCreateBody,
) -> ApiResult<api::MachineEnvironmentCreateResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = native::effects::apply::<Create>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::MachineEnvironmentCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done.output.environment,
        revision: done.output.revision,
    };
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    match native::resource_reply::with_input::<Create>(
        &done.ctx,
        &done.accepted,
        ReplyInput::First(done.output),
    )
    .await?
    {
        Some(orch_api_contract::resources::Resource::Environment(value)) => {
            Ok(api::MachineEnvironmentCreateResponse::Full(Box::new(value)))
        }
        Some(_) => Err(ApiError::internal(
            "the native directory registration returned another resource kind",
        )),
        None => Ok(api::MachineEnvironmentCreateResponse::Receipt(receipt)),
    }
}

impl ResourceProjection for Create {
    type Source = Source;
    type Value = EnvironmentFull;
    type Input = ReplyInput;
    const READ_SOURCE: Source = Source::Environment;
    const ROLES: &'static single::Roles = &single::Roles {
        sources: "machine_created_sources",
        names: NameRoles {
            groups: Some("machine_created_groups"),
            nodes: [
                (ObjectKind::Org, "machine_created_orgs"),
                (ObjectKind::Project, "machine_created_projects"),
                (ObjectKind::Topic, "machine_created_topics"),
                (ObjectKind::Thread, "machine_created_threads"),
            ],
            containers: None,
        },
    };
    fn declaration(_: &Ctx, _: &ReplyInput) -> ApiResult<&'static Declaration<Source>> {
        Ok(&MACHINE_CREATE)
    }
    fn load<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a ReplyInput,
    ) -> BoxFuture<'a, ApiResult<(EnvironmentFull, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = queries::environments(conn, &[input.environment()])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let mut current = if let ReplyInput::First(input) = input {
                if row.owner_id != input.owner
                    || row.org_id != input.org
                    || row.home_node_id != input.home
                    || row.machine_id != Some(input.machine)
                    || row.placement != "connected"
                {
                    return Err(ApiError::not_found());
                }
                facts(conn, input.machine, input.home).await?
            } else {
                Vec::new()
            };
            current.push(native::Facts::one(
                Source::Environment,
                KeyTargetKind::Object(ObjectKind::Environment),
                targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: row.id,
                    },
                )
                .await?,
            ));
            Ok((row, current))
        })
    }
    fn object(row: &EnvironmentFull) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Environment,
            id: row.id,
        }
    }
    fn require_read<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a EnvironmentFull,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { reading::decide(conn, principal, row.id).await.map(|_| ()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        opened: &'a EnvironmentFull,
        current: &'a EnvironmentFull,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.id == current.id
                && opened.owner_id == current.owner_id
                && opened.org_id == current.org_id
                && opened.home_node_id == current.home_node_id
                && opened.machine_id == current.machine_id
                && opened.placement == current.placement
                && opened.template_id == current.template_id
                && opened.workspace_root == current.workspace_root)
        })
    }
}
