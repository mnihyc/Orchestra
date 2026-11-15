//! The accepted provider placement remains conjunctive with its original native command source.
use super::*;
use crate::commands::{CommandAuthority, Outgoing};
use futures::future::BoxFuture;
use orch_db::queries::link::{CommandRow, NewCommand};
use orch_link_contract::commands::CommandKind;
use orch_types::{Clock, Hash};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    job: Id,
    provider: Id,
    runtime: Id,
    host: Id,
    actor: Id,
    id: Id,
    machine: Id,
    key: Option<String>,
    kind: CommandKind,
    body: Hash,
    native: Option<Box<CommandAuthority>>,
}
type CurrentCallers = Vec<(Option<Caller>, Option<Time>)>;

impl Command {
    async fn base_callers(
        &self,
        conn: &mut orch_db::Conn,
        body: &Json,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let row = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .ok_or_else(|| EngineError::unreadable("the provider placement source is gone"))?;
        if creator_authority::is_retained(&row) {
            return Ok(creator_authority::command_current_callers(
                conn,
                self.id,
                self.machine,
                self.key.as_deref(),
                self.kind.as_str(),
                body,
                clock,
            )
            .await?
            .filter(|callers| !callers.is_empty()));
        }
        if crate::jobs::lifecycle_commands::applies(&row)
            || crate::jobs::recreate::batch_authority::applies(&row)
            || row.plan.get("provider_recovery").is_some()
        {
            return Ok(None);
        }
        Ok(Some(vec![(
            Some(Caller::session(self.actor).with_admin_mode(false)),
            None,
        )]))
    }
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub(crate) fn lock_source<'a>(
        &'a self,
        conn: &'a mut orch_db::Conn,
    ) -> BoxFuture<'a, Result<(), EngineError>> {
        Box::pin(async move {
            if let Some(native) = &self.native {
                Box::pin(native.lock_native_source(conn)).await?;
            }
            orch_db::queries::jobs::lock(conn, self.job)
                .await?
                .ok_or_else(|| EngineError::unreadable("the provider placement job is gone"))?;
            Ok(())
        })
    }
    fn matches(
        &self,
        id: Id,
        machine: Id,
        key: Option<&str>,
        kind: &str,
        body: &Json,
    ) -> Result<bool, EngineError> {
        Ok(id == self.id
            && machine == self.machine
            && key == self.key.as_deref()
            && kind == self.kind.as_str()
            && creator_authority::body_hash(body)? == self.body)
    }
    async fn permits(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        clock: &dyn Clock,
    ) -> Result<bool, EngineError> {
        let Some(row) = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .filter(|row| {
                row.is_open()
                    && !row.cancel_requested()
                    && row.kind == provider_jobs::PROVISION
                    && row.provider_id == Some(self.provider)
                    && row.plan.get("runtime") == Some(&json!(self.runtime))
                    && row.plan.get("host") == Some(&json!(self.host))
            })
        else {
            return Ok(false);
        };
        let Some(provider) = rows::state(conn, self.provider)
            .await?
            .filter(|provider| provider.deleted_at.is_none() && provider.owner_id == self.actor)
        else {
            return Ok(false);
        };
        let Some(runtime) = rows::live_runtime(conn, self.provider)
            .await?
            .filter(|runtime| runtime.id == self.runtime && runtime.host_id == self.host)
        else {
            return Ok(false);
        };
        let Some(host) = host_rows::host(conn, self.host).await? else {
            return Ok(false);
        };
        if self.machine != host.machine_id && Some(self.machine) != runtime.machine_id {
            return Ok(false);
        }
        let Some(template) = host_rows::template(conn, runtime.template_id)
            .await?
            .filter(|template| template.enabled && template.deleted_at.is_none())
        else {
            return Ok(false);
        };
        let principal = Principal::load(conn, caller.clone(), clock.time()).await?;
        super::current(conn, &principal, &provider, &template, &row, self.host).await
    }
    pub(crate) fn current_issue_callers<'a>(
        &'a self,
        conn: &'a mut orch_db::Conn,
        command: &'a NewCommand<'a>,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            if !self.matches(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? {
                return Ok(None);
            }
            let callers = if let Some(native) = &self.native {
                let Some(callers) =
                    Box::pin(native.current_issue_callers(conn, command, clock, access)).await?
                else {
                    return Ok(None);
                };
                callers
            } else {
                let Some(callers) = self.base_callers(conn, command.body, clock).await? else {
                    return Ok(None);
                };
                callers
            };
            if callers.is_empty() {
                return Ok(None);
            }
            for (caller, _) in &callers {
                let Some(caller) = caller else {
                    return Ok(None);
                };
                if !self.permits(conn, caller, clock).await? {
                    return Ok(None);
                }
            }
            Ok(Some(callers))
        })
    }
    pub(crate) fn current_callers<'a>(
        &'a self,
        conn: &'a mut orch_db::Conn,
        command: &'a CommandRow,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            if !self.matches(
                command.id,
                command.machine_id,
                command.serial_key(),
                &command.kind,
                &command.body,
            )? {
                return Ok(None);
            }
            let callers = if let Some(native) = &self.native {
                let Some(callers) =
                    Box::pin(native.current_callers(conn, command, clock, access)).await?
                else {
                    return Ok(None);
                };
                callers
            } else {
                let Some(callers) = self.base_callers(conn, &command.body, clock).await? else {
                    return Ok(None);
                };
                callers
            };
            if callers.is_empty() {
                return Ok(None);
            }
            for (caller, _) in &callers {
                let Some(caller) = caller else {
                    return Ok(None);
                };
                if !self.permits(conn, caller, clock).await? {
                    return Ok(None);
                }
            }
            Ok(Some(callers))
        })
    }
}

pub(crate) async fn decorate(
    job: &Job,
    command: &mut Outgoing,
    respect_cancel: bool,
) -> Result<(), Stop> {
    if !respect_cancel
        || matches!(
            command.authority,
            Some(CommandAuthority::ProviderPlacement(_))
        )
    {
        return Ok(());
    }
    let owner = job.row().await?;
    if owner.kind != provider_jobs::PROVISION
        || owner.plan.get("pool_choice").is_none_or(Json::is_null)
    {
        return Ok(());
    }
    let provider = owner
        .provider_id
        .ok_or_else(|| failed("no_host", "the provider placement has no provider"))?;
    let runtime: Id =
        serde_json::from_value(owner.plan["runtime"].clone()).map_err(EngineError::from)?;
    let host: Id = serde_json::from_value(owner.plan["host"].clone()).map_err(EngineError::from)?;
    let mut conn = job.engine().db().acquire().await?;
    let actor = rows::state(&mut conn, provider)
        .await?
        .ok_or_else(|| failed("provider_gone", "the provider placement is gone"))?
        .owner_id;
    command.authority = Some(CommandAuthority::ProviderPlacement(Box::new(Command {
        job: owner.id,
        provider,
        runtime,
        host,
        actor,
        id: command.id,
        machine: command.machine,
        key: command.key.wire(),
        kind: command.kind,
        body: creator_authority::body_hash(&command.body).map_err(EngineError::from)?,
        native: command.authority.take().map(Box::new),
    })));
    Ok(())
}
