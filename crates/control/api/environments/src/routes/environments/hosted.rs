//! Environments made from a template on a host over HTTP: making one, starting and stopping it, keeping it awake, taking back a removal that has not begun, and the fleet of a place.
//!
//! Making an environment is one request that does two things in one transaction: the environment's
//! row, owned by the caller, and the job that makes its unit on a host, which the reply names. A host
//! that is named and cannot take it is not replaced by another; the request says why and which places
//! there are. Starting is the same job for an environment that is stopped (a turn does it by itself);
//! stopping asks the host to stop the unit and keeps it and its volumes; a unit's credentials that were
//! handed over are returned first, which is not built yet, so an environment that holds one stays up.

use super::Standing;

use super::clean_config;

use super::home_of;

use super::show;

use super::taken;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::actor_name;

use crate::routes::nodes::invalid;

use crate::routes::nodes::placed;

use crate::routes::resources::Home;

use crate::routes::resources::clean_name;

use crate::routes::resources::record;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::NodePermission;

use orch_access_contract::permissions::PermissionName;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::binding::NewEnvironment;

use orch_api_contract::binding::NewEnvironmentAccess;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::environment as api;

use orch_api_contract::environment::Environment;

use orch_api_contract::environment::FleetEntry;

use orch_api_contract::environment::HostLoad;

use orch_api_contract::environment::TemplateRef;

use orch_api_contract::workspace::Job;

use orch_authz::ownership;

use orch_authz::ownership::NewAccess;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::environments::hosting as rows;

use orch_db::queries::environments::routes as queries;

use orch_db::queries::resources::hosting as host_rows;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::environments::CreateError;

use orch_engine::environments::Request;

use orch_engine::environments::StartError;

use orch_engine::environments::Starting;

use orch_engine::environments::StopError;

use orch_engine::environments::StopReason;

use orch_engine::places;

use orch_types::Id;

use orch_types::Time;

use serde_json::json;

use std::collections::HashMap;

pub use orch_api_base::routes::environments::hosted::FleetSource;

pub use orch_api_base::routes::environments::hosted::FleetBounds;

pub use orch_api_base::routes::environments::hosted::PreparedFleet;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    crate::routes::duplicates::register::<duplicating::Environments, _>(
        registry,
        orch_api_contract::duplicates::EnvironmentDuplicate,
    );
    duplicating::mount_moves(registry);
    registry
        .on_with_operation(
            api::EnvironmentCreate,
            crate::key_operations::native::effects::prepare::<creation::Create>,
            creation::create,
        )
        .creation(api::EnvironmentCreate, creation::binding())
        .on(api::EnvironmentStart, start)
        .on(api::EnvironmentStop, stop)
        .on(api::EnvironmentRestart, restart)
        .on(api::EnvironmentKeepAwake, keep_awake)
        .on(api::EnvironmentKeep, keep);
    crate::routes::resources::environment_fleet::mount(registry);
}

/// The place an environment is made in when the request names none: the caller's domain, where they
/// belong to one tree of orgs only.
fn own_domain(principal: &Principal) -> ApiResult<Id> {
    match principal.memberships().roots().as_slice() {
        [root] => principal
            .memberships()
            .domain_in(*root)
            .ok_or_else(|| invalid("home_node_id", "name the place it is made in")),
        _ => Err(invalid(
            "home_node_id",
            "name the place it is made in: the caller belongs to more than one org, or to none",
        )),
    }
}

/// What a host that cannot take the environment answers: the plan is blocked, and the reason says
/// which of the two ways it was.
async fn blocked(conn: &mut Conn, why: &places::NoHost) -> ApiResult<ApiError> {
    let ids: Vec<Id> = why.blockers.iter().map(|blocker| blocker.host.id).collect();
    let addresses = crate::routes::resources::view::addresses(conn, &ids).await?;
    let mut blockers = why.blockers.clone();
    for blocker in &mut blockers {
        blocker.host.address = addresses.get(&blocker.host.id).cloned().unwrap_or_default();
    }
    Ok(ApiError::new(ErrorCode::PlanBlocked, why.message.clone())
        .with("reason", why.code)
        .with("host_blockers", blockers))
}

// ---------------------------------------------------------------- making one

/// Makes an environment from a template on a host, the caller's own, and the job that makes its unit.
/// The same guarded admission, ownership and audit for a standalone environment or an atomic thread start.
/// Only database work is done here; the caller commits the whole accepted request before kicking jobs.
pub(crate) async fn create_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: NewEnvironment,
) -> ApiResult<orch_engine::environments::Made> {
    create_with_prepared_initial(ctx, conn, principal, body, None).await
}
orch_api_base::__api_bind_3664da9cf1!();

/// Reuses a creator's single ordinary initial-file plan on its private creation
/// transaction; no body choice or source is resampled before the existing writer.
pub(crate) async fn create_with_prepared_initial(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: NewEnvironment,
    prepared: Option<orch_engine::initial_files::Plan>,
) -> ApiResult<orch_engine::environments::Made> {
    let input = prepare_input(ctx, conn, principal, body, prepared).await?;
    let made = match orch_engine::environments::create(
        conn,
        ctx.now,
        principal,
        &input.request(principal),
    )
    .await
    {
        Ok(made) => made,
        Err(error) => return Err(creation_failed(conn, error).await?),
    };
    finish_input(ctx, conn, principal, &input, &made).await?;
    Ok(made)
}
orch_api_base::__api_bind_25a1104266!();

struct CreationInput {
    body: NewEnvironment,
    config: serde_json::Value,
    name: Option<String>,
    home: Id,
    org: Id,
    initial: Option<orch_engine::initial_files::Plan>,
}

impl CreationInput {
    fn request<'a>(&'a self, principal: &Principal) -> Request<'a> {
        Request {
            owner: principal.user(),
            name: self.name.as_deref(),
            home: self.home,
            template: self.body.template_id,
            host: self.body.host_id,
            config: &self.config,
        }
    }
}

async fn prepare_input(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: NewEnvironment,
    prepared: Option<orch_engine::initial_files::Plan>,
) -> ApiResult<CreationInput> {
    let mut config = body.config.clone().unwrap_or_else(|| json!({}));
    if let Some(sources) = &body.data_sources {
        let own = config
            .as_object_mut()
            .ok_or_else(|| invalid("config", "the environment's own values are an object"))?;
        own.insert(
            orch_settings_contract::registry::keys::ENV_DATA_SOURCES.into(),
            serde_json::to_value(sources).map_err(ApiError::internal)?,
        );
    }
    clean_config("config", &config)?;
    orch_engine::environments::check_initial_program_consent(&config)
        .map_err(|reason| invalid("config.exec.version_overrides", reason))?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let home = match body.home_node_id {
        Some(home) => home,
        None => own_domain(principal)?,
    };
    let (home_place, _) = placed(conn, home).await?;
    authorization::require(
        conn,
        principal,
        home,
        PermissionName::OfNode(NodePermission::Interact),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let initial = match (body.files.as_ref(), prepared) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(ApiError::internal(
                "an initial file plan has no matching creation request",
            ));
        }
        (Some(_), Some(plan)) => Some(plan),
        (Some(files), None) => Some(
            orch_engine::initial_files::plan(conn, principal, files)
                .await
                .map_err(initial_failed)?,
        ),
    };
    if let Some(initial) = initial.as_ref().filter(|plan| plan.copies_files()) {
        crate::routes::duplicates::copy_to(
            conn,
            principal,
            orch_engine::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: initial.source_environment(),
            }),
            Some(home),
        )
        .await?;
    }
    let _ = ctx;
    Ok(CreationInput {
        body,
        config,
        name,
        home,
        org: home_place.org,
        initial,
    })
}

async fn creation_failed(conn: &mut Conn, error: CreateError) -> ApiResult<ApiError> {
    Ok(match error {
        CreateError::Taken(existing) => taken(existing),
        CreateError::NoTemplate => invalid(
            "template_id",
            "name a template: none is set for the place it is made in",
        ),
        CreateError::Template(why) => invalid("template_id", &why),
        CreateError::Runtime(why) => invalid("config", &why),
        CreateError::DataSources(why) => invalid("data_sources", &why),
        CreateError::NoPlace(why) => blocked(conn, &why).await?,
        CreateError::NoHome => ApiError::not_found(),
        CreateError::OperationScope => ApiError::new(
            ErrorCode::AuthzDenied,
            "the key does not permit this creation source",
        ),
        CreateError::Internal(error) => ApiError::engine(error),
    })
}

async fn finish_input(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    input: &CreationInput,
    made: &orch_engine::environments::Made,
) -> ApiResult<()> {
    if let Some(initial) = input.initial.as_ref() {
        orch_db::queries::jobs::merge_plan(
            conn,
            made.job.id,
            &json!({"initial_files":initial}),
            ctx.now,
        )
        .await?;
    }
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: made.id,
        },
        principal.user(),
        match input.body.access {
            None | Some(NewEnvironmentAccess::Defaults) => NewAccess::Defaults,
            Some(NewEnvironmentAccess::OnlyMe) => NewAccess::OnlyMe,
        },
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let who = actor_name(ctx)?;
    record(
        ctx,
        conn,
        AuditAction::EnvironmentCreated,
        made.id,
        Home {
            node: Some(input.home),
            org: Some(input.org),
        },
        &format!("{who} made an environment"),
        json!({ "environment": made.id, "job": made.job.id }),
    )
    .await?;
    Ok(())
}

/// Accepts the manager's same-unit restart and leaves its progress to the durable job.
async fn restart(ctx: Ctx, (): (), body: api::EnvironmentRestartBody) -> ApiResult<Job> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = ctx
        .app
        .engine
        .queue_restart(&mut tx, &principal, id, body.mode)
        .await
        .map_err(|error| match error {
            orch_engine::jobs::restart::Error::NotFound => ApiError::not_found(),
            orch_engine::jobs::restart::Error::NotManaged => {
                invalid("id", "restart needs an Orch-managed unit")
            }
            orch_engine::jobs::restart::Error::Busy(job) => ApiError::new(
                ErrorCode::EnvironmentUnavailable,
                "another job holds this environment",
            )
            .with("job_id", job.id),
            orch_engine::jobs::restart::Error::Denied(why) => ApiError::new(
                ErrorCode::AuthzDenied,
                "the environment's manager may restart it",
            )
            .with("missing", why.missing),
            orch_engine::jobs::restart::Error::Unavailable(reason) => {
                ApiError::new(ErrorCode::EnvironmentUnavailable, reason)
            }
            orch_engine::jobs::restart::Error::Internal(error) => ApiError::engine(error),
        })?;
    let shown = super::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}

// ---------------------------------------------------------------- start, stop, keep

/// Starts a stopped environment by hand, as a job; one that is up has no job to show, and says so.
async fn start(ctx: Ctx, (): (), (): ()) -> ApiResult<Job> {
    let id = ctx.id("id")?;
    let (ctx, tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let started = ctx
        .app
        .engine
        .start_environment_in(tx, &principal, id)
        .await
        .map_err(|error| match error {
            StartError::NotFound => ApiError::not_found(),
            StartError::NotManaged => ApiError::new(
                ErrorCode::PlanBlocked,
                "this is a directory on a machine, which is not started from here",
            ),
            StartError::BeingRemoved => {
                ApiError::new(ErrorCode::PlanBlocked, "its removal is scheduled")
            }
            StartError::Unavailable(status) => ApiError::new(
                ErrorCode::PlanBlocked,
                format!("the environment is {status}"),
            ),
            StartError::Internal(error) => ApiError::engine(error),
        })?;
    let mut conn = ctx.app.db.acquire().await?;
    match started {
        Starting::Job(job) => {
            let row = orch_db::queries::jobs::job(&mut conn, job.id)
                .await?
                .ok_or_else(|| ApiError::internal("a job that was queued is not there"))?;
            super::jobs::view(&mut conn, &row).await
        }
        Starting::Up => Err(ApiError::new(
            ErrorCode::ConflictExists,
            "the environment is up already",
        )),
    }
}

/// Stops an environment and keeps its unit and its volumes. No turn may run in it unless the request
/// says to interrupt them.
async fn stop(ctx: Ctx, (): (), body: api::EnvironmentStopBody) -> ApiResult<Environment> {
    let id = ctx.id("id")?;
    // The stop is decided in the transaction that asked the caller's credential and Manage of the
    // environment again once the body had come, before any turn is interrupted or a machine is told.
    let (ctx, tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let user = principal.user();
    ctx.app
        .engine
        .stop_environment_in(
            tx,
            &principal,
            &crate::sign_in::origin(&ctx),
            id,
            body.interrupt,
            StopReason::Person(user),
        )
        .await
        .map_err(|error| match error {
            StopError::NotFound => ApiError::not_found(),
            StopError::NotManaged => ApiError::new(
                ErrorCode::PlanBlocked,
                "this is a directory on a machine, which is not stopped from here",
            ),
            StopError::Busy(turns) => ApiError::new(
                ErrorCode::ResourceInUse,
                "turns run in the environment; stop it with interrupt to end them first",
            )
            .with("turns", turns),
            StopError::Unavailable(status) => ApiError::new(
                ErrorCode::PlanBlocked,
                format!("the environment is {status}"),
            ),
            StopError::CredentialReturnFailed(why) => {
                ApiError::new(ErrorCode::CredentialReturnFailed, why)
            }
            StopError::Host(why) => ApiError::new(ErrorCode::EnvironmentUnavailable, why),
            StopError::Internal(error) => ApiError::engine(error),
        })?;
    let mut conn = ctx.app.db.acquire().await?;
    let row = queries::environments(&mut conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut conn,
        AuditAction::EnvironmentStopped,
        id,
        home_of(&row),
        &format!("{who} stopped the environment {}", row.name),
        json!({ "environment": id, "interrupt": body.interrupt }),
    )
    .await?;
    show(&ctx, &mut conn, &principal, id).await
}

/// Counts as activity now, so that the idle timeout starts over: for whoever manages the environment,
/// as keeping it running is, and in the transaction that asked them for that Manage again.
async fn keep_awake(ctx: Ctx, (): (), (): ()) -> ApiResult<Environment> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    if !rows::keep_awake(&mut tx, id, ctx.now).await? {
        return Err(ApiError::not_found());
    }
    let shown = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Withdraws the removal of an environment that has been scheduled and has not begun.
async fn keep(ctx: Ctx, (): (), (): ()) -> ApiResult<Environment> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    rows::lock_environment(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let row = queries::environments(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let removal = orch_db::queries::environments::retained::removal(&mut tx, id).await?;
    if removal
        .as_ref()
        .and_then(|row| row.removal_phase.as_deref())
        .is_some_and(|phase| phase != "scheduled")
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "Removal has begun, so this environment cannot be kept. Export files before removal begins.",
        ));
    }
    let open = orch_db::queries::jobs::open_of_environment(&mut tx, id).await?;
    for job in open
        .into_iter()
        .filter(|job| job.kind == orch_db::queries::jobs::kind::ENVIRONMENT_REMOVE)
    {
        let current = orch_db::queries::jobs::lock(&mut tx, job.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if current.state != "queued" {
            return Err(ApiError::new(
                ErrorCode::PlanBlocked,
                "its removal has begun, and is not withdrawn",
            ));
        }
    }
    let lasting = orch_db::queries::environments::temporary::keep(&mut tx, id, ctx.now).await?;
    if rows::keep(&mut tx, id, ctx.now).await? || lasting {
        orch_db::queries::lifecycle::withdraw_removal(&mut tx, id, principal.user(), ctx.now)
            .await?;
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(ApiError::internal)?;
        let who = actor_name(&ctx)?;
        record(
            &ctx,
            &mut tx,
            AuditAction::EnvironmentUpdated,
            id,
            home_of(&row),
            &format!("{who} kept the environment {}", row.name),
            json!({ "environment": id, "removal": "withdrawn" }),
        )
        .await?;
    }
    let shown = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// The original native query predicates and calculations, with raw row associations retained.
pub(crate) async fn prepare_fleet(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    under: &[Id],
    query: &api::NodeEnvironmentListQuery,
) -> ApiResult<PreparedFleet> {
    let (prepared, standing) = super::prepare_viewed_among(conn, principal, under).await?;
    let views = super::show_inventory_prepared(ctx, conn, principal, &prepared).await?;
    let listed: Vec<Id> = views.iter().map(|view| view.base.id).collect();
    let threads: HashMap<Id, i64> = rows::thread_counts(conn, &listed)
        .await?
        .into_iter()
        .collect();
    let hosted: HashMap<Id, String> = rows::hosts_of(conn, &listed)
        .await?
        .into_iter()
        .map(|(environment, _, name)| (environment, name))
        .collect();
    let templates: HashMap<Id, TemplateRef> = views
        .iter()
        .filter_map(|view| {
            view.template.as_ref().map(|template| {
                (
                    view.base.id,
                    TemplateRef {
                        id: template.id,
                        name: template.name.clone(),
                    },
                )
            })
        })
        .collect();
    let mut entries = Vec::new();
    let mut sources = Vec::new();
    let mut providers = HashMap::new();
    for view in views {
        let id = view.base.id;
        let idle_minutes = view
            .last_activity_at
            .map(|at| (ctx.now - at.get()).num_minutes().max(0));
        if query
            .status
            .is_some_and(|status| status != view.base.status)
        {
            continue;
        }
        if let Some(least) = query.idle_minutes
            && idle_minutes.is_none_or(|idle| idle < i64::from(least))
        {
            continue;
        }
        if query.behind == Some(true) && view.updates.is_empty() {
            continue;
        }
        let manageable = standing.get(&id).is_some_and(Standing::manages);
        let native = prepared
            .rows
            .iter()
            .find(|row| row.id == id)
            .ok_or_else(ApiError::not_found)?;
        let references = if manageable {
            let held = rows::providers_holding(conn, id).await?;
            orch_db::queries::resources::routes::providers_among(conn, &held, &[]).await?
        } else {
            Vec::new()
        };
        sources.push(FleetSource {
            environment: id,
            home: native.home_node_id,
            template: native.template_id.filter(|template| {
                standing.get(&id).is_some_and(Standing::holds_something)
                    && prepared.template_ids().contains(template)
            }),
            host: prepared
                .hosted
                .iter()
                .find(|(environment, _, _)| *environment == id)
                .map(|(_, host, _)| *host),
            providers: references.iter().map(|row| row.id).collect(),
            manageable,
        });
        providers.insert(id, references);
        entries.push(FleetEntry {
            environment: view.base,
            home: Some(view.home),
            threads: threads.get(&id).copied().unwrap_or(0),
            host_name: hosted.get(&id).cloned(),
            template: templates.get(&id).cloned(),
            generation: view.generation,
            updates: i64::try_from(view.updates.len()).unwrap_or(i64::MAX),
            last_activity_at: view.last_activity_at,
            idle_minutes,
            idle_stop_at: view.idle_stop_at,
            active_turns: rows::active_turns(conn, id).await?,
            open_shells: rows::open_shells(conn, id).await?,
            processes: None,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: None,
            disk_used_bytes: None,
            manageable,
            holds: Vec::new(),
        });
    }
    let homes = sources.iter().map(|source| source.home).collect::<Vec<_>>();
    let disclosure = crate::routes::nodes::view::reference_disclosure(
        conn,
        principal,
        &homes,
        Policy::default(),
    )
    .await?;
    Ok(PreparedFleet {
        sources,
        entries,
        providers,
        bounds: FleetBounds::default(),
        disclosure,
    })
}
orch_api_base::__api_bind_84fb7b9d00!();

/// The hosts the caller manages, with how loaded each is.
pub(crate) async fn host_loads(conn: &mut Conn, managed: &[Id]) -> ApiResult<Vec<HostLoad>> {
    if managed.is_empty() {
        return Ok(Vec::new());
    }
    let hosts = host_rows::hosts(conn, Some(managed)).await?;
    let loads = host_rows::loads(conn, managed).await?;
    let mut found = Vec::new();
    for host in hosts {
        let held: Vec<_> = loads
            .iter()
            .filter(|load| load.host_id == host.id)
            .cloned()
            .collect();
        let declared = places::declared_with_overhead(&held, &host.drivers);
        let (kept_environments, kept_bytes, kept_sizes_unknown, _) =
            orch_db::queries::environments::retained::on_host(conn, host.id).await?;
        found.push(HostLoad {
            kept_environments,
            kept_bytes,
            kept_sizes_unknown,
            host_id: host.id,
            name: host.name.clone(),
            memory_declared_mb: declared.memory_mb,
            memory_capacity_mb: host
                .capacity
                .get("memory_mb")
                .and_then(orch_types::Json::as_i64)
                .unwrap_or(0),
            cpus: host
                .capacity
                .get("cpus")
                .and_then(orch_types::Json::as_i64)
                .unwrap_or(0),
            environments_running: i64::try_from(
                held.iter()
                    .filter(|load| load.kind == "environment" && load.state == "running")
                    .count(),
            )
            .unwrap_or(i64::MAX),
            schedulable: host.schedulable,
        });
    }
    Ok(found)
}
orch_api_base::__api_bind_a292f8a126!();

/// The existing initial-file refusal projection, shared by atomic creator prefixes.
pub(crate) fn initial_failed(error: orch_engine::initial_files::Error) -> ApiError {
    match error {
        orch_engine::initial_files::Error::NotFound => ApiError::not_found(),
        orch_engine::initial_files::Error::Denied(refusal) => ApiError::new(
            ErrorCode::AuthzDenied,
            "the initial file source is not authorized",
        )
        .with("missing", refusal.missing),
        orch_engine::initial_files::Error::Invalid(message) => invalid("files", &message),
        orch_engine::initial_files::Error::Unavailable(message) => {
            ApiError::new(ErrorCode::EnvironmentUnavailable, message)
        }
        orch_engine::initial_files::Error::Internal(error) => ApiError::engine(error),
    }
}
orch_api_base::__api_bind_0df7f1c396!();

#[path = "hosted/creation.rs"]
pub mod creation;

#[path = "hosted/duplicating.rs"]
pub mod duplicating;
