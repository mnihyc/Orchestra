//! Exact original operation and native copy, kept in the existing private command channel.

use super::importing::{Input, Offering};
use crate::{EngineError, commands::Outgoing, operation_scope::continuing};
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{environment_sources::MCP_IMPORT, operations::FacetUsage};
use orch_authz::{Caller, CapturedOperation};
use orch_db::{
    Conn,
    queries::link::{CommandRow, NewCommand},
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// One genuine creator and its exact physical copy; never a machine wire-body field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyCommand {
    id: Id,
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    input: Input,
    canonical: Json,
}
impl std::fmt::Debug for CopyCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpCopyCommand([private])")
    }
}
impl CopyCommand {
    /// Capture only the already admitted operation and the native producer's actual command.
    /// Fresh queue/send checks restore this origin and repeat the shared Offering factory.
    pub fn capture(
        caller: &Caller,
        session: Option<Id>,
        input: Input,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let row = crate::operation_scope::Prepared::for_selection(MCP_IMPORT.operation)
            .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        let same = row
            .variant()
            .facets
            .iter()
            .filter(|facet| facet.usage == FacetUsage::Required)
            .all(|facet| {
                facet.resources.iter().all(|kind| {
                    admitted.iter().any(|fact| {
                        fact.operation == MCP_IMPORT.operation
                            && fact.action == facet.action
                            && fact.resource_kind == *kind
                            && fact.target.object.kind
                                == KeyTargetKind::Object(ObjectKind::Environment)
                            && fact.target.object.id == Some(input.environment())
                    })
                })
            });
        if !same
            || origin.operation() != MCP_IMPORT.operation
            || session.is_some() && caller.grant().is_some()
            || command.kind.as_str() != "mcp.import"
            || command.machine != input.machine()
            || command.key != input.serial()
            || command.body != serde_json::to_value(input.command())?
        {
            return Err(EngineError::unreadable(
                "the MCP copy does not match its original native source",
            ));
        }
        Ok(Self {
            id: command.id,
            origin,
            session,
            input,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }

    /// Original operation for native result persistence, never completed-copy Read authority.
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    /// Exact source retained by the accepted copy, without reselecting a new offering.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// Canonical forward command whose successful native result can own inverse cleanup.
    pub fn id(&self) -> Id {
        self.id
    }

    pub(super) async fn completed(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        let Some(stored) = orch_db::queries::link::command_with_authority(conn, self.id).await?
        else {
            return Ok(false);
        };
        if stored.command.state != "succeeded"
            || stored.command.result.as_ref().is_none_or(|result| {
                serde_json::from_value::<orch_link_contract::commands::McpImportResult>(
                    result.clone(),
                )
                .is_err()
            })
            || stored.authority
                != Some(serde_json::to_value(
                    crate::commands::CommandAuthority::mcp_import(self.clone()),
                )?)
        {
            return Ok(false);
        }
        Ok(crate::jobs::command_identity::canonical(
            stored.command.id,
            stored.command.machine_id,
            stored.command.serial_key(),
            &stored.command.kind,
            &stored.command.body,
        )? == self.canonical)
    }

    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "mcp.import"
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
        let Some(session) = crate::commands::authority::session_state(
            conn,
            &self.origin,
            self.session,
            clock.time(),
        )
        .await?
        else {
            return Ok(None);
        };
        let frame = match continuing::prepare::<Offering>(
            conn,
            clock,
            &self.origin,
            self.input.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(continuing::Error::Refused)
            | Err(continuing::Error::Binding(crate::operation_scope::BindingError::Refused)) => {
                return Ok(None);
            }
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        let caller = frame.principal().caller().clone();
        if self.session.is_some() && caller.grant().is_some()
            || caller.as_admin()
                && self.session.is_some()
                && !session.is_some_and(|(admin, _)| admin)
        {
            return Ok(None);
        }
        if frame.into_parts().first_effect.check(clock).is_err() {
            return Ok(None);
        }
        Ok(Some(vec![(Some(caller), session.map(|(_, until)| until))]))
    }

    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
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
