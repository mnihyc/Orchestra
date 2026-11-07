//! One native forget, owned by either an unfiled import copy or its recorded deletion.

use super::CopyCommand;
use crate::{EngineError, commands::Outgoing};
use orch_authz::Caller;
use orch_db::{
    Conn, DbError,
    queries::{
        link::{CommandRow, NewCommand},
        resources::mcp,
    },
};
use orch_link_contract::commands::{CommandBody, McpForget, SerialKey};
use orch_types::{Clock, Json, Time};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};

mod recorded_deletion;
pub use recorded_deletion::RecordedDeletionCustody;

/// Distinct native custody proofs for the same connector command.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "proof",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ForgetCustody {
    /// An exact successful copy whose generated server identity was never filed.
    UnfiledCopy {
        /// The native forward copy, not a renewed caller.
        copy: Box<CopyCommand>,
        /// Its original native inverse budget.
        until: Time,
    },
    /// A cleanup command journaled atomically with the accepted resource deletion.
    RecordedDeletion(Box<RecordedDeletionCustody>),
    /// This real recorded server ended while its exact accepted copy was completing.
    AbandonedExistingCopy {
        copy: Box<super::existing::Command>,
        until: Time,
    },
}
impl std::fmt::Debug for ForgetCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnfiledCopy { .. } => "UnfiledMcpCopy([owned])",
            Self::RecordedDeletion(_) => "RecordedMcpDeletion([owned])",
            Self::AbandonedExistingCopy { .. } => "AbandonedExistingMcpCopy([owned])",
        })
    }
}

/// Native inverse custody, with no actor or renewed authority to create anything.
#[derive(Clone, PartialEq, Eq)]
pub struct ForgetCommand {
    custody: ForgetCustody,
    canonical: Json,
}

// Keep earlier unfiled-copy records byte-compatible in the private authority channel.
impl Serialize for ForgetCommand {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.custody {
            ForgetCustody::UnfiledCopy { copy, until } => {
                let mut value = serializer.serialize_struct("ForgetCommand", 3)?;
                value.serialize_field("copy", copy)?;
                value.serialize_field("canonical", &self.canonical)?;
                value.serialize_field("until", until)?;
                value.end()
            }
            ForgetCustody::RecordedDeletion(_) | ForgetCustody::AbandonedExistingCopy { .. } => {
                let mut value = serializer.serialize_struct("ForgetCommand", 2)?;
                value.serialize_field("custody", &self.custody)?;
                value.serialize_field("canonical", &self.canonical)?;
                value.end()
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Unfiled {
    copy: CopyCommand,
    canonical: Json,
    until: Time,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tagged {
    custody: ForgetCustody,
    canonical: Json,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored {
    Tagged(Box<Tagged>),
    Unfiled(Box<Unfiled>),
}
impl<'de> Deserialize<'de> for ForgetCommand {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Tagged(value) => Self {
                custody: value.custody,
                canonical: value.canonical,
            },
            Stored::Unfiled(value) => Self {
                custody: ForgetCustody::UnfiledCopy {
                    copy: Box::new(value.copy),
                    until: value.until,
                },
                canonical: value.canonical,
            },
        })
    }
}
impl std::fmt::Debug for ForgetCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpForgetCommand([owned])")
    }
}
impl ForgetCommand {
    /// Only the real completed stored-resource copy can remove its file after that target ended.
    /// No active resource, fabricated creator or renewed invoker is admitted by this inverse.
    pub async fn capture_abandoned_existing(
        conn: &mut Conn,
        clock: &dyn Clock,
        copy: super::existing::Command,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.id == copy.input().id()
            || command.kind != McpForget::KIND
            || command.machine != copy.input().machine()
            || command.key != SerialKey::Machine
            || command.body
                != serde_json::to_value(McpForget {
                    server_id: copy.input().server(),
                })?
            || copy.completed(conn).await?.is_none()
            || !mcp::servers(conn, &[copy.input().server()])
                .await?
                .is_empty()
        {
            return Err(EngineError::unreadable(
                "the inverse does not own an ended server's exact successful copy",
            ));
        }
        let until = Time::new(
            clock.now() + chrono::Duration::seconds(i64::from(McpForget::KIND.deadline_secs())),
        );
        Ok(Self {
            custody: ForgetCustody::AbandonedExistingCopy {
                copy: Box::new(copy),
                until,
            },
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if let ForgetCustody::AbandonedExistingCopy { copy, .. } = &self.custody {
            copy.lock_source(conn).await?;
        }
        Ok(())
    }
    /// Own only a real successful, canonical, privately recorded copy which was never filed.
    /// The ordinary native Forget budget bounds its cleanup; the ended invoker is not restored.
    pub async fn capture(
        conn: &mut Conn,
        clock: &dyn Clock,
        copy: CopyCommand,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.id == copy.id()
            || command.kind != McpForget::KIND
            || command.machine != copy.input().machine()
            || command.key != SerialKey::Machine
            || command.body
                != serde_json::to_value(McpForget {
                    server_id: copy.input().server(),
                })?
            || !copy.completed(conn).await?
            || mcp::recorded(conn, copy.input().server()).await?
        {
            return Err(EngineError::unreadable(
                "the native cleanup does not own an unfiled successful MCP copy",
            ));
        }
        Ok(Self {
            custody: ForgetCustody::UnfiledCopy {
                copy: Box::new(copy),
                until: Time::new(
                    clock.now()
                        + chrono::Duration::seconds(i64::from(McpForget::KIND.deadline_secs())),
                ),
            },
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }

    /// Capture before deletion; the writer must journal this exact command with that deletion.
    /// It cannot be issued as a new command after the writer commits.
    pub async fn capture_recorded_deletion(
        conn: &mut Conn,
        caller: &Caller,
        offering: &mcp::Offering,
        deleted_at: Time,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let custody =
            RecordedDeletionCustody::capture(conn, caller, offering, deleted_at, command).await?;
        Ok(Self {
            custody: ForgetCustody::RecordedDeletion(Box::new(custody)),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }

    /// The native proof type, without granting insertion or restoring an invoker.
    pub fn custody(&self) -> &ForgetCustody {
        &self.custody
    }

    /// Unfiled inverses pin their budget; accepted deletions use their actual journal's deadline.
    pub fn deadline(&self) -> Option<Time> {
        match &self.custody {
            ForgetCustody::UnfiledCopy { until, .. } => Some(*until),
            ForgetCustody::RecordedDeletion(_) => None,
            ForgetCustody::AbandonedExistingCopy { until, .. } => Some(*until),
        }
    }

    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if let ForgetCustody::AbandonedExistingCopy { copy, until } = &self.custody {
            if command.kind != "mcp.forget"
                || command.deadline_at != until.get()
                || clock.now() >= until.get()
                || crate::jobs::command_identity::canonical(
                    command.id,
                    command.machine_id,
                    command.serial_key,
                    command.kind,
                    command.body,
                )? != self.canonical
            {
                return Ok(None);
            }
            copy.lock_source(conn).await?;
            if copy.completed(conn).await?.is_none()
                || !mcp::servers(conn, &[copy.input().server()])
                    .await?
                    .is_empty()
                || clock.now() >= until.get()
            {
                return Ok(None);
            }
            return Ok(Some(vec![(None, Some(*until))]));
        }
        let ForgetCustody::UnfiledCopy { copy, until } = &self.custody else {
            // An accepted deletion owns only the forget inserted by its native transaction.
            return Ok(None);
        };
        if command.kind != "mcp.forget"
            || command.deadline_at != until.get()
            || until.get() <= clock.now()
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        // Filing and its owned inverse serialize on the same actual Environment.
        // A completed public server is never undone by this private copy cleanup.
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(copy.input().environment())
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        if !copy.completed(conn).await?
            || mcp::recorded(conn, copy.input().server()).await?
            || until.get() <= clock.now()
        {
            return Ok(None);
        }
        Ok(Some(vec![(None, Some(*until))]))
    }

    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if let ForgetCustody::RecordedDeletion(custody) = &self.custody {
            if command.kind != "mcp.forget"
                || command.serial_key() != SerialKey::Machine.wire().as_deref()
                || command.deadline_at <= clock.now()
                || crate::jobs::command_identity::canonical(
                    command.id,
                    command.machine_id,
                    command.serial_key(),
                    &command.kind,
                    &command.body,
                )? != self.canonical
            {
                return Ok(None);
            }
            let Some(journal) =
                orch_db::queries::link::command_with_authority(conn, command.id).await?
            else {
                return Ok(None);
            };
            let expected =
                serde_json::to_value(crate::commands::CommandAuthority::mcp_forget(self.clone()))?;
            if journal.authority != Some(expected)
                || journal.command.deadline_at != command.deadline_at
                || crate::jobs::command_identity::canonical(
                    journal.command.id,
                    journal.command.machine_id,
                    journal.command.serial_key(),
                    &journal.command.kind,
                    &journal.command.body,
                )? != self.canonical
                || !custody.current(conn, command).await?
                || command.deadline_at <= clock.now()
            {
                return Ok(None);
            }
            return Ok(Some(vec![(None, Some(Time::new(command.deadline_at)))]));
        }
        self.current_issue_callers(
            conn,
            &NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}
