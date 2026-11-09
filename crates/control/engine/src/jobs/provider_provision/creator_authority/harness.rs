//! The original first-Make family supplies the existing native harness startup seams.
use super::*;
use crate::commands::{CommandAuthority, Outgoing};
use crate::harnesses::{
    HarnessKey,
    preparation::{self, Target},
};
use futures::future::BoxFuture;
use orch_db::queries::link::{CommandRow, NewCommand};
use orch_link_contract::commands::CommandKind;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct Source {
    pub(super) job: Id,
    pub(super) origin: CapturedOperation,
    pub(super) until: Option<Time>,
}
/// Exact native startup evidence for this one retained owner creation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    job: Id,
    provider: Id,
    profile: Id,
    machine: Id,
    id: Id,
    kind: CommandKind,
    key: Option<String>,
    body: Hash,
    origin: CapturedOperation,
    until: Option<Time>,
}
struct Native<'a> {
    id: Id,
    machine: Id,
    kind: &'a str,
    serial: Option<&'a str>,
    body: &'a Json,
}

fn closed() -> EngineError {
    EngineError::unreadable("the original provider Move no longer permits this native startup")
}
async fn current(
    conn: &mut Conn,
    job: Id,
    clock: &dyn orch_types::Clock,
    key: HarnessKey,
) -> Result<(Principal, Option<Time>), EngineError> {
    let row = jobs::lock(conn, job).await?.ok_or_else(closed)?;
    let authority = read(&row)?.ok_or_else(closed)?;
    let copy = authority
        .duplicate
        .as_ref()
        .filter(|copy| copy.moving.is_some())
        .ok_or_else(closed)?;
    let principal = principal(conn, &row, clock).await?.ok_or_else(closed)?;
    let source = rows::state(conn, copy.source).await?.ok_or_else(closed)?;
    let target = rows::live_runtime(conn, authority.provider).await?;
    let allowed = key.agent == authority.agent.ok_or_else(closed)?
        && (key.provider == copy.source
            && source.key_machine_id.or(source.machine_id) == Some(key.machine)
            || key.provider == authority.provider
                && target.as_ref().and_then(|runtime| runtime.machine_id) == Some(key.machine));
    if !allowed {
        return Err(closed());
    }
    let latest = jobs::job(conn, job)
        .await?
        .and_then(|row| read(&row).ok().flatten())
        .and_then(|authority| authority.duplicate)
        .ok_or_else(closed)?;
    Ok((principal, latest.raw.until()))
}
fn key(target: Target) -> Result<HarnessKey, EngineError> {
    match target {
        Target::Harness(key) => Ok(key),
        _ => Err(closed()),
    }
}
impl preparation::Source for Source {
    fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    fn until(&self) -> Option<Time> {
        self.until
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn preparation::Source>, EngineError> {
        if origin.user() != self.origin.user() || origin.operation() != self.origin.operation() {
            return Err(closed());
        }
        Ok(Arc::new(Self {
            job: self.job,
            origin: self
                .origin
                .intersect(&origin)
                .map_err(EngineError::unreadable)?,
            until: self.until.into_iter().chain(until).min(),
        }))
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn orch_types::Clock,
        target: Target,
    ) -> BoxFuture<'a, Result<preparation::Checkpoint, EngineError>> {
        Box::pin(async move {
            let (principal, until) = current(conn, self.job, clock, key(target)?).await?;
            let accepted = self
                .origin
                .restore_current(conn, clock)
                .await?
                .ok_or_else(closed)?;
            let caller = principal
                .caller()
                .narrow_current_admission(&accepted)
                .map_err(EngineError::unreadable)?;
            let retained = self.until;
            let until = until.into_iter().chain(retained).min();
            Ok(preparation::Checkpoint::from_check(
                caller,
                until,
                Box::new(move |clock| {
                    if retained.is_some_and(|until| clock.time() >= until) {
                        Err(closed())
                    } else {
                        Ok(())
                    }
                }),
            ))
        })
    }
    fn command<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn orch_types::Clock,
        target: Target,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>> {
        Box::pin(async move {
            let target = key(target)?;
            current(conn, self.job, clock, target).await?;
            if !physical(target, command.machine, command.kind, &command.body)? {
                return Err(closed());
            }
            Ok(CommandAuthority::ProviderCreator(Box::new(Command {
                job: self.job,
                provider: target.provider,
                profile: target.agent,
                machine: target.machine,
                id: command.id,
                kind: command.kind,
                key: command.key.wire(),
                body: body_hash(&command.body)?,
                origin: self.origin.clone(),
                until: self.until,
            })))
        })
    }
}
fn physical(
    key: HarnessKey,
    machine: Id,
    kind: CommandKind,
    body: &Json,
) -> Result<bool, EngineError> {
    if key.machine != machine {
        return Ok(false);
    }
    Ok(match kind {
        CommandKind::AgentStart => {
            let body: orch_link_contract::commands::AgentStart =
                serde_json::from_value(body.clone())?;
            body.provider_id == key.provider
                && body.agent_id == key.agent
                && body.provider_kind
                    == Some(orch_agent_contract::descriptor::ProviderKind::Subscription)
        }
        CommandKind::UnitAfterUp => {
            body.get("provider_id") == Some(&json!(key.provider))
                && body.get("agent_id") == Some(&json!(key.agent))
                && body.get("environment_id").is_none_or(Json::is_null)
        }
        CommandKind::AgentStop => {
            body.get("provider_id") == Some(&json!(key.provider))
                && body.get("agent_id") == Some(&json!(key.agent))
                && body.get("logout") != Some(&json!(true))
        }
        _ => false,
    })
}
impl Command {
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        jobs::lock(conn, self.job).await?.ok_or_else(closed)?;
        Ok(())
    }
    async fn callers(
        &self,
        conn: &mut Conn,
        native: Native<'_>,
        clock: &dyn orch_types::Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Native {
            id,
            machine,
            kind,
            serial,
            body,
        } = native;
        if id != self.id
            || machine != self.machine
            || kind != self.kind.as_str()
            || serial != self.key.as_deref()
            || body_hash(body)? != self.body
            || !physical(
                HarnessKey {
                    provider: self.provider,
                    agent: self.profile,
                    machine: self.machine,
                },
                machine,
                self.kind,
                body,
            )?
        {
            return Ok(None);
        }
        let result = current(
            conn,
            self.job,
            clock,
            HarnessKey {
                provider: self.provider,
                agent: self.profile,
                machine: self.machine,
            },
        )
        .await;
        match result {
            Ok((principal, until)) => {
                if self.origin.operation() != principal.caller().admitted_operation().unwrap_or("")
                {
                    return Ok(None);
                }
                let Some(accepted) = self.origin.restore_current(conn, clock).await? else {
                    return Ok(None);
                };
                let caller = principal
                    .caller()
                    .narrow_current_admission(&accepted)
                    .map_err(EngineError::unreadable)?;
                let until = until.into_iter().chain(self.until).min();
                if !caller.key_deadline_allows(clock)
                    || until.is_some_and(|until| clock.time() >= until)
                {
                    return Ok(None);
                }
                Ok(Some(vec![(Some(caller), until)]))
            }
            Err(EngineError::Unreadable(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn orch_types::Clock,
        _: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.callers(
            conn,
            Native {
                id: command.id,
                machine: command.machine_id,
                kind: command.kind,
                serial: command.serial_key,
                body: command.body,
            },
            clock,
        )
        .await
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn orch_types::Clock,
        _: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.callers(
            conn,
            Native {
                id: command.id,
                machine: command.machine_id,
                kind: &command.kind,
                serial: command.serial_key(),
                body: &command.body,
            },
            clock,
        )
        .await
    }
}
pub(super) async fn start(job: &Job, provider: Id, machine: Id) -> Result<(), Stop> {
    let mut conn = job.engine().db().acquire().await?;
    let row = job.row().await?;
    let authority = read(&row)?.ok_or_else(closed)?;
    let copy = authority
        .duplicate
        .as_ref()
        .filter(|copy| copy.moving.is_some())
        .ok_or_else(closed)?;
    let agent = authority.agent.ok_or_else(closed)?;
    let profile = orch_db::queries::resources::thread_engine::agent(&mut conn, agent)
        .await?
        .ok_or_else(closed)?;
    let preparation = preparation::Preparation::new(Source {
        job: row.id,
        origin: authority.origin.clone(),
        until: copy.raw.until(),
    });
    drop(conn);
    let key = HarnessKey {
        provider,
        agent,
        machine,
    };
    match job
        .engine()
        .ensure_harness_with_preparation(
            key,
            &profile,
            ids::derived(
                "provider.move.harness",
                &[job.id().as_bytes(), provider.as_bytes()],
            ),
            Some(&preparation),
        )
        .await?
    {
        crate::harnesses::Started::Up => Ok(()),
        crate::harnesses::Started::Failed(_) => {
            Err(EngineError::unreadable("the native sign-in keeper could not start").into())
        }
    }
}
