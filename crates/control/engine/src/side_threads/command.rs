//! Canonical side-history commands retain their real source through queue and native delivery.
use super::*;
use crate::commands::{CommandAuthority, Outgoing};
use orch_db::{
    Conn,
    queries::{
        link::{self, CommandRow, NewCommand},
        thread_history_cuts as cuts,
    },
};
use orch_link_contract::commands::{CommandKind, SerialKey};
use orch_types::{Clock, Hash};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    side: Id,
    segment: Id,
    turn: Id,
    revision: i64,
    slot: String,
    command: Id,
    machine: Id,
    kind: CommandKind,
    key: Option<String>,
    body: Hash,
    deadline: Time,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SideHistoryAuthority([private])")
    }
}
impl Authority {
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        threads::lock(conn, self.side).await?;
        Ok(())
    }
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    fn capture(
        side: &threads::ThreadRow,
        slot: &str,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if !matches!(
            slot,
            "side_source_record"
                | "side_verify_child"
                | "side_open_fork"
                | "side_open_neutral"
                | "side_open_empty"
        ) {
            return Err(EngineError::unreadable(
                "The side history command has no native phase.",
            ));
        }
        Ok(Self {
            side: side.node_id,
            segment: side
                .current_segment_id
                .ok_or_else(|| EngineError::unreadable("The side has no segment."))?,
            turn: side
                .current_turn_id
                .ok_or_else(|| EngineError::unreadable("The side has no starting turn."))?,
            revision: side.execution_revision,
            slot: slot.into(),
            command: command.id,
            machine: command.machine,
            kind: command.kind,
            key: command.key.wire(),
            body: Hash::of(&serde_json::to_vec(&command.body)?),
            deadline: command.deadline.ok_or_else(|| {
                EngineError::unreadable("The side command lost its native budget.")
            })?,
        })
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
        reason = "the proposed and stored native envelopes share one verifier"
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
        if self.command != id
            || self.machine != machine
            || self.kind.as_str() != kind
            || self.key.as_deref() != key
            || self.body != Hash::of(&serde_json::to_vec(body)?)
            || self.deadline != deadline
            || clock.time() >= deadline
        {
            return Ok(None);
        }
        let Some(side) = threads::lock(conn, self.side)
            .await?
            .filter(|row| row.deleted_at.is_none() && row.archived_at.is_none())
        else {
            return Ok(None);
        };
        if side.current_segment_id != Some(self.segment)
            || side.current_turn_id != Some(self.turn)
            || side.execution_revision != self.revision
        {
            return Ok(None);
        }
        let Some((_, cut, auth)) = operation::original(conn, self.side).await? else {
            return Ok(None);
        };
        if auth.operation_origin.is_none()
            || auth.segment != self.segment
            || auth.continuation != side.active_continuation_id
        {
            return Ok(None);
        }
        let Some(saved) = cut.artifacts.get(&self.slot) else {
            return Ok(None);
        };
        let original: Outgoing = serde_json::from_value(saved.clone())?;
        if original.id != id
            || original.machine != machine
            || original.body != *body
            || original.kind.as_str() != kind
            || original.key.wire().as_deref() != key
            || original.deadline != Some(deadline)
            || original.authority != Some(CommandAuthority::SideHistory(Box::new(self.clone())))
        {
            return Ok(None);
        }
        let lane = threads::segment(conn, self.segment)
            .await?
            .ok_or_else(|| EngineError::unreadable("The side native lane is unavailable."))?;
        let generation: Option<i32> = sqlx::query_scalar(
            "SELECT generation FROM environments WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(lane.environment_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if lane.thread_id != self.side
            || lane.continuation_id != auth.continuation
            || side.agent_id != Some(lane.agent_id)
            || side.provider_id != Some(lane.provider_id)
            || side.environment_id != Some(lane.environment_id)
            || generation != Some(lane.env_generation)
        {
            return Ok(None);
        }
        let valid = match self.slot.as_str() {
            "side_source_record" => {
                id == crate::ids::derived("side.native-record", &[cut.id.as_bytes()])
                    && self.kind == CommandKind::AgentReadSession
                    && key
                        == (SerialKey::Harness {
                            provider_id: lane.provider_id,
                            agent_id: lane.agent_id,
                        })
                        .wire()
                        .as_deref()
            }
            "side_verify_child" => {
                let Some(created) = cut.artifacts.get("side_open_fork") else {
                    return Ok(None);
                };
                let created: Outgoing = serde_json::from_value(created.clone())?;
                id == crate::ids::derived("side.verify-native-child", &[created.id.as_bytes()])
                    && self.kind == CommandKind::AgentReadSession
                    && key
                        == (SerialKey::Harness {
                            provider_id: lane.provider_id,
                            agent_id: lane.agent_id,
                        })
                        .wire()
                        .as_deref()
            }
            "side_open_fork" | "side_open_neutral" | "side_open_empty" => {
                id == crate::ids::derived(&self.slot, &[self.side.as_bytes()])
                    && self.kind == CommandKind::SessionOpenFenced
                    && key == SerialKey::Thread(self.side).wire().as_deref()
            }
            _ => false,
        };
        if !valid {
            return Ok(None);
        }
        let principal = match operation::principal(conn, clock, self.side).await {
            Ok(Some(principal)) => principal,
            Ok(None) | Err(EngineError::BeforeEffect(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut unit = Unit::new(conn, clock.now());
        if let Err(error) = crate::inputs::check_sender_for_turn(
            &mut unit,
            principal.caller(),
            &side,
            true,
            None,
            access,
        )
        .await
        {
            return match error {
                InputError::Internal(error) => Err(error),
                _ => Ok(None),
            };
        }
        if !principal.caller().key_deadline_allows(clock) || clock.time() >= deadline {
            return Ok(None);
        }
        Ok(Some(vec![(
            Some(principal.caller().clone()),
            Some(deadline),
        )]))
    }
}
/// Shared native history producers retain their exact bytes and private original source atomically.
pub(super) async fn retain(
    engine: &Engine,
    conn: &mut Conn,
    side: &threads::ThreadRow,
    cut: &cuts::Cut,
    slot: &str,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    let auth: history::Authorization =
        serde_json::from_value(cut.artifacts["side_authorization"].clone())?;
    if auth.operation_origin.is_none() {
        return Ok(command);
    }
    if let Some(saved) = cut.artifacts.get(slot) {
        let old: Outgoing = serde_json::from_value(saved.clone())?;
        if old.id != command.id
            || old.machine != command.machine
            || old.kind != command.kind
            || old.key != command.key
            || old.body != command.body
        {
            return Err(EngineError::unreadable(
                "The original side command changed its native identity.",
            ));
        }
        command = old;
    } else {
        command.deadline.get_or_insert_with(|| {
            Time::new(
                engine.inner.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
        let authority = Authority::capture(side, slot, &command)?;
        command = command.with_authority(CommandAuthority::SideHistory(Box::new(authority)));
        // Admission precedes this canonical journal effect; actual queue insertion checks again below.
        let principal = operation::principal(conn, engine.inner.clock.as_ref(), side.node_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("The side's operation ended."))?;
        if !principal
            .caller()
            .key_deadline_allows(engine.inner.clock.as_ref())
        {
            return Err(EngineError::unreadable("The side's credential ended."));
        }
        if !cuts::keep_artifact(conn, cut.id, slot, &serde_json::to_value(&command)?, &[]).await? {
            return Err(EngineError::unreadable(
                "Another native command owns this side phase.",
            ));
        }
    }
    let authority = command.authority.as_ref().ok_or_else(|| {
        EngineError::unreadable("The known side operation lost its private native origin.")
    })?;
    let private = serde_json::to_value(authority)?;
    let key = command.key.wire();
    let deadline = command
        .deadline
        .ok_or_else(|| EngineError::unreadable("The side command lost its native budget."))?;
    let proposed = NewCommand {
        id: command.id,
        machine_id: command.machine,
        kind: command.kind.as_str(),
        body: &command.body,
        serial_key: key.as_deref(),
        deadline_at: deadline.get(),
    };
    let current = authority
        .current_issue_callers(
            conn,
            &proposed,
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
        )
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("The side source ended before native queue insertion.")
        })?;
    if current.iter().any(|(caller, end)| {
        caller
            .as_ref()
            .is_some_and(|caller| !caller.key_deadline_allows(engine.inner.clock.as_ref()))
            || end.is_some_and(|end| engine.inner.clock.time() >= end)
    }) {
        return Err(EngineError::unreadable(
            "The side source ended before native queue insertion.",
        ));
    }
    link::insert_command_with_authority(conn, &proposed, engine.inner.clock.now(), Some(&private))
        .await?;
    let retained = link::command_with_authority(conn, command.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The native side command is unavailable."))?;
    let row = &retained.command;
    if retained.authority.as_ref() != Some(&private)
        || row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.body != command.body
        || row.serial_key() != key.as_deref()
        || Time::new(row.deadline_at) != deadline
    {
        return Err(EngineError::unreadable(
            "The retained side command has another body, budget or private origin.",
        ));
    }
    Ok(command)
}
/// Called by the common cut reader for the side's actual source-record phase only.
pub(crate) async fn source_record(
    engine: &Engine,
    cut: &cuts::Cut,
    command: Outgoing,
) -> Result<Outgoing, EngineError> {
    if cut.artifacts.get("side_authorization").is_none() {
        return Ok(command);
    }
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let side = threads::lock(&mut tx, cut.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The side source is unavailable."))?;
    let fresh = history::authorize(engine, &mut tx, &side, engine.inner.clock.now()).await?;
    let command = retain(
        engine,
        &mut tx,
        &side,
        &fresh,
        "side_source_record",
        command,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(command)
}
