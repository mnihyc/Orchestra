//! Delegated role selection, with native acknowledgement before a supervised reconnect.

use super::*;

pub(super) fn register(registry: &mut Registry) {
    registry.on(api::MachineRolesSet, change);
}

async fn change(
    ctx: Ctx,
    (): (),
    body: api::MachineRolesSetBody,
) -> ApiResult<api::MachineRolesChange> {
    let roles = orch_link_contract::enrollment_choices::connector_roles(&body.roles.join(","))
        .map_err(|detail| ApiError::invalid("roles", detail))?;
    if roles.len() != body.roles.len() {
        return Err(ApiError::invalid(
            "roles",
            "Choose each role as a separate name.",
        ));
    }
    let id = ctx.id("id")?;
    let (ctx, mut conn, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = own_machine(&ctx, &mut conn, id).await?;
    require_machine(&mut conn, &principal, id, Permission::MachineManage).await?;
    require_online(&ctx, &row)?;
    if row.roles.iter().any(|role| role == "node") {
        return Err(ApiError::invalid(
            "roles",
            orch_link_contract::enrollment_choices::CONNECTOR_NODE_REFUSAL,
        ));
    }
    let policy = reported_policy(&mut conn, &row).await?;
    let policy = policy.as_ref().ok_or_else(|| {
        ApiError::invalid(
            "roles",
            "This machine has not reported its role policy. Update its connector and try again.",
        )
    })?;
    if !policy.delegates(PolicyKey::MachineRoles) || !policy.machine_roles {
        return Err(ApiError::invalid(
            "roles",
            "machine.roles (machine section): the machine operator keeps role changes here or has switched them off. Use the role command on this machine, or hand machine.roles to Orch.",
        ));
    }
    let current = machine_with_readiness(&ctx, &mut conn, row.clone()).await?;
    for (role, used, detail) in [
        (
            Role::Environment,
            !current.environments.is_empty(),
            "Remove its registered environments before turning off Environment.",
        ),
        (
            Role::Agent,
            !current.agents.is_empty(),
            "Remove its registered agent profiles before turning off Agent.",
        ),
        (
            Role::Provider,
            !current.providers.is_empty(),
            "Remove its registered providers before turning off Provider.",
        ),
    ] {
        if used && row.roles.iter().any(|name| name == role_name(role)) && !roles.contains(&role) {
            return Err(ApiError::invalid("roles", detail));
        }
    }
    conn.commit().await.map_err(DbError::from)?;
    let answer: orch_link_contract::commands::MachineRolesSetResult = ctx
        .app
        .link
        .dispatcher()
        .run(id, &SerialKey::Machine, &body)
        .await
        .map_err(|failure| match failure {
            RunError::Command(CommandFailure::Failed(refusal)) => {
                ApiError::invalid("roles", &refusal.detail)
            }
            other => command_failed(id, other),
        })?;
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    require_machine(&mut tx, guard.principal(), id, Permission::MachineManage).await?;
    let who = actor_name(&ctx)?;
    audit_machine(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MachineRolesChanged),
        id,
        &format!("{who} changed the roles of {}", row.name),
        json!({"roles":&answer.roles,"reconnecting":answer.reconnecting}),
    )
    .await?;
    let row = rows::machine(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let machine_view = machine_with_readiness(&ctx, &mut tx, row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(api::MachineRolesChange {
        machine_view,
        requested_roles: answer.roles,
        reconnecting: answer.reconnecting,
    })
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Environment => "environment",
        Role::Agent => "agent",
        Role::Provider => "provider",
        Role::Node => "node",
    }
}
