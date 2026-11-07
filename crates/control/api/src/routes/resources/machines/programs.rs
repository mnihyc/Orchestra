//! Reported program registry and authorized native operations, with no transaction across a machine wait.

use super::*;
use crate::key_operations::native::{self, Stored};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_api_contract::machine_programs::{
    MachinePrograms, MachineProgramsCommand, MachineProgramsReceipt, MachineProgramsResponse,
};
use orch_api_contract::{
    common::ReceiptProjection,
    machine_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_engine::commands::{CommandAuthority, MachineProgramsAuthority, Outgoing};

pub(super) struct Changes;
impl Stored for Changes {
    type Source = Source;
    type Value = mutations::Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::PROGRAMS];
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
        <mutations::Machines as Stored>::require(ctx, conn, principal, value)
    }
}

struct Content;
impl acknowledgement::Body for Content {
    type Reply = MachinePrograms;
    const DECLARATION: &'static Declaration<Source> = &sources::PROGRAMS;
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
        machine: Option<Machine>,
    ) -> BoxFuture<'a, ApiResult<MachinePrograms>> {
        Box::pin(async move {
            let mut answer = projection(conn, principal, row).await?;
            answer.machine_view = Some(machine.ok_or_else(|| {
                ApiError::internal("the native program acknowledgement has no individual Machine")
            })?);
            Ok(answer)
        })
    }
}

pub(super) fn register(registry: &mut Registry) {
    registry
        .on_with_operation(api::MachineExecutorList, read_operation::prepare, executors)
        .on_with_operation(api::MachineProgramsGet, read_operation::prepare, get)
        .on_with_optional_operation(
            api::MachineProgramsChange,
            native::stored_deferred::<Changes, _, _>,
            change,
        );
}

async fn projection(
    conn: &mut Conn,
    principal: &Principal,
    row: &MachineRow,
) -> ApiResult<MachinePrograms> {
    let facts = shown_facts(&row.facts);
    let mut programs = match facts.get("programs").filter(|value| !value.is_null()) {
        Some(value) => {
            serde_json::from_value::<MachinePrograms>(value.clone()).map_err(ApiError::internal)?
        }
        None => MachinePrograms::default(),
    };
    programs.can_manage = orch_authz::resources::check_machine(
        conn,
        principal,
        row.id,
        Permission::MachineManage,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed();
    Ok(programs)
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<MachinePrograms> {
    crate::key_operations::snapshot_read::run(ctx, programs_snapshot).await
}

async fn programs_snapshot(ctx: Ctx) -> ApiResult<MachinePrograms> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let row = own_machine_on(&mut tx, &principal, ctx.id("id")?).await?;
    let shown = projection(&mut tx, &principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// The executors the machine can offer: those of the registry, which a connector release carries, and the executor
/// plugins it reported. Read from what the machine last said; nothing is asked of it.
async fn executors(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::programs::MachineExecutor>> {
    crate::key_operations::snapshot_read::run(ctx, executors_snapshot).await
}

async fn executors_snapshot(
    ctx: Ctx,
) -> ApiResult<Vec<orch_api_contract::programs::MachineExecutor>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let row = own_machine_on(&mut tx, &principal, ctx.id("id")?).await?;
    let shown = crate::programs::machine_executors(
        orch_agent_programs::Programs::builtin(),
        &shown_facts(&row.facts),
    );
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

fn harness_version(facts: &Json, kind: &str, path: &str) -> String {
    facts
        .get("harnesses")
        .and_then(Json::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["kind"] == kind && entry["path"] == path)
        })
        .and_then(|entry| entry.get("version"))
        .and_then(Json::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn receipt(id: Id) -> MachineProgramsResponse {
    MachineProgramsResponse::Receipt(MachineProgramsReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
    })
}

async fn change(
    ctx: Ctx,
    (): (),
    body: MachineProgramsCommand,
) -> ApiResult<MachineProgramsResponse> {
    let projection = ctx.clone();
    match change_on(ctx, body).await {
        Ok(answer) => Ok(answer),
        Err(error) => Err(acknowledgement::refusal::<Content>(&projection, error).await?),
    }
}

async fn change_on(ctx: Ctx, body: MachineProgramsCommand) -> ApiResult<MachineProgramsResponse> {
    let id = ctx.id("id")?;
    let (credential, raw_until) = transaction_guard::native_origin(&ctx).await?;
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let row = own_machine(&ctx, &mut conn, id).await?;
    require_machine(&mut conn, guard.principal(), id, Permission::MachineManage).await?;
    require_online(&ctx, &row)?;
    let (operation, name, kind, harness) = match &body {
        MachineProgramsCommand::Register(entry) => (
            "registered",
            Some(entry.name.clone()),
            Some(json!(entry.kind)),
            None,
        ),
        MachineProgramsCommand::Test(entry) => (
            "tested",
            Some(entry.name.clone()),
            Some(json!(entry.kind)),
            None,
        ),
        MachineProgramsCommand::Remove(entry) => (
            "removed",
            Some(entry.name.clone()),
            Some(json!(entry.kind)),
            None,
        ),
        MachineProgramsCommand::ConfirmHarness { kind, path } => (
            "confirmed",
            None,
            Some(json!(kind)),
            Some((kind.as_str(), path.as_str(), true)),
        ),
        MachineProgramsCommand::RemoveHarness { kind, path } => (
            "unconfirmed",
            None,
            Some(json!(kind)),
            Some((kind.as_str(), path.as_str(), false)),
        ),
        MachineProgramsCommand::Reload => ("reloaded", None, None, None),
    };
    let command = Outgoing::new(orch_types::new_id(), id, SerialKey::Machine, &body)
        .map_err(ApiError::internal)?;
    let authority = MachineProgramsAuthority::capture(
        &mut conn,
        guard.principal().caller(),
        credential,
        raw_until,
        &command,
        ctx.app.clock.as_ref(),
    )
    .await
    .map_err(ApiError::internal)?;
    guard.check_deadline(&ctx)?;
    conn.commit().await.map_err(DbError::from)?;
    let dispatcher = ctx.app.link.dispatcher();
    let outcome = async {
        dispatcher
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    id: Some(command.id),
                    machine: id,
                    kind: command.kind.as_str().into(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body.clone(),
                    authority: Some(CommandAuthority::machine_programs(authority.clone())),
                },
                Box::new(|| {
                    guard.check_deadline(&ctx).map_err(|error| {
                        Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                    })
                }),
            )
            .await?;
        dispatcher
            .wait_for::<MachineProgramsCommand>(command.id)
            .await
    }
    .await;
    let facts: MachineFacts = outcome.map_err(|failure| match failure {
        RunError::Command(CommandFailure::Failed(refusal))
            if harness.is_some() && refusal.code == "conflict" =>
        {
            ApiError::new(ErrorCode::ConflictStaleRevision, &refusal.detail)
        }
        RunError::Command(CommandFailure::Failed(refusal))
            if harness.is_some()
                && refusal
                    .details
                    .as_ref()
                    .and_then(|details| details.get("policy_key"))
                    .and_then(Json::as_str)
                    == Some("agent.harnesses") =>
        {
            ApiError::new(ErrorCode::PolicyNotDelegated, &refusal.detail)
                .with("keys", vec!["agent.harnesses"])
        }
        RunError::Command(CommandFailure::Failed(refusal)) => {
            ApiError::invalid("program", &refusal.detail)
        }
        other => command_failed(id, other),
    })?;
    let written = serde_json::to_value(&facts).map_err(ApiError::internal)?;
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    if !rows::lock_resource(&mut tx, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    require_machine(&mut tx, guard.principal(), id, Permission::MachineManage).await?;
    let (accepted, until) = authority
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the original program operation has ended",
            )
        })?;
    guard.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    queries_set_facts(&mut tx, id, &written, &facts, ctx.app.clock.now()).await?;
    let who = actor_name(&ctx)?;
    let mut details = json!({"operation":operation,"name":name,"kind":kind});
    if let Some((kind, path, confirm)) = harness {
        details["path"] = json!(path);
        details["version"] = json!(harness_version(
            if confirm { &written } else { &row.facts },
            kind,
            path
        ));
    }
    audit_machine(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MachineProgramsChanged),
        id,
        &format!("{who} {operation} programs on {}", row.name),
        details,
    )
    .await?;
    guard.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&ctx, &receipt(id))?;
    Ok(
        match acknowledgement::project::<Content>(&ctx, &accepted, id).await? {
            Some(answer) => MachineProgramsResponse::Full(Box::new(answer)),
            None => receipt(id),
        },
    )
}
