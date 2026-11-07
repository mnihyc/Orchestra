//! One native startup boundary, with typed live sources and durable exact command evidence.

use super::HarnessKey;
use crate::{
    Engine, EngineError,
    commands::{CommandAuthority, Outgoing},
    operation_scope::continuing,
};
use futures::future::BoxFuture;
use orch_authz::{Caller, CapturedOperation};
use orch_db::Conn;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The actual request transport's synchronous raw-credential boundary.
pub type BeforeEffect = Box<dyn FnOnce() -> Result<(), EngineError> + Send>;
type Check = Box<dyn FnOnce(&dyn Clock) -> Result<(), EngineError> + Send>;

/// The actual native placement selected by the existing startup writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The actual descriptor request, before the native selector has chosen a machine.
    Descriptor {
        provider: Id,
        agent: Id,
        environment: Option<Id>,
    },
    /// The actual credential keeper request, before its native start plan is selected.
    ProviderSelection { provider: Id },
    /// One provider/profile process on its selected machine.
    Harness(HarnessKey),
    /// One provider credential on its actual keeper.
    Provider { provider: Id, machine: Id },
}

/// An actual shared source frame, consumed at the native writer's first effect.
pub struct Checkpoint {
    caller: Caller,
    until: Option<Time>,
    before: Check,
}
/// A clock-only tail check; identity and source locks still belong to the same transaction.
#[derive(Clone)]
pub struct Deadline {
    caller: Caller,
    until: Option<Time>,
}
impl Deadline {
    /// Check after later native awaits and immediately before the guarded commit.
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        if !self.caller.key_deadline_allows(clock)
            || self
                .until
                .is_some_and(|end| clock.time().get() >= end.get())
        {
            return Err(ended());
        }
        Ok(())
    }
}
impl Checkpoint {
    /// Retain the real complete-operation proof and its actual raw-credential boundary.
    pub fn from_frame<S: continuing::Source>(
        frame: continuing::Frame<S>,
        until: Option<Time>,
        before: BeforeEffect,
    ) -> Self {
        let parts = frame.into_parts();
        Self::from_check(
            parts.principal.caller().clone(),
            until,
            Box::new(move |clock| {
                before()?;
                parts.first_effect.check(clock).map_err(source_error)
            }),
        )
    }

    /// Preparation facts remain preparation facts; this cannot produce a full act proof.
    pub fn from_preparation<S: continuing::Source>(
        frame: continuing::preparation::Frame<S>,
        until: Option<Time>,
        before: BeforeEffect,
    ) -> Self {
        let parts = frame.into_parts();
        Self::from_check(
            parts.principal.caller().clone(),
            until,
            Box::new(move |clock| {
                before()?;
                parts.first_effect.check(clock).map_err(source_error)
            }),
        )
    }

    pub(crate) fn from_check(caller: Caller, until: Option<Time>, before: Check) -> Self {
        Self {
            caller,
            until,
            before,
        }
    }

    /// The original/current key intersection produced by the shared factual source.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    /// The actual original/current raw-credential minimum.
    pub fn until(&self) -> Option<Time> {
        self.until
    }
    /// Retain only the accepted clocks for the rollbackable tail on this transaction.
    pub fn deadline(&self) -> Deadline {
        Deadline {
            caller: self.caller.clone(),
            until: self.until,
        }
    }
    /// Consume after all native waits, immediately before the effect on this transaction.
    pub fn check(self, clock: &dyn Clock) -> Result<(), EngineError> {
        self.deadline().check(clock)?;
        (self.before)(clock)
    }
}

fn source_error(error: continuing::Error) -> EngineError {
    match error {
        continuing::Error::Refused
        | continuing::Error::Binding(crate::operation_scope::BindingError::Refused) => ended(),
        continuing::Error::Native(error) => error,
        continuing::Error::Binding(error) => EngineError::unreadable(error),
    }
}
#[derive(Debug, thiserror::Error)]
#[error("the original native startup source no longer permits this work")]
/// A known native source closed; this is never an ordinary unavailable observation.
pub struct Ended;
fn ended() -> EngineError {
    EngineError::BeforeEffect(Box::new(Ended))
}

/// A domain composes its existing factual source/native selector with the actual startup operands.
/// This object is request-local. Recovery retains only the command channel's typed private evidence.
pub trait Source: Send + Sync + 'static {
    /// Exact original operation and all accepted key ceilings, never an endpoint alias.
    fn origin(&self) -> &CapturedOperation;
    /// The original/accepted raw credential deadline, separate from the captured key bounds.
    fn until(&self) -> Option<Time>;
    /// Restore the actual raw credential/current keys, lock and reload native sources, and run
    /// the existing ordinary/setup/placement gates on this supplied writer transaction.
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: Target,
    ) -> BoxFuture<'a, Result<Checkpoint, EngineError>>;
    /// Keep the selected native association, replacing only its accepted original/current minimum.
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn Source>, EngineError>;
    /// Produce exact native issue/send evidence for this real server-built command, on the
    /// writer's transaction. Its verifier must retain the raw credential and source association,
    /// reject substitutions and recheck unaccepted work through CommandAuthority after restart.
    /// No IO, independent transaction, cached admission or ordinary-work fallback belongs here.
    fn command<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: Target,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>>;
}

/// The one live carrier used by adoption, retained jobs and binding discovery alike.
#[derive(Clone)]
pub struct Preparation(Arc<dyn Source>);
impl Preparation {
    /// The native domain supplies its typed factual source, not an admitted boolean.
    pub fn new(source: impl Source) -> Self {
        Self(Arc::new(source))
    }
    pub(crate) async fn check_selection(
        &self,
        engine: &Engine,
        target: Target,
    ) -> Result<(), EngineError> {
        let mut tx = engine.inner.db.begin().await?;
        let check = self
            .checkpoint(&mut tx, engine.inner.clock.as_ref(), target)
            .await?;
        let deadline = check.deadline();
        check.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        deadline.check(engine.inner.clock.as_ref())
    }
    pub(crate) async fn checkpoint(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        target: Target,
    ) -> Result<Checkpoint, EngineError> {
        let mut check = self.0.checkpoint(conn, clock, target).await?;
        if check.caller().user() != self.0.origin().user()
            || check.caller().admitted_operation() != Some(self.0.origin().operation())
        {
            return Err(ended());
        }
        check.until = minimum(self.0.until(), check.until);
        Ok(check)
    }
    pub(crate) fn accepted_at(&self, check: &Checkpoint) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(check.caller()).map_err(EngineError::unreadable)?;
        let until = minimum(self.0.until(), check.until());
        let accepted = self.0.accepted(origin.clone(), until)?;
        if accepted.origin() != &origin || accepted.until() != until {
            return Err(ended());
        }
        Ok(Self(accepted))
    }
    pub(super) async fn evidence(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        target: Target,
        command: &Outgoing,
    ) -> Result<Evidence, EngineError> {
        if command.authority.is_some() {
            return Err(EngineError::unreadable(
                "startup cannot replace another command source",
            ));
        }
        let authority = self.0.command(conn, clock, target, command).await?;
        if !authority.requires_issue_checkpoint()
            || authority.command_kind() != command.kind.as_str()
        {
            return Err(ended());
        }
        Ok(Evidence::new(command, authority))
    }
    pub(crate) async fn command(
        &self,
        engine: &Engine,
        target: Target,
        mut command: Outgoing,
    ) -> Result<Outgoing, EngineError> {
        let mut tx = engine.inner.db.begin().await?;
        let check = self
            .checkpoint(&mut tx, engine.inner.clock.as_ref(), target)
            .await?;
        let accepted = self.accepted_at(&check)?;
        // A deterministic native ID may already have an accepted command. Preserve its
        // canonical evidence (including genuine old absence), never replace its owner.
        let _ = sqlx::query("SELECT id FROM commands WHERE id=$1 FOR SHARE")
            .bind(command.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        if let Some(row) = orch_db::queries::link::command(&mut tx, command.id).await? {
            if row.machine_id != command.machine
                || row.kind != command.kind.as_str()
                || row.serial_key() != command.key.wire().as_deref()
                || row.body != command.body
            {
                return Err(crate::commands::CommandsError::AuthorityMismatch.into());
            }
            command.authority = orch_db::queries::link::command_authority(&mut tx, row.id)
                .await?
                .map(serde_json::from_value)
                .transpose()?;
            check.check(engine.inner.clock.as_ref())?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return authorize_command(engine, command).await;
        }
        let evidence = accepted
            .evidence(&mut tx, engine.inner.clock.as_ref(), target, &command)
            .await?;
        check.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        command.authority = evidence.command.authority;
        authorize_command(engine, command).await
    }
}

fn minimum(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Stored on the existing private plan, never machine wire data or a serialized runtime object.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Stored {
    Commands(Commands),
    // Preserve already accepted plans and their original private format.
    Adoption(Box<crate::adoption::operation::Preparation>),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Commands {
    version: Version,
    commands: Vec<Evidence>,
}
#[derive(Clone, Serialize, Deserialize)]
enum Version {
    #[serde(rename = "native_startup_v1")]
    One,
}

/// Absence is old native work; present null or malformed modern evidence is never absence.
pub(super) fn present_stored<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Stored>, D::Error> {
    Stored::deserialize(deserializer).map(Some)
}
impl Stored {
    pub(super) fn commands(commands: Vec<Evidence>) -> Self {
        Self::Commands(Commands {
            version: Version::One,
            commands,
        })
    }
    pub(super) async fn command(
        &self,
        engine: &Engine,
        key: HarnessKey,
        command: Outgoing,
    ) -> Result<Outgoing, EngineError> {
        match self {
            Self::Adoption(source) => source.command(engine, key, command).await,
            Self::Commands(commands) => {
                let evidence = commands
                    .commands
                    .iter()
                    .find(|evidence| evidence.command.id == command.id)
                    .ok_or_else(ended)?;
                authorize_command(engine, evidence.attach(command)?).await
            }
        }
    }
}

/// Compose the existing command verifier at the actual native IO boundary too. A command
/// service may be in process; durable link issue/send still repeats its own final checkpoint.
async fn authorize_command(
    engine: &Engine,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    let mut tx = engine.inner.db.begin().await?;
    let row = orch_db::queries::link::command(&mut tx, command.id).await?;
    if let Some(row) = &row {
        if row.machine_id != command.machine
            || row.kind != command.kind.as_str()
            || row.serial_key() != command.key.wire().as_deref()
            || row.body != command.body
        {
            return Err(crate::commands::CommandsError::AuthorityMismatch.into());
        }
        command.authority = orch_db::queries::link::command_authority(&mut tx, row.id)
            .await?
            .map(serde_json::from_value)
            .transpose()?;
        // Results that crossed actual acceptance are reconciled by their native owner.
        if !matches!(row.state.as_str(), "queued" | "sent") {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(command);
        }
    }
    let Some(authority) = &command.authority else {
        // Only an actual canonical old command can have genuine absent provenance.
        if row.is_none() {
            return Err(ended());
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(command);
    };
    authority.lock_native_source(&mut tx).await?;
    let callers = match &row {
        Some(row) => {
            authority
                .current_callers(
                    &mut tx,
                    row,
                    engine.inner.clock.as_ref(),
                    engine.inner.access.as_ref(),
                )
                .await?
        }
        None => {
            let serial = command.key.wire();
            let proposed = orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: serial.as_deref(),
                deadline_at: command.deadline.map(Time::get).unwrap_or_else(|| {
                    engine.inner.clock.now()
                        + chrono::Duration::seconds(i64::from(command.kind.deadline_secs()))
                }),
            };
            authority
                .current_issue_callers(
                    &mut tx,
                    &proposed,
                    engine.inner.clock.as_ref(),
                    engine.inner.access.as_ref(),
                )
                .await?
        }
    }
    .ok_or_else(ended)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    for (caller, until) in callers {
        if caller
            .as_ref()
            .is_some_and(|caller| !caller.key_deadline_allows(engine.inner.clock.as_ref()))
            || until.is_some_and(|until| engine.inner.clock.time().get() >= until.get())
        {
            return Err(ended());
        }
    }
    Ok(command)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    command: Outgoing,
}
impl Evidence {
    fn new(command: &Outgoing, authority: CommandAuthority) -> Self {
        Self {
            command: command.clone().with_authority(authority),
        }
    }
    fn attach(&self, command: Outgoing) -> Result<Outgoing, EngineError> {
        let mut expected = self.command.clone();
        let authority = expected.authority.take().ok_or_else(ended)?;
        if expected != command {
            return Err(crate::commands::CommandsError::AuthorityMismatch.into());
        }
        Ok(command.with_authority(authority))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]
    use super::*;
    use orch_link_contract::commands::{SerialKey, TurnSteer};
    use orch_types::new_id;
    use serde_json::json;

    #[test]
    fn recovery_preserves_semantic_serial_keys_and_the_exact_native_envelope() {
        let thread = new_id();
        let command = Outgoing::new(
            new_id(),
            new_id(),
            SerialKey::Thread(thread),
            &TurnSteer {
                thread_id: thread,
                turn_id: new_id(),
                text: "continue".into(),
                images: vec![],
                native_action: None,
            },
        )
        .unwrap();
        // This tests the private journal envelope only, never grants input authority or performs IO.
        let evidence = Evidence::new(&command, CommandAuthority::input_steer(new_id()));
        let evidence: Evidence =
            serde_json::from_value(serde_json::to_value(evidence).unwrap()).unwrap();
        assert_eq!(
            evidence.attach(command.clone()).unwrap().authority,
            evidence.command.authority
        );
        let mut substitutes = Vec::new();
        let mut other = command.clone();
        other.id = new_id();
        substitutes.push(other);
        let mut other = command.clone();
        other.machine = new_id();
        substitutes.push(other);
        let mut other = command.clone();
        other.body["text"] = json!("replace");
        substitutes.push(other);
        let mut other = command.clone();
        other.deadline = Some("2030-01-01T00:00:00Z".parse().unwrap());
        substitutes.push(other);
        let mut other = command.clone();
        other.key = SerialKey::Provider(thread);
        assert_eq!(
            other.key.wire(),
            command.key.wire(),
            "wire spelling alone cannot distinguish native serial lanes"
        );
        substitutes.push(other);
        for other in substitutes {
            assert!(evidence.attach(other).is_err());
        }
    }

    #[test]
    fn absent_native_provenance_is_distinct_from_null_or_malformed_modern_sources() {
        assert!(serde_json::from_value::<super::super::AttachCommands>(json!({})).is_ok());
        for preparation in [
            json!(null),
            json!({}),
            json!({"version":"native_startup_v2","commands":[]}),
            json!({"origin":null,"credential":null}),
        ] {
            assert!(
                serde_json::from_value::<super::super::AttachCommands>(
                    json!({"preparation":preparation})
                )
                .is_err()
            );
        }
    }
}
