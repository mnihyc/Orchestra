//! What the engine asks of the machine link: send a command, and learn how it ended.
//!
//! A command is a row before it is anything else, sent whenever its machine
//! has a session and again after every reconnect and restart, until the
//! machine has answered, its deadline has passed or it was cancelled. The
//! engine relies on three properties of that, and its tests on a double that
//! has them: a command asked for twice under one identifier is made once;
//! its end can be read at any time after, also by a process that did not
//! issue it; and waiting for it survives nothing but the row.

use async_trait::async_trait;
use orch_link_contract::commands::{CommandBody, CommandError, CommandKind, SerialKey};
use orch_types::{Id, Json, Time};

/// Use the lifecycle runner's existing half-second retry spacing for Busy cleanup attempts.
pub(crate) const BUSY_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(500);

/// Why a command could not be sent, or its end not learned.
#[derive(Debug, thiserror::Error)]
pub enum CommandsError {
    /// The system does not know the machine, or it was deleted.
    #[error("the system does not know that machine")]
    UnknownMachine,
    /// The machine was revoked.
    #[error("the machine was revoked")]
    Revoked,
    /// The system is stopping.
    #[error("the system is stopping")]
    Stopping,
    /// The body of the command cannot be written.
    #[error("the body of the command cannot be written: {0}")]
    Body(String),
    /// Anything else the link or the database reports.
    #[error("{0}")]
    Other(String),
}

/// A command to send to a machine.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Outgoing {
    /// Its identifier: a command asked for twice under one is made once.
    pub id: Id,
    /// The machine it is for.
    pub machine: Id,
    /// What it must not run beside.
    #[serde(with = "RetainedKey")]
    pub key: SerialKey,
    /// Its kind.
    pub kind: CommandKind,
    /// Its body.
    pub body: Json,
    /// The moment after which it is not to be started; the kind's own time
    /// after now when none is given.
    pub deadline: Option<Time>,
}

/// A saved engine command retains the semantic key kind as well as its value. UUID keys
/// intentionally share the link's wire spelling, so that spelling cannot decode the kind.
/// This tagged representation belongs to the job journal; Cmd.key still uses SerialKey::wire.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(
    remote = "SerialKey",
    tag = "kind",
    content = "value",
    rename_all = "snake_case"
)]
enum RetainedKey {
    None,
    Machine,
    Thread(Id),
    Harness { provider_id: Id, agent_id: Id },
    Provider(Id),
    Environment(Id),
    Runtime(Id),
    Volume(String),
    Image(String),
    Tool(String),
}

impl Outgoing {
    /// The command with body `body`, which names its own kind.
    pub fn new<B: CommandBody>(
        id: Id,
        machine: Id,
        key: SerialKey,
        body: &B,
    ) -> Result<Self, CommandsError> {
        Ok(Self {
            id,
            machine,
            key,
            kind: B::KIND,
            body: serde_json::to_value(body)
                .map_err(|error| CommandsError::Body(error.to_string()))?,
            deadline: None,
        })
    }

    /// The same command with a deadline of its own.
    #[must_use]
    pub fn by(mut self, deadline: Time) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

/// Capture the segment at admission, not when a delayed lifecycle command is finally delivered.
pub(crate) async fn session_close(
    conn: &mut orch_db::Conn,
    id: Id,
    machine: Id,
    thread: Id,
    segment: Option<Id>,
) -> Result<Outgoing, crate::error::EngineError> {
    if requires_fence(conn, thread).await? {
        let segment = segment.ok_or_else(|| {
            crate::error::EngineError::unreadable(
                "A continuation lifecycle close needs its captured native segment.",
            )
        })?;
        // A job may own a provisional segment not published to history yet. Existing
        // segments must belong to this thread; an absent one stays its captured UUID.
        if orch_db::queries::threads::segment(conn, segment)
            .await?
            .is_some_and(|row| row.thread_id != thread)
        {
            return Err(crate::error::EngineError::unreadable(
                "The captured session segment belongs to another thread.",
            ));
        }
        return Outgoing::new(
            id,
            machine,
            SerialKey::Thread(thread),
            &orch_link_contract::continuations::SessionCloseFenced {
                thread_id: thread,
                segment_id: segment,
            },
        )
        .map_err(Into::into);
    }
    Outgoing::new(
        id,
        machine,
        SerialKey::Thread(thread),
        &orch_link_contract::commands::SessionClose { thread_id: thread },
    )
    .map_err(Into::into)
}

/// A failed preparation can also leave its original lane guarded; do not infer this from selection alone.
pub(crate) async fn requires_fence(
    conn: &mut orch_db::Conn,
    thread: Id,
) -> Result<bool, crate::error::EngineError> {
    Ok(sqlx::query_scalar(
        "SELECT t.execution_revision>1 OR EXISTS(SELECT 1 FROM thread_continuation_operations o
        WHERE o.thread_id=t.node_id) OR EXISTS(SELECT 1 FROM thread_native_guards g
        WHERE g.thread_id=t.node_id) FROM threads t WHERE t.node_id=$1",
    )
    .bind(thread)
    .fetch_one(conn)
    .await
    .map_err(orch_db::DbError::from)?)
}

/// Configuration refresh may replace only the idle native generation captured by its owner.
pub(crate) async fn session_open(
    conn: &mut orch_db::Conn,
    id: Id,
    machine: Id,
    body: &orch_link_contract::commands::SessionOpen,
    predecessor: Option<orch_agent_contract::history::NativePrefixSource>,
) -> Result<Outgoing, crate::error::EngineError> {
    if predecessor
        .as_ref()
        .is_some_and(|source| source.thread_id != body.thread_id)
    {
        return Err(crate::error::EngineError::unreadable(
            "The captured predecessor belongs to another thread.",
        ));
    }
    if requires_fence(conn, body.thread_id).await? {
        return Outgoing::new(
            id,
            machine,
            SerialKey::Thread(body.thread_id),
            &orch_link_contract::continuations::SessionOpenFenced {
                thread_id: body.thread_id,
                session: Box::new(body.clone()),
                predecessor,
            },
        )
        .map_err(Into::into);
    }
    Outgoing::new(id, machine, SerialKey::Thread(body.thread_id), body).map_err(Into::into)
}

/// Stable next attempt for a retryable Busy refusal; a terminal command ID is never rerun.
#[must_use]
pub(crate) fn busy_retry_id(previous: Id) -> Id {
    crate::ids::derived("command.busy-retry", &[previous.as_bytes()])
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The machine answered, with this result.
    Succeeded(Json),
    /// The machine failed it, or did not answer.
    Failed(CommandError),
    /// Its deadline passed before a machine took it.
    Expired,
    /// It was cancelled.
    Cancelled,
}

/// How far a command has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// It waits for a machine.
    Queued,
    /// It was sent and the machine has not said that it has it.
    Sent,
    /// The machine has it.
    Accepted,
    /// It is over.
    Over,
}

/// Where a command stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    /// How far it has come.
    pub stage: Stage,
    /// When it was issued.
    pub issued_at: chrono::DateTime<chrono::Utc>,
    /// How it ended, once it is over.
    pub outcome: Option<Outcome>,
}

/// A command's original machine and protected body, read from one retained record.
#[derive(Debug, Clone, PartialEq)]
pub struct RetainedCommand {
    /// The machine the command was admitted for.
    pub machine: Id,
    /// The body the command retained for replay.
    pub body: Json,
}

/// The commands the engine sends and waits for.
#[async_trait]
pub trait Commands: Send + Sync {
    /// Reads a live observation without putting its result in a durable command row.
    async fn observe(&self, _command: Outgoing) -> Result<Outcome, CommandsError> {
        Err(CommandsError::Other(
            "live native observations are unavailable".to_owned(),
        ))
    }

    /// Stores a command and hands it to the machine's link. Asked again
    /// under an identifier that exists, it does nothing.
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError>;

    /// The retained protected body of a command, including an expired tool-result delivery.
    async fn retained_body(&self, _id: Id) -> Result<Option<Json>, CommandsError> {
        Ok(None)
    }

    /// Reads the frozen recipient with the body so a replay cannot choose another machine.
    async fn retained_command(&self, id: Id) -> Result<Option<RetainedCommand>, CommandsError> {
        if self.retained_body(id).await?.is_some() {
            return Err(CommandsError::Other(
                "a retained command has no proven original machine".to_owned(),
            ));
        }
        Ok(None)
    }

    /// Where a command stands now; none where there is no such command.
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError>;

    /// Waits until a command is over, and says how it ended.
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError>;

    /// Asks that a command that is not over not be carried out; whatever the
    /// machine then answers is not heard. Says whether it was not over.
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError>;

    /// Whether the machine returned from its handler and cleanup.
    /// A retained success proves completion; other outcomes need the desk's actual acknowledgement.
    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        Ok(self.status(id).await?.is_some_and(|info| {
            info.stage == Stage::Over && matches!(info.outcome, Some(Outcome::Succeeded(_)))
        }))
    }

    /// Whether events that `machine` sent on `stream` have reached the system and are not taken in
    /// yet (C-turns): what the engine waits to hear from that machine may be among them. Where no
    /// link can be asked, none are.
    fn hears_behind(&self, _machine: Id, _stream: &orch_link_contract::streams::Stream) -> bool {
        false
    }
}

/// Issues a command and waits for its end.
pub async fn run(commands: &dyn Commands, command: Outgoing) -> Result<Outcome, CommandsError> {
    let id = command.id;
    commands.issue(command).await?;
    commands.wait(id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct ReportedStatus(Option<Info>);

    #[async_trait]
    impl Commands for ReportedStatus {
        async fn issue(&self, _: Outgoing) -> Result<(), CommandsError> {
            Err(CommandsError::Stopping)
        }
        async fn status(&self, _: Id) -> Result<Option<Info>, CommandsError> {
            Ok(self.0.clone())
        }
        async fn wait(&self, _: Id) -> Result<Outcome, CommandsError> {
            Err(CommandsError::Stopping)
        }
        async fn cancel(&self, _: Id) -> Result<bool, CommandsError> {
            Err(CommandsError::Stopping)
        }
    }

    #[tokio::test]
    async fn a_retained_native_success_proves_stop_but_uncertain_outcomes_never_do() {
        let id = orch_types::new_id();
        assert!(!ReportedStatus(None).stopped(id).await.unwrap());
        for (stage, outcome, expected) in [
            (Stage::Over, Some(Outcome::Succeeded(json!({}))), true),
            (Stage::Accepted, Some(Outcome::Succeeded(json!({}))), false),
            (Stage::Over, Some(Outcome::Cancelled), false),
            (Stage::Over, Some(Outcome::Expired), false),
            (
                Stage::Over,
                Some(Outcome::Failed(CommandError::new(
                    orch_link_contract::commands::ErrorCode::Timeout,
                    "an unanswered command may still be running",
                ))),
                false,
            ),
            (Stage::Over, None, false),
        ] {
            let commands = ReportedStatus(Some(Info {
                stage,
                issued_at: chrono::DateTime::UNIX_EPOCH,
                outcome,
            }));
            assert_eq!(commands.stopped(id).await.unwrap(), expected);
        }
    }

    #[test]
    fn retained_commands_preserve_every_semantic_key_kind_and_the_exact_wire_body()
    -> Result<(), Box<dyn std::error::Error>> {
        let id = orch_types::new_id();
        let other = orch_types::new_id();
        let keys = [
            SerialKey::None,
            SerialKey::Machine,
            SerialKey::Thread(id),
            SerialKey::Harness {
                provider_id: id,
                agent_id: other,
            },
            SerialKey::Provider(id),
            SerialKey::Environment(id),
            SerialKey::Runtime(id),
            SerialKey::Volume("synthetic-volume".into()),
            SerialKey::Image("synthetic-image".into()),
            SerialKey::Tool("synthetic-tool".into()),
        ];
        for key in keys {
            let outgoing = Outgoing::new(
                id,
                other,
                key.clone(),
                &orch_link_contract::commands::SessionClose { thread_id: id },
            )?;
            let written = serde_json::to_value(&outgoing)?;
            let restored: Outgoing = serde_json::from_value(written.clone())?;
            assert_eq!(restored, outgoing);
            assert_eq!(restored.key.wire(), key.wire());
            assert!(written["key"].get("kind").is_some());
            assert_eq!(written["body"], json!({"thread_id":id}));
        }
        let outgoing = Outgoing::new(
            id,
            other,
            SerialKey::Thread(id),
            &orch_link_contract::commands::SessionClose { thread_id: id },
        )?;
        let mut written = serde_json::to_value(&outgoing)?;
        assert_eq!(written["key"], json!({"kind":"thread","value":id}));
        for raw in [
            json!(id),
            Json::Null,
            json!({"kind":"unrecognized","value":id}),
        ] {
            written["key"] = raw;
            assert!(
                serde_json::from_value::<Outgoing>(written.clone()).is_err(),
                "a damaged key must not be guessed from its wire spelling"
            );
        }
        written["key"] = json!({"kind":"environment","value":id});
        let environment: Outgoing = serde_json::from_value(written)?;
        assert_eq!(environment.key.wire(), outgoing.key.wire());
        assert_ne!(
            environment.key, outgoing.key,
            "equal UUID wire spellings retain different queue kinds in the journal"
        );
        Ok(())
    }
}
