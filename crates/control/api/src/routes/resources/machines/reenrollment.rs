//! Issuing a machine's replacement enrollment token to its owner or the managers of its host.

use super::{Installer, enrollment_commands, require_machine};
use crate::certificates::{TokenFor, TokenRequest};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::resources::{EnrollmentToken, MachineEnrollmentAgainTokenCreate};
use orch_authz::Policy;
use orch_db::queries::resources::{self, reenrollment};

pub(super) fn mount(registry: &mut Registry) {
    registry.on(MachineEnrollmentAgainTokenCreate, create);
}

async fn create(ctx: Ctx, (): (), (): ()) -> ApiResult<EnrollmentToken> {
    let machine = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let row = resources::machine(&mut conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    if row.lost_at.is_some() {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::PlanBlocked,
            "A machine declared lost needs a new enrollment and identity",
        ));
    }
    // A host's machine is enrolled again by its node, every other machine by its connector.
    let host = reenrollment::host_of(&mut conn, machine).await?;
    let installer = if host.is_some() {
        Installer::Node
    } else {
        Installer::Connector
    };
    if let Some(host) = host {
        let principal = authorization::principal(&ctx, &mut conn).await?;
        authorization::require_resource(
            &mut conn,
            &principal,
            ResourceKind::Host,
            host,
            Permission::HostManage,
            Policy::default(),
        )
        .await?;
    } else {
        // A person's machine is enrolled again by one of its owners alone: who manages it, an
        // admin's reach included, does not hand its identity on.
        let principal = authorization::principal(&ctx, &mut conn).await?;
        require_machine(&mut conn, &principal, row.id, Permission::AccessOwn).await?;
    }
    match installer {
        Installer::Node => super::require_node_installable(&ctx.app.config)?,
        Installer::Connector => super::require_installable(&ctx.app.config)?,
    }
    drop(conn);
    let made = ctx
        .app
        .link
        .certificates()
        .issue_token(&TokenRequest {
            created_by: ctx.user()?.id(),
            purpose: TokenFor::Again { machine },
            name_hint: None,
        })
        .await
        .map_err(|fault| ApiError::internal(format!("{}: {}", fault.code, fault.detail)))?;
    let mut conn = ctx.app.db.acquire().await?;
    if !crate::certificates::bind_issuer(&mut conn, &made.receipt, ctx.user()?).await? {
        return Err(ApiError::internal(
            "the enrollment issuer could not be recorded",
        ));
    }
    drop(conn);
    let mut commands = enrollment_commands(&ctx.app.config, &made.token, installer);
    if matches!(installer, Installer::Connector) {
        let roles = row
            .roles
            .iter()
            .filter(|role| role.as_str() != "node")
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        commands
            .linux
            .push_str(&format!(" --roles {}", super::sh_word(&roles)));
        commands
            .macos
            .push_str(&format!(" --roles {}", super::sh_word(&roles)));
        commands
            .windows
            .push_str(&format!(" -Roles {}", super::ps_word(&roles)));
    }
    // Retained and restored installations must address the same directory, including named instances.
    if let Some(directory) = row
        .facts
        .pointer("/installation/data_dir")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let posix = format!(" --data-dir {}", super::sh_word(directory));
        commands.linux.push_str(&posix);
        if matches!(installer, Installer::Connector) {
            commands.macos.push_str(&posix);
            commands
                .windows
                .push_str(&format!(" -DataDir {}", super::ps_word(directory)));
        }
    }
    if matches!(installer, Installer::Node)
        && let Some(path) = row
            .facts
            .pointer("/installation/config_file")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    {
        commands
            .linux
            .push_str(&format!(" --config {}", super::sh_word(path)));
    }
    if row.facts.pointer("/installation/service") == Some(&serde_json::Value::Null)
        && let (Some(executable), Some(directory)) = (
            row.facts
                .pointer("/installation/executable")
                .and_then(serde_json::Value::as_str),
            row.facts
                .pointer("/installation/data_dir")
                .and_then(serde_json::Value::as_str),
        )
    {
        let mut start = format!(
            "{} --data-dir {}",
            super::sh_word(executable),
            super::sh_word(directory)
        );
        if matches!(installer, Installer::Node)
            && let Some(path) = row
                .facts
                .pointer("/installation/config_file")
                .and_then(serde_json::Value::as_str)
        {
            start.push_str(&format!(" --config {}", super::sh_word(path)));
        }
        commands
            .linux
            .push_str(&format!(" --no-service && {start} run"));
        if matches!(installer, Installer::Connector) {
            commands
                .macos
                .push_str(&format!(" --no-service && {start} run"));
            commands.windows.push_str(&format!(
                " -NoService; if ($?) {{ & {} --data-dir {} run }}",
                super::ps_word(executable),
                super::ps_word(directory)
            ));
        }
    }
    Ok(EnrollmentToken {
        receipt: Some(made.receipt),
        commands,
        token: made.token,
        expires_at: made.expires_at,
    })
}
