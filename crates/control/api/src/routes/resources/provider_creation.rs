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

pub(super) mod duplicate;
mod replays;
pub(in crate::routes::resources) use replays::{machine_binding, observed, provider_binding};

#[derive(Clone)]
pub(super) struct Operands {
    pub request: CreateProvider,
    pub home_overridden: bool,
    plan: Prepared,
    duplicate: Option<duplicate::Source>,
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

fn machine_request(machine: Id, home: Id, body: MachineProviderCreateBody) -> CreateProvider {
    match body {
        MachineProviderCreateBody::Subscription {
            home_id: _,
            name,
            agent_id,
            sources,
        } => CreateProvider::Subscription {
            name,
            home_id: home,
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
            home_id: _,
            name,
            upstreams,
            sources,
        } => CreateProvider::Api {
            name,
            home_id: home,
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
    home_overridden: bool,
) -> ApiResult<Prepared> {
    let home = match &request {
        CreateProvider::Api { home_id, .. } | CreateProvider::Subscription { home_id, .. } => {
            *home_id
        }
    };
    let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
    let target = targets::node(conn, home).await?;
    if !matches!(
        target.object.kind,
        KeyTargetKind::Object(ObjectKind::Org | ObjectKind::Project)
    ) {
        return Err(crate::routes::nodes::invalid(
            "home_id",
            "a provider is at home in an org or project",
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
    Ok(plan.clone().with_data(Operands {
        request,
        home_overridden,
        plan,
        duplicate: None,
    }))
}
pub(super) fn create<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: CreateProvider,
) -> OperationFuture<'a> {
    Box::pin(async move {
        prepare(ctx, conn, caller, body, true)
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
        let machine = ctx.id("id")?;
        let mut initial = Prepared::new(ctx.route, ctx.route.name)?;
        let target = targets::machine_subject(conn, machine).await?;
        initial.bind(0, KeyTargetKind::Object(ObjectKind::Machine), &[target])?;
        let principal = initial.preparation_principal(ctx, conn, caller).await?;
        super::machines::require_machine(
            conn,
            &principal,
            machine,
            orch_access_contract::permissions::Permission::MachineUse,
        )
        .await
        .map_err(|error| problem(caller, error))?;
        let supplied = match &body {
            MachineProviderCreateBody::Api { home_id, .. }
            | MachineProviderCreateBody::Subscription { home_id, .. } => *home_id,
        };
        let home = match supplied {
            Some(id) => id,
            None => orch_db::queries::resources::machine_registration_home(conn, machine, None)
                .await?
                .map(|(home, _)| home)
                .ok_or_else(ApiError::not_found)?,
        };
        prepare(
            ctx,
            conn,
            caller,
            machine_request(machine, home, body),
            supplied.is_some(),
        )
        .await
        .map_err(|error| problem(caller, error))
    })
}
/// Record only an explicit creation home; automatic discovery/default selection stays unpinned.
pub(super) async fn pin_home(
    conn: &mut Conn,
    provider: Id,
    operands: &Operands,
    owner: Id,
    at: orch_types::Time,
) -> ApiResult<()> {
    if operands.duplicate.is_some()
        && matches!(&operands.request, CreateProvider::Subscription { .. })
        && !orch_db::queries::resources::provider_runtime::set_initial_owned_credential_home(
            conn, provider, owner,
        )
        .await?
    {
        return Err(ApiError::internal(
            "the new subscription cannot select its own credential home",
        ));
    }
    if operands.home_overridden {
        sqlx::query("UPDATE providers SET home_overridden_at=$2 WHERE id=$1")
            .bind(provider)
            .bind(at.get())
            .execute(conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    Ok(())
}

/// Keep the normalized request from this actual guard snapshot, including an inferred keeper.
pub(super) async fn begin(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Principal, Operands)> {
    let (tx, guard) = crate::key_operations::native::observed(
        ctx,
        "provider.creation.begin",
        transaction_guard::begin(ctx).await,
    )?;
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
    if operands.duplicate.is_some() {
        return Ok(None);
    }
    project(
        ctx,
        conn,
        principal.caller(),
        operands.plan.clone(),
        provider,
        super::provider_projection::Purpose::CompletedContent,
    )
    .await
}

/// Read only current response content after the accepted creator's source reads have finished.
pub(super) async fn read_after(
    ctx: &Ctx,
    accepted: &Caller,
    operands: &Operands,
    provider: Id,
) -> ApiResult<Option<Provider>> {
    // A duplicate's native reply is its receipt. Its source Read is not an
    // optional Read of the new provider, which this operation does not declare.
    if operands.duplicate.is_some() {
        return Ok(None);
    }
    let plan = operands.plan.clone();
    crate::key_operations::native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(project(
            ctx,
            conn,
            current.caller(),
            plan,
            provider,
            super::provider_projection::Purpose::CompletedContent,
        ))
    })
    .await
}

async fn project(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    mut plan: Prepared,
    provider: Id,
    purpose: super::provider_projection::Purpose,
) -> ApiResult<Option<Provider>> {
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
        caller,
        provider,
        plan,
        purpose,
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
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    operands: &Operands,
    provider: Id,
    queued: orch_db::queries::jobs::provider_jobs::Queued,
) -> ApiResult<()> {
    let caller = principal.caller();
    let now = principal.now();
    let home = match &operands.request {
        CreateProvider::Api { home_id, .. } | CreateProvider::Subscription { home_id, .. } => {
            *home_id
        }
    };
    let orch_db::queries::jobs::provider_jobs::Queued::Made(job) = queued else {
        // This helper is only called for a fresh random Provider ID in this transaction.
        // An old or absent job cannot receive this creator's authority.
        return Err(ApiError::internal(
            "the new provider did not queue its initial Make",
        ));
    };
    if let Some(source) = &operands.duplicate {
        let origin = orch_authz::CapturedOperation::capture(caller).map_err(ApiError::internal)?;
        let raw = crate::key_operations::native::http_origin::capture_operation(ctx, conn, &origin)
            .await?;
        if let Some(fingerprint) = source.moving {
            return orch_engine::jobs::provider_provision::retain_move_creator_origin(
                conn,
                &raw,
                (source.provider, fingerprint),
                provider,
                home,
                job.id,
                ctx.app.clock.as_ref(),
            )
            .await
            .map_err(duplicate::creator_error);
        }
        return orch_engine::jobs::provider_provision::retain_duplicate_creator_origin(
            conn,
            &raw,
            source.provider,
            provider,
            home,
            job.id,
            ctx.app.clock.as_ref(),
        )
        .await
        .map_err(duplicate::creator_error);
    }
    let origin = orch_authz::CapturedOperation::capture(&caller.clone().with_admin_mode(false))
        .map_err(ApiError::internal)?;
    orch_engine::jobs::provider_provision::retain_creator_origin(
        conn, &origin, provider, home, job.id, now,
    )
    .await
    .map_err(ApiError::engine)?;
    Ok(())
}

/// The original source writer also accepts a copied definition without another owner's secrets.
pub(super) async fn write_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: Id,
    operands: &Operands,
) -> ApiResult<()> {
    let sources = match &operands.request {
        CreateProvider::Api { sources, .. } | CreateProvider::Subscription { sources, .. } => {
            sources.as_ref()
        }
    };
    if let Some(source) = &operands.duplicate {
        super::provider_sources::write_copied_definition(ctx, conn, provider, sources).await?;
        duplicate::write_runtime_secrets(ctx, conn, provider, source).await
    } else {
        super::provider_sources::write_new(ctx, conn, provider, sources).await
    }
}
