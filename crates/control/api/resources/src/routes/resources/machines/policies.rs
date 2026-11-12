//! Native key-by-key policy changes, sharing preparation with the Host policy synchronization.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::Stored;

use crate::key_operations::native::resource_reply::StoredResource;

use crate::middleware::transaction_guard;

use futures::future::BoxFuture;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::machine_sources as sources;

use orch_api_contract::machine_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::HostPolicyAuthority;

use orch_engine::commands::MachinePolicyAuthority;

use orch_engine::commands::Outgoing;

use orch_engine::commands::PolicyFailure;

use orch_engine::commands::PolicyPreparation;

use orch_engine::commands::prepare_policy;

use orch_engine::commands::validate_policy_drivers;

use orch_types::Clock;
pub(super) struct Changes;

impl Stored for Changes {
    type Source = Source;
    type Value = mutations::Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::POLICY];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        <mutations::Machines as Stored>::load(ctx, conn, declaration)
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let id = <mutations::Machines as StoredResource>::object(value).id;
            machine_for_policy(ctx, conn, principal, id).await?;
            Ok(())
        })
    }
}

struct Content<const INCLUDE_MACHINE: bool>;

impl<const INCLUDE_MACHINE: bool> acknowledgement::Body for Content<INCLUDE_MACHINE> {
    type Reply = MachinePolicy;
    const DECLARATION: &'static Declaration<Source> = &sources::POLICY;
    const WITH_MACHINE: bool = INCLUDE_MACHINE;
    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            machine_for_policy(ctx, conn, principal, row.id).await?;
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
        machine: Option<Machine>,
    ) -> BoxFuture<'a, ApiResult<MachinePolicy>> {
        Box::pin(async move {
            let mut policy = policy_of(ctx, conn, principal, row).await?;
            policy.machine_view = machine;
            Ok(policy)
        })
    }
}

enum Entry {
    Machine,
    Host,
}

enum Evidence {
    Machine(MachinePolicyAuthority),
    Host(HostPolicyAuthority),
}

impl Evidence {
    fn authority(&self) -> CommandAuthority {
        match self {
            Self::Machine(value) => CommandAuthority::machine_policy(value.clone()),
            Self::Host(value) => CommandAuthority::host_policy(value.clone()),
        }
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(orch_authz::Caller, Option<Time>)>, orch_engine::EngineError> {
        match self {
            Self::Machine(value) => value.current(conn, clock).await,
            Self::Host(value) => value.current(conn, clock).await,
        }
    }
}

struct Completed {
    ctx: Ctx,
    accepted: orch_authz::Caller,
    changed: bool,
}

fn receipt(id: Id) -> api::MachinePolicyResponse {
    api::MachinePolicyResponse::Receipt(api::MachinePolicyReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
    })
}

pub(super) async fn update(
    ctx: Ctx,
    (): (),
    body: api::MachinePolicyUpdateBody,
) -> ApiResult<api::MachinePolicyResponse> {
    let projection = ctx.clone();
    match update_on(ctx, body).await {
        Ok(answer) => Ok(answer),
        Err(error) => Err(acknowledgement::refusal::<Content<false>>(&projection, error).await?),
    }
}

async fn update_on(
    ctx: Ctx,
    body: api::MachinePolicyUpdateBody,
) -> ApiResult<api::MachinePolicyResponse> {
    let id = ctx.id("id")?;
    let done = execute(ctx, id, body, Entry::Machine).await?;
    crate::key_operations::publication::receipt(&done.ctx, &receipt(id))?;
    let value = if done.changed {
        acknowledgement::project::<Content<true>>(&done.ctx, &done.accepted, id).await?
    } else {
        acknowledgement::project::<Content<false>>(&done.ctx, &done.accepted, id).await?
    };
    Ok(match value {
        Some(value) => api::MachinePolicyResponse::Full(Box::new(value)),
        None => receipt(id),
    })
}

/// The Host writer's existing helper has one consumer, which discards its old policy readback.
/// Keep its original operation on native synchronization and avoid materializing an unused body.
pub(super) async fn synchronize(
    ctx: Ctx,
    machine: Id,
    body: api::MachinePolicyUpdateBody,
) -> ApiResult<()> {
    execute(ctx, machine, body, Entry::Host).await?;
    Ok(())
}

async fn failure(conn: &mut Conn, failure: PolicyFailure) -> ApiResult<ApiError> {
    Ok(match failure {
        PolicyFailure::Missing => ApiError::not_found(),
        PolicyFailure::Invalid(name, detail) => invalid(&name, &detail),
        PolicyFailure::Refused(refusal) => refusals::denied(conn, *refusal).await?,
        PolicyFailure::NotOwner(key) => ApiError::new(
            ErrorCode::AuthzDenied,
            format!("{key} is changed by the machine's owner"),
        )
        .with("required", "machine.owner")
        .with("requestable", false)
        .with("override", false)
        .with("missing", Vec::<Json>::new()),
        PolicyFailure::NotDelegated(keys) => {
            let named = keys
                .iter()
                .map(|name| {
                    name.parse::<PolicyKey>().map_or_else(
                        |_| name.clone(),
                        |key| {
                            format!(
                                "{name} ({} role)",
                                key.sections()
                                    .iter()
                                    .map(|section| section.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        },
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            ApiError::new(ErrorCode::PolicyNotDelegated,format!("{named}: kept by the machine's operator; change the value locally or ask that operator to hand it to Orch")).with("keys",keys)
        }
        PolicyFailure::Drivers(errors) => Problem::invalid(errors).into(),
        PolicyFailure::Native(error) => ApiError::engine(error),
    })
}

async fn execute(
    ctx: Ctx,
    machine: Id,
    body: api::MachinePolicyUpdateBody,
    entry: Entry,
) -> ApiResult<Completed> {
    let (credential, raw_until) = transaction_guard::native_origin(&ctx).await?;
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let prepared = match prepare_policy(&mut conn, guard.principal(), machine, &body).await {
        Ok(value) => value,
        Err(error) => return Err(failure(&mut conn, error).await?),
    };
    if prepared.changes.is_empty() {
        guard.check_deadline(&ctx)?;
        let accepted = guard.principal().caller().clone();
        conn.commit().await.map_err(DbError::from)?;
        return Ok(Completed {
            ctx,
            accepted,
            changed: false,
        });
    }
    require_online(&ctx, &prepared.row)?;
    if let Err(error) = validate_policy_drivers(&mut conn, &prepared).await {
        return Err(failure(&mut conn, error).await?);
    }
    let PolicyPreparation {
        row,
        before,
        changes,
    } = prepared;
    let sent = MachinePolicySet {
        values: changes
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), value.clone()))
            .collect(),
    };
    let command = Outgoing::new(orch_types::new_id(), machine, SerialKey::Machine, &sent)
        .map_err(ApiError::internal)?;
    let evidence = match entry {
        Entry::Machine => Evidence::Machine(
            MachinePolicyAuthority::capture(
                &mut conn,
                guard.principal().caller(),
                credential,
                raw_until,
                &command,
                ctx.app.clock.as_ref(),
            )
            .await
            .map_err(ApiError::internal)?,
        ),
        Entry::Host => Evidence::Host(
            HostPolicyAuthority::capture(
                &mut conn,
                guard.principal().caller(),
                credential,
                raw_until,
                &command,
                ctx.app.clock.as_ref(),
            )
            .await
            .map_err(ApiError::internal)?,
        ),
    };
    guard.check_deadline(&ctx)?;
    conn.commit().await.map_err(DbError::from)?;
    let dispatcher = ctx.app.link.dispatcher();
    let outcome = async {
        dispatcher
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    id: Some(command.id),
                    machine,
                    kind: command.kind.as_str().into(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body.clone(),
                    authority: Some(evidence.authority()),
                },
                Box::new(|| {
                    guard.check_deadline(&ctx).map_err(|error| {
                        Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                    })
                }),
            )
            .await?;
        dispatcher.wait_for::<MachinePolicySet>(command.id).await
    }
    .await;
    let result = outcome.map_err(|error| command_failed(machine, error))?;
    if !result.refused.is_empty() {
        return Err(ApiError::new(
            ErrorCode::PolicyNotDelegated,
            "the machine keeps these keys to itself",
        )
        .with("keys", result.refused));
    }
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    if !rows::lock_resource(&mut tx, "machine", machine).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let (accepted, until) = evidence
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the original machine policy operation has ended",
            )
        })?;
    let now_in_force = serde_json::to_value(&result.policy).map_err(ApiError::internal)?;
    guard.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    queries::set_machine_policy(&mut tx, machine, &now_in_force, ctx.now).await?;
    let applied: Vec<&str> = result.applied.iter().map(String::as_str).collect();
    let values_of = |policy: Option<&LocalPolicy>| -> Json {
        let mut shown = serde_json::Map::new();
        for name in &applied {
            if let (Some(policy), Ok(key)) = (policy, name.parse::<PolicyKey>()) {
                shown.insert((*name).to_owned(), policy.get(key));
            }
        }
        Json::Object(shown)
    };
    let who = actor_name(&ctx)?;
    audit_machine(&ctx,&mut tx,orch_db::audit::action!(AuditAction::MachinePolicyChanged),machine,
        &format!("{who} changed the local policy of the machine {}",row.name),
        json!({"machine":machine,"keys":applied,"before":values_of(before.as_ref()),"after":values_of(Some(&result.policy))})).await?;
    guard.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Completed {
        ctx,
        accepted,
        changed: true,
    })
}
