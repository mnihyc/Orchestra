//! Actual creation home/path facts and optional projection for the existing provider writers.
use crate::key_operations::{admission::Prepared, targets};
use crate::middleware::{
    authentication, authorization,
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::{Ctx, OperationFuture};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::operations::TargetResolver;
use orch_api_contract::resources::{
    CreateProvider, MachineProviderCreateBody, Provider, ProviderCreateReceipt,
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, Tx};
use orch_types::Id;

#[derive(Clone)]
pub(super) struct Operands {
    pub request: CreateProvider,
    plan: Prepared,
}

/// A failed creation has no actual new Provider whose Read could authorize its
/// existing source/grantor metadata. Use the established bounded Problem envelope
/// for modern operation keys; browser and legacy refusals keep their ordinary form.
pub(super) fn problem(caller: &Caller, error: ApiError) -> ApiError {
    if caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

pub(super) fn machine_request(machine: Id, body: MachineProviderCreateBody) -> CreateProvider {
    match body {
        MachineProviderCreateBody::Subscription {
            home_id,
            name,
            agent_id,
            sources,
        } => CreateProvider::Subscription {
            name,
            home_id,
            agent_id,
            sources,
            runtime_machine_id: Some(machine),
            key_machine_id: Some(machine),
            access: None,
            arrangements: None,
            runtime_config: None,
            config: None,
            sign_in: None,
        },
        MachineProviderCreateBody::Api {
            home_id,
            name,
            upstreams,
            sources,
        } => CreateProvider::Api {
            name,
            home_id,
            upstreams,
            sources,
            runtime_machine_id: Some(machine),
            key_machine_id: Some(machine),
            access: None,
            arrangements: None,
            runtime_config: None,
            api_key: None,
            secrets: None,
        },
    }
}
async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    mut request: CreateProvider,
) -> ApiResult<Prepared> {
    let home = match &request {
        CreateProvider::Api { home_id, .. } | CreateProvider::Subscription { home_id, .. } => {
            *home_id
        }
    };
    let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
    let target = targets::node(conn, home).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
        return Err(crate::routes::nodes::invalid(
            "home_id",
            "a provider is at home in an org",
        ));
    }
    let slot = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::Resolved("home"))
        .ok_or_else(|| ApiError::internal("the provider creation home is not declared"))?;
    plan.bind(slot, KeyTargetKind::Object(ObjectKind::Provider), &[target])?;
    if ctx.route.name == "MachineProviderCreate" {
        let machine = ctx.id("id")?;
        let target = targets::machine_subject(conn, machine).await?;
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Machine), &[target])?;
    }
    // Known home/path scope precedes effect-free profile lookup and named validation.
    // The ordinary writer still checks current MayMake and resource Use.
    let _ = plan.preparation_principal(ctx, conn, caller).await?;
    // This is an ordinary selection operand, not a new Machine action. The shared writer
    // repeats current AgentUse/MachineUse/local-policy checks against this exact keeper.
    if let CreateProvider::Subscription {
        agent_id,
        key_machine_id,
        ..
    } = &mut request
    {
        let profile = orch_db::queries::resources::thread_engine::agent(conn, *agent_id)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(|| crate::routes::nodes::invalid("agent_id", "this is no profile"))?;
        *key_machine_id = key_machine_id.or(profile.machine_id);
    }
    Ok(plan.clone().with_data(Operands { request, plan }))
}
pub(super) fn create<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: CreateProvider,
) -> OperationFuture<'a> {
    Box::pin(async move {
        prepare(ctx, conn, caller, body)
            .await
            .map_err(|error| problem(caller, error))
    })
}
pub(super) fn machine<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: MachineProviderCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        prepare(ctx, conn, caller, machine_request(ctx.id("id")?, body))
            .await
            .map_err(|error| problem(caller, error))
    })
}
/// Keep the normalized request from this actual guard snapshot, including an inferred keeper.
pub(super) async fn begin(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Principal, Operands)> {
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let operands = guard.operation_data::<Operands>()?.as_ref().clone();
    let mut current = ctx.clone();
    current.caller = authentication::Caller::User(std::sync::Arc::new(guard.user().clone()));
    current.now = guard.now().get();
    Ok((current, tx, guard.into_principal(), operands))
}
pub(super) async fn receipt(
    conn: &mut Conn,
    provider: Id,
    warning: Option<String>,
) -> ApiResult<ProviderCreateReceipt> {
    let revision = orch_db::queries::resources::routes::providers(conn, &[provider])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?
        .revision;
    Ok(ProviderCreateReceipt {
        projection: ReceiptProjection::Receipt,
        provider_id: provider,
        revision,
        warning,
    })
}
pub(super) async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    operands: &Operands,
    provider: Id,
) -> ApiResult<Option<Provider>> {
    let mut plan = operands.plan.clone();
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::Resolved("created_provider"))
        .ok_or_else(|| ApiError::internal("the actual created provider Read is not declared"))?;
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    )
    .await?;
    plan.bind(
        index,
        KeyTargetKind::Object(ObjectKind::Provider),
        &[target],
    )?;
    let projection = ctx.clone();
    super::provider_projection::read_plan(
        ctx,
        conn,
        principal.caller(),
        provider,
        plan,
        move |conn, reader, prepared| {
            Box::pin(async move {
                super::providers::show_prepared(
                    &projection,
                    conn,
                    reader,
                    prepared.ok_or_else(|| {
                        ApiError::internal("the bounded creation projection is absent")
                    })?,
                )
                .await
            })
        },
    )
    .await
}

/// The exact initial Make keeps the proven creator operation before commit. A queued
/// existing provider or autonomous wake cannot borrow this origin.
pub(super) async fn retain_make(
    conn: &mut Conn,
    caller: &Caller,
    provider: Id,
    home: Id,
    queued: orch_db::queries::jobs::provider_jobs::Queued,
    now: orch_types::Time,
) -> ApiResult<()> {
    let orch_db::queries::jobs::provider_jobs::Queued::Made(job) = queued else {
        // This helper is only called for a fresh random Provider ID in this transaction.
        // An old or absent job cannot receive this creator's authority.
        return Err(ApiError::internal(
            "the new provider did not queue its initial Make",
        ));
    };
    let origin = orch_authz::CapturedOperation::capture(&caller.clone().with_admin_mode(false))
        .map_err(ApiError::internal)?;
    orch_engine::jobs::provider_provision::retain_creator_origin(
        conn, &origin, provider, home, job.id, now,
    )
    .await
    .map_err(ApiError::engine)?;
    Ok(())
}
