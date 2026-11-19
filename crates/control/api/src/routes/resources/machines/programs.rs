//! Reported program registry and authorized native operations, with no transaction across a machine wait.

use super::*;
use orch_api_contract::machine_programs::{MachinePrograms, MachineProgramsCommand};

pub(super) fn register(registry: &mut Registry) {
    registry
        .on_with_operation(api::MachineExecutorList, read_operation::prepare, executors)
        .on_with_operation(api::MachineProgramsGet, read_operation::prepare, get)
        .on(api::MachineProgramsChange, change);
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

async fn change(ctx: Ctx, (): (), body: MachineProgramsCommand) -> ApiResult<MachinePrograms> {
    let id = ctx.id("id")?;
    let (ctx, mut conn, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = own_machine(&ctx, &mut conn, id).await?;
    require_machine(&mut conn, &principal, id, Permission::MachineManage).await?;
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
    // The same credential/authority decision as other native machine mutations, committed first.
    conn.commit().await.map_err(DbError::from)?;
    let facts: MachineFacts = ctx
        .app
        .link
        .dispatcher()
        .run(id, &SerialKey::Machine, &body)
        .await
        .map_err(|failure| match failure {
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
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    require_machine(&mut tx, guard.principal(), id, Permission::MachineManage).await?;
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
    let current = rows::machine(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut answer = projection(&mut tx, guard.principal(), &current).await?;
    answer.machine_view = Some(machine_with_readiness(&ctx, &mut tx, current).await?);
    tx.commit().await.map_err(DbError::from)?;
    Ok(answer)
}
