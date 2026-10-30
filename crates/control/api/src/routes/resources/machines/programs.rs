//! Reported program registry and authorized native operations, with no transaction across a machine wait.

use super::*;
use orch_api_contract::machine_programs::{MachinePrograms, MachineProgramsCommand};

pub(super) fn register(registry: &mut Registry) {
    registry
        .on(api::MachineExecutorList, executors)
        .on(api::MachineProgramsGet, get)
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
    let mut conn = ctx.app.db.acquire().await?;
    let row = own_machine(&ctx, &mut conn, ctx.id("id")?).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    projection(&mut conn, &principal, &row).await
}

/// The executors the machine can offer: those of the registry, which a connector release carries, and the executor
/// plugins it reported. Read from what the machine last said; nothing is asked of it.
async fn executors(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::programs::MachineExecutor>> {
    let mut conn = ctx.app.db.acquire().await?;
    let row = own_machine(&ctx, &mut conn, ctx.id("id")?).await?;
    Ok(crate::programs::machine_executors(
        orch_agent_programs::Programs::builtin(),
        &shown_facts(&row.facts),
    ))
}

async fn change(ctx: Ctx, (): (), body: MachineProgramsCommand) -> ApiResult<MachinePrograms> {
    let id = ctx.id("id")?;
    let (ctx, mut conn, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = own_machine(&ctx, &mut conn, id).await?;
    require_machine(&mut conn, &principal, id, Permission::MachineManage).await?;
    require_online(&ctx, &row)?;
    let (operation, name, kind) = match &body {
        MachineProgramsCommand::Register(entry) => {
            ("registered", Some(entry.name.clone()), Some(entry.kind))
        }
        MachineProgramsCommand::Test(entry) => {
            ("tested", Some(entry.name.clone()), Some(entry.kind))
        }
        MachineProgramsCommand::Remove(entry) => {
            ("removed", Some(entry.name.clone()), Some(entry.kind))
        }
        MachineProgramsCommand::Reload => ("reloaded", None, None),
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
    audit_machine(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MachineProgramsChanged),
        id,
        &format!("{who} {operation} programs on {}", row.name),
        json!({"operation":operation,"name":name,"kind":kind}),
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
