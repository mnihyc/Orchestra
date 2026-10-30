//! A managed provider's runtime over HTTP: started and stopped by hand, and what the provider's page shows of it (its unit, its host, its template, how many units it has had, when the idle stop or the renewal of its sign-in comes, and the job at work on it).
//!
//! Every provider made here is managed: the server runs a unit of its own for it, made by a job when
//! the provider is, kept as long as the provider exists, stopped when it has been idle and started again
//! by the next turn or sign-in. These requests start and stop it by hand. A start answers with the
//! provider as it stands, and a runtime that is being made shows the job on the page; a stop is refused
//! while a turn runs on the provider unless the request says to interrupt it.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::resources::providers::show;
use crate::routes::resources::view::addresses;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{JobRef, JobState};
use orch_api_contract::resources::{
    self as api, HostRef, Provider, ProviderRuntime, ProviderRuntimeTemplate,
};
use orch_api_contract::users::UserRef;
use orch_authz::Policy;
use orch_db::Conn;
use orch_db::queries::environments::hosting::runtime_state;
use orch_db::queries::jobs::{JobRow, provider_jobs};
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::resources::provider_runtime::{self as rows};
use orch_db::queries::resources::routes::ProviderRow;
use orch_engine::credentials::handover::ReturnError;
use orch_engine::provider_runtime::RuntimeError;
use orch_settings::{ProviderAt, get_at_providers};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use std::collections::HashMap;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::ProviderStart, start)
        .on(api::ProviderStop, stop)
        .on(api::ProviderHandoverReturn, handover_return);
}

/// A job in the few words a provider's page carries it in.
fn job_ref(job: &JobRow) -> JobRef {
    let (progress, _) = orch_engine::jobs::runner::read_progress(job);
    JobRef {
        id: job.id,
        kind: job.kind.clone(),
        state: match job.state.as_str() {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "succeeded" => JobState::Succeeded,
            "cancelled" => JobState::Cancelled,
            "lost" => JobState::Lost,
            _ => JobState::Failed,
        },
        phase: progress.phase,
        percent: progress.percent,
    }
}

/// The word of the state a runtime that has no row yet is shown in.
const BEING_MADE: &str = "creating";

/// What the settings say of a provider's runtime: the template it is made from when it has no unit yet, how
/// many minutes idle stop it, and after how many days stopped its sign-in is renewed.
#[derive(Debug, Clone, Copy)]
struct RuntimeStanding {
    template_id: Id,
    idle_minutes: i64,
    renew_days: i64,
}

/// A template and whose it is, as a page shows them.
type TemplateShown = (Option<host_rows::TemplateState>, Option<UserRef>);

/// The runtimes of these providers as their pages show them: the unit with its host and template, how
/// far it has fallen behind, when it is stopped for being idle or started to renew a sign-in, and the
/// job at work on it, or the last that ended when none is. What is read for each provider is read once
/// for all of them, or once for each home and set of values they share, so that a list of many costs
/// what a list of a few does.
pub(crate) async fn runtime_views(
    ctx: &Ctx,
    conn: &mut Conn,
    managed: &[&ProviderRow],
) -> ApiResult<HashMap<Id, ProviderRuntime>> {
    if managed.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Id> = managed.iter().map(|row| row.id).collect();
    let live = rows::live_runtimes(conn, &ids).await?;
    let renewed: HashMap<Id, Option<chrono::DateTime<chrono::Utc>>> =
        rows::renewal_stamps(conn, &ids)
            .await?
            .into_iter()
            .collect();
    let networks = crate::routes::environments::network::of_runtimes(
        conn,
        &live.iter().map(|runtime| runtime.id).collect::<Vec<_>>(),
    )
    .await?;
    let mut pending = ctx
        .app
        .engine
        .pending_runtime_updates(conn, &live.iter().map(|r| r.id).collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let versions: HashMap<Id, String> =
        orch_db::queries::environments::changes::connector_versions(
            conn,
            &live.iter().filter_map(|r| r.machine_id).collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .collect();
    let open = provider_jobs::open_for(conn, &ids).await?;
    let host_ids: Vec<Id> = live.iter().map(|runtime| runtime.host_id).collect();
    let hosts = host_rows::hosts(conn, Some(&host_ids)).await?;
    let host_addresses = addresses(conn, &host_ids).await?;
    let activity: HashMap<Id, Option<chrono::DateTime<chrono::Utc>>> =
        rows::activities_at(conn, &ids).await?.into_iter().collect();
    let settings_at: Vec<ProviderAt<'_>> = managed
        .iter()
        .map(|row| ProviderAt {
            id: row.id,
            home_id: row.home_id,
            runtime_config: &row.runtime_config,
            revision: row.revision,
        })
        .collect();
    let chosen_templates: HashMap<Id, Option<Id>> =
        get_at_providers(conn, &settings_at, keys::RUNTIME_TEMPLATE_ID)
            .await
            .map_err(ApiError::internal)?;
    let idle_minutes: HashMap<Id, Option<i64>> =
        get_at_providers(conn, &settings_at, keys::RUNTIME_IDLE_STOP_MINUTES)
            .await
            .map_err(ApiError::internal)?;
    let renew_days: HashMap<Id, Option<i64>> =
        get_at_providers(conn, &settings_at, keys::RUNTIME_RENEW_DAYS)
            .await
            .map_err(ApiError::internal)?;
    let template_ids: Vec<Id> = managed
        .iter()
        .map(|row| {
            live.iter()
                .find(|runtime| runtime.provider_id == Some(row.id))
                .map(|runtime| runtime.template_id)
                .unwrap_or_else(|| {
                    chosen_templates
                        .get(&row.id)
                        .copied()
                        .flatten()
                        .unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID)
                })
        })
        .collect();
    let template_rows = host_rows::templates(conn, &template_ids).await?;
    let template_owners = crate::middleware::authorization::refusals::user_refs(
        conn,
        &template_rows
            .iter()
            .filter_map(|row| row.owner_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let templates: HashMap<Id, TemplateShown> = template_rows
        .into_iter()
        .map(|row| {
            let owner = row
                .owner_id
                .and_then(|owner| template_owners.get(&owner).cloned());
            (row.id, (Some(row), owner))
        })
        .collect();
    let mut views = HashMap::new();
    for row in managed {
        let runtime = live
            .iter()
            .find(|runtime| runtime.provider_id == Some(row.id));
        let job = open.iter().find(|job| job.provider_id == Some(row.id));
        if runtime.is_none() && job.is_none() {
            continue;
        }
        let own = RuntimeStanding {
            template_id: chosen_templates
                .get(&row.id)
                .copied()
                .flatten()
                .unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID),
            idle_minutes: idle_minutes.get(&row.id).copied().flatten().unwrap_or(0),
            renew_days: renew_days.get(&row.id).copied().flatten().unwrap_or(0),
        };
        let template_id: Id = runtime.map_or(own.template_id, |runtime| runtime.template_id);
        let (template, owner) = templates.get(&template_id).cloned().unwrap_or_default();
        let name = template
            .as_ref()
            .map(|template| template.name.clone())
            .unwrap_or_default();
        let (idle_minutes, renew_days) = (own.idle_minutes, own.renew_days);
        let state = runtime.map_or_else(|| BEING_MADE.to_owned(), |runtime| runtime.state.clone());
        let last_active = runtime.and_then(|runtime| {
            let seen = activity.get(&row.id).copied().flatten();
            [runtime.last_active_at, seen, Some(runtime.created_at)]
                .into_iter()
                .flatten()
                .max()
        });
        let idle_stop_at = (state == runtime_state::RUNNING && idle_minutes > 0)
            .then(|| last_active.map(|at| at + chrono::Duration::minutes(idle_minutes)))
            .flatten();
        let renew_at = (matches!(
            state.as_str(),
            runtime_state::RUNNING | runtime_state::STOPPED
        ) && renew_days > 0
            && row.kind == "subscription"
            && row.held_by_environment_id.is_none()
            && row
                .facts
                .pointer("/credential/yields")
                .and_then(orch_types::Json::as_str)
                .unwrap_or("login")
                == "login")
            .then(|| {
                let days = chrono::Duration::try_days(renew_days).unwrap_or(chrono::Duration::MAX);
                match row
                    .facts
                    .pointer("/credential/expires_at")
                    .and_then(orch_types::Json::as_str)
                    .and_then(|time| time.parse::<Time>().ok())
                {
                    Some(expires) => expires.get().checked_sub_signed(days),
                    None => renewed
                        .get(&row.id)
                        .copied()
                        .flatten()
                        .unwrap_or(row.created_at)
                        .checked_add_signed(days),
                }
            })
            .flatten();
        let host = runtime
            .and_then(|runtime| hosts.iter().find(|host| host.id == runtime.host_id))
            .map(|host| HostRef {
                id: host.id,
                name: host.name.clone(),
                address: host_addresses.get(&host.id).cloned().unwrap_or_default(),
            });
        let updates = runtime
            .and_then(|runtime| pending.remove(&runtime.id))
            .unwrap_or_default();
        views.insert(
            row.id,
            ProviderRuntime {
                network: runtime
                    .and_then(|runtime| networks.get(&runtime.id))
                    .cloned()
                    .unwrap_or_else(|| {
                        orch_api_contract::environment::NetworkState::open(
                            orch_api_contract::environment::UnitNetworkMode::Bridge,
                        )
                    }),
                state,
                host,
                driver: runtime.map_or_else(
                    || {
                        template
                            .as_ref()
                            .map(|each| each.driver.clone())
                            .unwrap_or_default()
                    },
                    |runtime| runtime.driver.clone(),
                ),
                template: ProviderRuntimeTemplate {
                    id: template_id,
                    name,
                    owner,
                },
                generation: runtime.map_or(0, |runtime| i64::from(runtime.generation)),
                update_required: orch_engine::updates::update_required(
                    runtime
                        .and_then(|r| r.machine_id)
                        .and_then(|id| versions.get(&id))
                        .map(String::as_str),
                    orch_engine::updates::CONNECTOR_MINIMUM,
                ),
                updates,
                last_active_at: runtime
                    .and_then(|runtime| runtime.last_active_at)
                    .map(Time::new),
                idle_stop_at: idle_stop_at.map(Time::new),
                renew_at: renew_at.map(Time::new),
                job: job.map(job_ref),
            },
        );
    }
    Ok(views)
}

/// What a refusal to start or stop a runtime is to the caller.
pub(super) fn refused(error: RuntimeError) -> ApiError {
    match error {
        RuntimeError::NotFound => ApiError::not_found(),
        RuntimeError::NotManaged => ApiError::new(
            ErrorCode::RequestInvalid,
            "this provider is on a person's machine, which is started and stopped there",
        ),
        RuntimeError::Unavailable(why) => ApiError::new(
            ErrorCode::ProviderUnavailable,
            format!("its runtime cannot be started or stopped now: {why}"),
        ),
        RuntimeError::Busy(running) => ApiError::new(
            ErrorCode::ThreadNotIdle,
            "turns run on this provider; stop them first, or say to interrupt them",
        )
        .with("running", running),
        RuntimeError::Host(why) => ApiError::new(
            ErrorCode::ProviderUnavailable,
            format!("the host could not stop its runtime: {why}"),
        ),
        RuntimeError::Internal(error) => ApiError::engine(error),
    }
}

/// Starts a managed provider's runtime by hand: a unit that is stopped is started, one that failed or
/// was never made is made, one that is up is left. The answer shows the job when there is one.
async fn start(ctx: Ctx, (): (), _body: api::ProviderStartStopBody) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (ctx, tx, principal) = super::begin_mutation(&ctx).await?;
    ctx.app
        .engine
        .start_provider_runtime_in(tx, &principal, &crate::sign_in::origin(&ctx), id)
        .await
        .map_err(refused)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    show(&ctx, &mut conn, &principal, id).await
}

/// Stops a managed provider's runtime by hand and keeps it with its `state` volume: refused while a
/// turn runs on the provider, unless the request says to interrupt it.
async fn stop(ctx: Ctx, (): (), body: api::ProviderStartStopBody) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (ctx, tx, principal) = super::begin_mutation(&ctx).await?;
    ctx.app
        .engine
        .stop_provider_runtime_in(
            tx,
            &principal,
            &crate::sign_in::origin(&ctx),
            id,
            body.interrupt.unwrap_or(false),
        )
        .await
        .map_err(refused)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    show(&ctx, &mut conn, &principal, id).await
}

/// What a refusal to bring a credential home is to the caller.
fn not_home(error: ReturnError) -> ApiError {
    match error {
        ReturnError::NotHeld => ApiError::new(
            ErrorCode::RequestInvalid,
            "the credential of this provider is not handed over",
        ),
        ReturnError::Busy(running) => ApiError::new(
            ErrorCode::ThreadNotIdle,
            "turns run on this provider in the environment that holds its credential; stop them first, or say to interrupt them",
        )
        .with("running", running),
        ReturnError::Failed(why) => ApiError::new(ErrorCode::CredentialReturnFailed, why),
        ReturnError::Internal(error) => ApiError::engine(error),
    }
}

/// Brings home a credential that was handed over to an environment, or gives it up when the
/// environment cannot give it back. Whoever manages the provider may do either; whoever manages only
/// the environment that holds it may bring it home. Turns of that environment that run on the provider
/// refuse it, unless the request says to interrupt them.
async fn handover_return(
    ctx: Ctx,
    (): (),
    body: api::ProviderHandoverReturnBody,
) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = super::begin_mutation(&ctx).await?;
    let state = rows::state(&mut tx, id)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let manages = authorization::require_resource(
        &mut tx,
        &principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
        Policy::default(),
    )
    .await;
    let lost = body.declare_lost.unwrap_or(false);
    match (manages, state.held_by_environment_id) {
        (Ok(()), _) => {}
        // Whoever manages only the environment that holds the credential may bring it home, and may
        // not give it up: that is for those who manage the provider.
        (Err(denied), Some(holder)) => {
            if lost {
                return Err(denied);
            }
            if authorization::require_resource(
                &mut tx,
                &principal,
                ResourceKind::Environment,
                holder,
                Permission::EnvironmentManage,
                Policy::default(),
            )
            .await
            .is_err()
            {
                return Err(denied);
            }
        }
        (Err(denied), None) => return Err(denied),
    }
    let done = ctx
        .app
        .engine
        .return_credential_in(
            tx,
            &principal,
            &crate::sign_in::origin(&ctx),
            id,
            body.interrupt.unwrap_or(false),
            lost,
        )
        .await;
    done.map_err(not_home)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    show(&ctx, &mut conn, &principal, id).await
}
