//! Native continuation commands keep their exact accepted operation through queue/send waits.
use super::{
    admission, operation,
    recipe::{Lane, Recipe},
};
use crate::{EngineError, commands::Outgoing, unit::Unit};
use orch_authz::Caller;
use orch_db::{
    Conn,
    queries::{
        jobs,
        link::{CommandRow, NewCommand},
        threads,
    },
};
use orch_link_contract::commands::{CommandKind, SerialKey};
use orch_types::{Clock, Hash, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    job: Id,
    command: Id,
    machine: Id,
    slot: String,
    kind: CommandKind,
    body: Hash,
    deadline: Time,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContinuationAuthority([private])")
    }
}
impl Authority {
    pub(crate) fn capture(
        recipe: &Recipe,
        slot: &str,
        command: &Outgoing,
        deadline: Time,
        plan: &serde_json::Value,
    ) -> Result<Self, EngineError> {
        if recipe.operation_origin.is_none()
            || !matches!(
                command.kind,
                CommandKind::SessionPrefixInspect
                    | CommandKind::SessionPrefixOpen
                    | CommandKind::SessionContinuationResume
                    | CommandKind::SessionContinuationClose
            )
            || command.key != SerialKey::Thread(recipe.thread_id)
        {
            return Err(EngineError::unreadable(
                "the native continuation command has another origin or kind",
            ));
        }
        let value = Self {
            job: recipe.operation_id,
            command: command.id,
            machine: command.machine,
            slot: slot.into(),
            kind: command.kind,
            // PostgreSQL keeps an object's members in its own order, so the body is identified
            // without regard to member order, as every other stored command body is.
            body: crate::jobs::command_identity::hash(&command.body)?,
            deadline,
        };
        if value.lane(recipe, plan).is_none() {
            return Err(EngineError::unreadable(
                "the native continuation command changed its phase identity",
            ));
        }
        Ok(value)
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if let Some(thread) = jobs::job(conn, self.job).await?.and_then(|row| row.node_id) {
            threads::lock(conn, thread).await?;
            jobs::lock(conn, self.job).await?;
        }
        Ok(())
    }
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    fn lane<'a>(&self, recipe: &'a Recipe, plan: &serde_json::Value) -> Option<&'a Lane> {
        let expected = match self.slot.as_str() {
            "preinspect" => recipe.commands.preinspect,
            "inspect" => recipe.commands.inspect,
            "close_active" => recipe.commands.close_active,
            "resume_source" => recipe.commands.resume_source,
            "close_source" => recipe.commands.close_source,
            "open_prefix" => recipe.commands.open_prefix,
            "resume_target" => recipe.commands.resume_target,
            "close_candidate" => recipe.commands.close_candidate,
            "restore_active" => {
                let attempt = plan
                    .get("restore_attempt")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if attempt == 0 {
                    recipe.commands.restore_active
                } else {
                    crate::ids::derived(
                        "continuation.restore-retry",
                        &[
                            recipe.commands.restore_active.as_bytes(),
                            &attempt.to_be_bytes(),
                        ],
                    )
                }
            }
            _ => return None,
        };
        if self.command != expected {
            return None;
        }
        let lane = if matches!(self.slot.as_str(), "close_active" | "restore_active") {
            recipe.original_lane.as_ref()?
        } else {
            &recipe.source_lane
        };
        (lane.machine_id == self.machine).then_some(lane)
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            command.id,
            command.machine_id,
            command.kind,
            command.serial_key,
            command.body,
            Time::new(command.deadline_at),
            clock,
            access,
        )
        .await
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            command.id,
            command.machine_id,
            &command.kind,
            command.serial_key(),
            &command.body,
            Time::new(command.deadline_at),
            clock,
            access,
        )
        .await
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "the existing proposed and retained native envelopes share one exact verifier"
    )]
    async fn current(
        &self,
        conn: &mut Conn,
        id: Id,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &serde_json::Value,
        deadline: Time,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if id != self.command
            || machine != self.machine
            || kind != self.kind.as_str()
            || deadline != self.deadline
            || crate::jobs::command_identity::hash(body)? != self.body
        {
            return Ok(None);
        }
        let Some(peek) = jobs::job(conn, self.job).await? else {
            return Ok(None);
        };
        let Some(thread_id) = peek.node_id else {
            return Ok(None);
        };
        let Some(thread) = threads::lock(conn, thread_id).await? else {
            return Ok(None);
        };
        let Some(owner) = jobs::lock(conn, self.job).await? else {
            return Ok(None);
        };
        if owner.kind != orch_db::queries::thread_continuations::JOB_KIND {
            return Ok(None);
        }
        let recipe: Recipe = serde_json::from_value(owner.plan["recipe"].clone())?;
        if recipe.operation_origin.is_none()
            || recipe.operation_id != self.job
            || owner.node_id != Some(recipe.thread_id)
            || key != SerialKey::Thread(recipe.thread_id).wire().as_deref()
        {
            return Ok(None);
        }
        let Some(lane) = self.lane(&recipe, &owner.plan) else {
            return Ok(None);
        };
        if thread.node_id != recipe.thread_id {
            return Ok(None);
        }
        if thread.active_continuation_id != recipe.request.expected.active_continuation_id
            || thread.execution_revision != recipe.request.expected.execution_revision
            || thread.agent_id != Some(lane.agent_id)
            || thread.provider_id != Some(lane.provider_id)
            || thread.environment_id != Some(lane.environment_id)
            || !admission::generation_current(conn, lane).await?
        {
            return Ok(None);
        }
        let cleanup = matches!(self.slot.as_str(), "restore_active" | "close_candidate");
        if cleanup {
            // This exact owned recovery never admits new authored work or revives a revoked key.
            if owner
                .plan
                .get("committed")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
                || owner.plan.get("continuation_retirement").is_some()
            {
                return Ok(None);
            }
            if self.slot == "restore_active"
                && owner
                    .plan
                    .get("native_changed")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
            {
                return Ok(None);
            }
            if clock.time() >= deadline {
                return Ok(None);
            }
            return Ok(Some(vec![(None, Some(deadline))]));
        }
        if !matches!(owner.state.as_str(), "queued" | "running")
            || owner.cancel_requested()
            || clock.time() >= recipe.deadline
            || clock.time() >= deadline
        {
            return Ok(None);
        }
        let principal = match operation::principal(conn, clock, &recipe).await {
            Ok(Some(principal)) => principal,
            Ok(None) => return Ok(None),
            Err(EngineError::BeforeEffect(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut unit = Unit::new(conn, clock.now());
        match crate::inputs::check_sender_for_turn(
            &mut unit,
            principal.caller(),
            &thread,
            true,
            None,
            access,
        )
        .await
        {
            Ok(()) => {}
            Err(crate::inputs::InputError::Internal(error)) => return Err(error),
            Err(_) => return Ok(None),
        }
        if !principal.caller().key_deadline_allows(clock) {
            return Ok(None);
        }
        let caller = principal.caller().clone();
        let end = caller
            .grant()
            .and_then(orch_authz::TokenGrant::expires_at)
            .map_or(deadline, |end| end.min(deadline));
        Ok(Some(vec![(Some(caller), Some(end))]))
    }
}
