//! Exact private command evidence, retaining any independently required native composition source.

use super::*;
use crate::commands::CommandAuthority;
use futures::future::BoxFuture;
use orch_db::queries::link::{CommandRow, NewCommand};

type CurrentCallers = Vec<(Option<orch_authz::Caller>, Option<Time>)>;

/// Only the native replacement runner constructs this private accepted proposal.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    job: Id,
    source: Input,
    frame: Json,
    previous: Option<Box<CommandAuthority>>,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnvironmentUpdateCommand([private])")
    }
}
struct Wire<'a> {
    id: Id,
    machine: Id,
    key: Option<&'a str>,
    kind: &'a str,
    body: &'a Json,
}

impl Command {
    pub(super) fn capture(row: &jobs::JobRow, outgoing: &Outgoing) -> Result<Self, EngineError> {
        let binding = binding(row)?
            .ok_or_else(|| EngineError::unreadable("the native update source disappeared"))?;
        let frame = canonical(
            outgoing.id,
            outgoing.machine,
            outgoing.key.wire().as_deref(),
            outgoing.kind.as_str(),
            &outgoing.body,
        )?;
        if !command_identity::recorded_matches(
            row,
            COMMANDS,
            outgoing.id,
            outgoing.machine,
            outgoing.key.wire().as_deref(),
            outgoing.kind.as_str(),
            &outgoing.body,
        )? && !command_identity::recorded_matches(
            row,
            NOTICES,
            outgoing.id,
            outgoing.machine,
            outgoing.key.wire().as_deref(),
            outgoing.kind.as_str(),
            &outgoing.body,
        )? {
            return Err(EngineError::unreadable(
                "the update proposal has no native journal entry",
            ));
        }
        Ok(Self {
            job: row.id,
            source: binding.source,
            frame,
            previous: outgoing.authority.clone().map(Box::new),
        })
    }
    /// The source's typed native kind; a different command cannot reuse this carrier.
    pub fn command_kind(&self) -> &'static str {
        self.frame
            .get("kind")
            .and_then(Json::as_str)
            .and_then(|kind| {
                kind.parse::<orch_link_contract::commands::CommandKind>()
                    .ok()
            })
            .map(|kind| kind.as_str())
            .unwrap_or("invalid")
    }
    /// Actor/unit locks precede the job and command row, as in HTTP/native preparation.
    pub fn lock_source<'a>(&'a self, conn: &'a mut Conn) -> BoxFuture<'a, Result<(), EngineError>> {
        Box::pin(async move {
            let _ = orch_db::queries::transaction_guard::lock_native_actor(
                conn,
                self.source.origin().user(),
            )
            .await?;
            if let Some(previous) = &self.previous {
                Box::pin(previous.lock_native_source(conn)).await?;
            }
            Ok(())
        })
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        wire: Wire<'_>,
    ) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
        let Wire {
            id,
            machine,
            key,
            kind,
            body,
        } = wire;
        if canonical(id, machine, key, kind, body)? != self.frame {
            return Ok(None);
        }
        let Some(row) = jobs::lock(conn, self.job).await? else {
            return Ok(None);
        };
        if !present(&row)
            || !command_identity::recorded_matches(&row, COMMANDS, id, machine, key, kind, body)?
                && !command_identity::recorded_matches(&row, NOTICES, id, machine, key, kind, body)?
        {
            return Ok(None);
        }
        let Some(binding) = binding(&row)? else {
            return Ok(None);
        };
        if !self.source.same_request(&binding.source) {
            return Ok(None);
        }
        if super::super::accepted_notice(conn, &row, id, machine, key, kind, body).await? {
            // Completion of this exact stored system notice uses no sender authority.
            return Ok(Some(Vec::new()));
        }
        let original = match self.source.checkpoint(conn, clock).await {
            Ok(check) if check.matches() => check,
            Ok(_) => return Ok(None),
            Err(EngineError::BeforeEffect(error))
                if error.is::<crate::updates::operation::Ended>()
                    || error.is::<crate::updates::operation::Denied>() =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let Some(mut current) = callers(conn, &row, clock).await? else {
            return Ok(None);
        };
        current.push((Some(original.principal().caller().clone()), original.until));
        Ok(Some(current))
    }
    /// Refresh the actual proposed insertion after waits, including any native inner source.
    pub fn current_issue_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a NewCommand<'_>,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            let Some(mut callers) = self
                .current(
                    conn,
                    clock,
                    Wire {
                        id: command.id,
                        machine: command.machine_id,
                        key: command.serial_key,
                        kind: command.kind,
                        body: command.body,
                    },
                )
                .await?
            else {
                return Ok(None);
            };
            if let Some(previous) = &self.previous {
                let Some(inner) =
                    Box::pin(previous.current_issue_callers(conn, command, clock, access)).await?
                else {
                    return Ok(None);
                };
                callers.extend(inner);
            }
            Ok(Some(callers))
        })
    }
    /// Refresh a retained unaccepted dispatch; accepted receipts keep their native journal path.
    pub fn current_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a CommandRow,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            let Some(mut callers) = self
                .current(
                    conn,
                    clock,
                    Wire {
                        id: command.id,
                        machine: command.machine_id,
                        key: command.serial_key(),
                        kind: &command.kind,
                        body: &command.body,
                    },
                )
                .await?
            else {
                return Ok(None);
            };
            if let Some(previous) = &self.previous {
                let Some(inner) =
                    Box::pin(previous.current_callers(conn, command, clock, access)).await?
                else {
                    return Ok(None);
                };
                callers.extend(inner);
            }
            Ok(Some(callers))
        })
    }
}
