//! A single recorded credential trial, read without refreshing a provider or admitting work.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::nodes::view as node_view;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::resources::{ProviderTrial, ProviderTrialState};
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::{provider_facts, threads};
use orch_types::{Id, Time};
use serde::Deserialize;

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "provider_trial_names_orgs"),
        (ObjectKind::Project, "provider_trial_names_projects"),
        (ObjectKind::Topic, "provider_trial_names_topics"),
        (ObjectKind::Thread, "provider_trial_names_threads"),
    ],
    containers: None,
};

#[derive(Clone, Deserialize)]
struct Stored {
    state: ProviderTrialState,
    at: Option<Time>,
    input_ref: Option<Id>,
    thread_ref: Option<Id>,
    person_ref: Option<Id>,
}

struct Prepared {
    response: OperationPlan,
    trial: Option<Stored>,
    names: NameBounds,
}

async fn observation(conn: &mut Conn, provider: Id, now: Time) -> ApiResult<Option<Stored>> {
    let Some(observation) = provider_facts::trial_observation(conn, provider).await? else {
        return Ok(None);
    };
    let Some(receipt) = observation.receipt else {
        return Ok(None);
    };
    let mut trial: Stored = serde_json::from_value(receipt).map_err(ApiError::internal)?;
    if provider_facts::trial_pending(conn, provider).await? {
        if observation.turn_state.as_deref() == Some("running") {
            trial.state = ProviderTrialState::Running;
            trial.at = observation.started_at.map(Time::new);
        } else {
            trial.state = ProviderTrialState::Reserved;
        }
    } else if matches!(
        trial.state,
        ProviderTrialState::Reserved | ProviderTrialState::Running
    ) {
        trial.state = ProviderTrialState::NotReserved;
        trial.at = Some(now);
    }
    Ok(Some(trial))
}

async fn project(
    conn: &mut Conn,
    principal: &Principal,
    stored: &Stored,
    names: Option<&NameBounds>,
) -> ApiResult<ProviderTrial> {
    let mut trial = ProviderTrial {
        state: stored.state,
        at: stored.at,
        input_ref: None,
        thread_ref: None,
        reserved_for: None,
    };
    let (Some(input_id), Some(thread_id), Some(person_id)) =
        (stored.input_ref, stored.thread_ref, stored.person_ref)
    else {
        return Ok(trial);
    };
    // The ordinary read supplies exact nested targets. An act's token receipt carries only its safe state.
    if names.is_none() && principal.caller().grant().is_some()
        || names.is_some_and(|names| names.limited && !names.nodes.contains(&thread_id))
    {
        return Ok(trial);
    }
    if !nodes::check(
        conn,
        principal,
        thread_id,
        Permission::ThreadView,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed()
    {
        return Ok(trial);
    }
    let Some(input) = threads::input(conn, input_id).await? else {
        return Ok(trial);
    };
    let Some(thread) = threads::thread(conn, thread_id).await? else {
        return Ok(trial);
    };
    if input.thread_id != thread_id || input.continuation_id != thread.active_continuation_id {
        return Ok(trial);
    }
    let mut refs = match names {
        Some(names) => names.refs(conn, principal, &[thread_id]).await?,
        None => node_view::refs(conn, principal, &[thread_id], Policy::default()).await?,
    };
    trial.thread_ref = refs.remove(&thread_id);
    if trial.thread_ref.as_ref().is_none_or(|node| node.redacted) {
        return Ok(trial);
    }
    trial.input_ref = Some(input_id);
    let person = orch_engine::views::user_ref_json(conn, person_id)
        .await
        .map_err(ApiError::internal)?;
    let person: orch_api_contract::users::UserRef =
        serde_json::from_value(person).map_err(ApiError::internal)?;
    if !person.display_name.is_empty() {
        trial.reserved_for = Some(person);
    }
    Ok(trial)
}

pub(super) async fn for_reply(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    now: Time,
) -> ApiResult<Option<ProviderTrial>> {
    match observation(conn, provider, now).await? {
        Some(stored) => project(conn, principal, &stored, None).await.map(Some),
        None => Ok(None),
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let provider = ctx.id("id")?;
        let target = crate::key_operations::targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
        )
        .await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Provider), &[target])?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        super::providers::provider_for(conn, &principal, provider, Permission::ProviderView)
            .await?;
        super::require(
            conn,
            &principal,
            ResourceKind::Provider,
            provider,
            Permission::ProviderManage,
        )
        .await?;
        let trial = observation(conn, provider, Time::new(ctx.now)).await?;
        let mut sources = NameSources::default();
        if let Some(trial) = &trial {
            sources.nodes.extend(trial.thread_ref);
            sources.users.extend(trial.person_ref);
        }
        let names = projection_names(conn, caller, &mut plan, sources, &NAMES).await?;
        Ok(plan.clone().with_data(Prepared {
            response: plan,
            trial,
            names,
        }))
    })
}

pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<ProviderTrial>> {
    crate::key_operations::snapshot_read::run(ctx, read).await
}

async fn read(ctx: Ctx) -> ApiResult<Option<ProviderTrial>> {
    let completed = crate::key_operations::native::read_once(ctx, |ctx, conn, principal| {
        Box::pin(async move {
            let provider = ctx.id("id")?;
            let prepared = ctx.operation_data::<Prepared>()?;
            prepared
                .response
                .clone()
                .read_snapshot_response(ctx, conn, principal, move |conn, reader| {
                    Box::pin(async move {
                        super::providers::provider_for(
                            conn,
                            reader,
                            provider,
                            Permission::ProviderView,
                        )
                        .await?;
                        super::require(
                            conn,
                            reader,
                            ResourceKind::Provider,
                            provider,
                            Permission::ProviderManage,
                        )
                        .await?;
                        match &prepared.trial {
                            Some(trial) => project(conn, reader, trial, Some(&prepared.names))
                                .await
                                .map(Some),
                            None => Ok(None),
                        }
                    })
                })
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the trial response content has not been admitted",
                    )
                })
        })
    })
    .await?;
    Ok(completed.value)
}
