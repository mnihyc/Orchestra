//! Exact native planning suppliers retain the one managed act through queue/send waits.
use super::operation::{Origin, Preparation};
use crate::{
    Engine, EngineError,
    commands::{CommandAuthority, Outgoing},
};
use orch_authz::Caller;
use orch_db::{
    Conn,
    queries::link::{CommandRow, NewCommand},
};
use orch_link_contract::commands::CommandKind;
use orch_types::{Clock, Hash, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    origin: Origin,
    command: Id,
    machine: Id,
    kind: CommandKind,
    key: Option<String>,
    body: Hash,
    deadline: Time,
    measurement: Option<Measurement>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Measurement {
    job: Id,
    phase: String,
    origin: Hash,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ForkPreparation([private])")
    }
}
impl Authority {
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        orch_db::queries::transaction_guard::lock_native_actor(conn, self.origin.captured().user())
            .await?;
        orch_db::queries::threads::lock(conn, self.origin.input().thread()).await?;
        Ok(())
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if !self.matches(
            command.id,
            command.machine_id,
            command.kind,
            command.serial_key,
            command.body,
            Time::new(command.deadline_at),
        )? {
            return Ok(None);
        }
        self.current(conn, clock, access, command.body).await
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if !self.matches(
            command.id,
            command.machine_id,
            &command.kind,
            command.serial_key(),
            &command.body,
            Time::new(command.deadline_at),
        )? {
            return Ok(None);
        }
        self.current(conn, clock, access, &command.body).await
    }
    fn matches(
        &self,
        id: Id,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &serde_json::Value,
        deadline: Time,
    ) -> Result<bool, EngineError> {
        Ok(id == self.command
            && machine == self.machine
            && kind == self.kind.as_str()
            && key == self.key.as_deref()
            && Hash::of(&serde_json::to_vec(body)?) == self.body
            && deadline == self.deadline)
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
        body: &serde_json::Value,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if clock.time() >= self.deadline {
            return Ok(None);
        }
        if matches!(
            self.kind,
            CommandKind::NodeMountMeasure
                | CommandKind::NodeMountFingerprint
                | CommandKind::NodeRuntimeVolumesMeasure
        ) != self.measurement.is_some()
        {
            return Ok(None);
        }
        if let Some(measurement) = &self.measurement {
            if !measurement.current(conn, self, body).await? {
                return Ok(None);
            }
        }
        match self.origin.current_any(conn, clock, access).await {
            Ok((principal, until)) => {
                if !self
                    .origin
                    .input()
                    .supplier(conn, self.machine, self.kind, body, clock.now())
                    .await?
                {
                    return Ok(None);
                }
                self.origin.deadline(clock, principal.caller())?;
                Ok(Some(vec![(
                    Some(principal.caller().clone()),
                    Some(until.map_or(self.deadline, |until| until.min(self.deadline))),
                )]))
            }
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
impl Preparation {
    /// The actual native constructor supplies this immutable envelope; no body is rewritten.
    pub(crate) async fn command(
        &self,
        engine: &Engine,
        command: Outgoing,
    ) -> Result<Outgoing, EngineError> {
        self.capture_command(engine, command, None).await
    }
    pub(crate) async fn measurement_command(
        &self,
        engine: &Engine,
        job: Id,
        phase: &str,
        command: Outgoing,
    ) -> Result<Outgoing, EngineError> {
        self.capture_command(
            engine,
            command,
            Some(Measurement {
                job,
                phase: phase.to_owned(),
                origin: Hash::of(&serde_json::to_vec(&serde_json::to_value(self.origin())?)?),
            }),
        )
        .await
    }
    async fn capture_command(
        &self,
        engine: &Engine,
        mut command: Outgoing,
        measurement: Option<Measurement>,
    ) -> Result<Outgoing, EngineError> {
        if command.authority.is_some()
            || !matches!(
                command.kind,
                CommandKind::FsStat
                    | CommandKind::GitStatus
                    | CommandKind::FsManifest
                    | CommandKind::GitWorktreeCopyPlan
                    | CommandKind::NodeMountMeasure
                    | CommandKind::NodeMountFingerprint
                    | CommandKind::NodeVolumeCarryCheck
                    | CommandKind::NodeRuntimeVolumesMeasure
            )
        {
            return Err(EngineError::unreadable(
                "the native planning supplier has another origin or command kind",
            ));
        }
        if matches!(
            command.kind,
            CommandKind::NodeMountMeasure
                | CommandKind::NodeMountFingerprint
                | CommandKind::NodeRuntimeVolumesMeasure
        ) != measurement.is_some()
        {
            return Err(EngineError::unreadable(
                "the native measurement lost its job source",
            ));
        }
        let origin = self.accepted(engine).await?;
        let mut conn = engine.db().acquire().await?;
        if !origin
            .input()
            .supplier(
                &mut conn,
                command.machine,
                command.kind,
                &command.body,
                engine.inner.clock.now(),
            )
            .await?
        {
            return Err(EngineError::unreadable(
                "the native supplier has another physical source",
            ));
        }
        drop(conn);
        let deadline = *command.deadline.get_or_insert_with(|| {
            Time::new(
                engine.inner.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
        let authority = Authority {
            origin,
            command: command.id,
            machine: command.machine,
            kind: command.kind,
            key: command.key.wire(),
            body: Hash::of(&serde_json::to_vec(&command.body)?),
            deadline,
            measurement,
        };
        command.authority = Some(CommandAuthority::ForkPreparation(Box::new(authority)));
        Ok(command)
    }
}
impl Measurement {
    async fn current(
        &self,
        conn: &mut Conn,
        authority: &Authority,
        body: &serde_json::Value,
    ) -> Result<bool, EngineError> {
        use orch_link_contract::commands::{NodeMountFingerprint, WorkReport};
        let Some(row) = orch_db::queries::jobs::job(conn, self.job).await? else {
            return Ok(false);
        };
        let Some(origin) = row.plan.get("fork_preparation") else {
            return Ok(false);
        };
        if row.kind != crate::jobs::measurement::KIND
            || !row.is_open()
            || row.cancel_requested()
            || row.created_by != Some(authority.origin.captured().user())
            || authority.key.is_some()
            || Hash::of(&serde_json::to_vec(origin)?) != self.origin
            || row.plan["command_kind"].as_str() != Some(authority.kind.as_str())
        {
            return Ok(false);
        }
        let timeout_secs = row.plan["timeout_secs"]
            .as_u64()
            .ok_or_else(|| EngineError::unreadable("the native count lost its allowance"))?;
        let work = WorkReport {
            files_total: None,
            bytes_total: None,
            job_id: self.job,
            phase: self.phase.clone(),
            timeout_secs,
        };
        let (kind, machine, id, expected) = if authority.kind == CommandKind::NodeMountFingerprint {
            let input: crate::jobs::measurement::FingerprintInput =
                serde_json::from_value(row.plan["body"].clone())?;
            let (machine, mut selected): (Id, NodeMountFingerprint) = match self.phase.as_str() {
                "fingerprint_source" => (
                    serde_json::from_value(row.plan["machine"].clone())?,
                    input.source,
                ),
                "fingerprint_target" => (input.target_machine, input.target),
                _ => return Ok(false),
            };
            selected.work = Some(work);
            (
                CommandKind::NodeMountFingerprint.as_str(),
                machine,
                crate::ids::derived(&self.phase, &[self.job.as_bytes()]),
                serde_json::to_value(selected)?,
            )
        } else {
            if self.phase != "measure" {
                return Ok(false);
            }
            let Some(kind) = row.plan["command_kind"].as_str() else {
                return Ok(false);
            };
            let mut expected = row.plan["body"].clone();
            expected["work"] = serde_json::to_value(work)?;
            (
                kind,
                serde_json::from_value(row.plan["machine"].clone())?,
                crate::ids::derived("measurement", &[self.job.as_bytes()]),
                expected,
            )
        };
        Ok(authority.kind.as_str() == kind
            && authority.machine == machine
            && authority.command == id
            && &expected == body)
    }
}
