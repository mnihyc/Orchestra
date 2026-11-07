//! Exact reducing cleanup of an MCP resource whose deletion already committed.

use super::Outgoing;
use crate::{
    EngineError,
    operation_scope::{
        sources::{self, Facts},
        targets,
    },
};
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    mcp_actions::{self, Source},
    resources::McpServerDelete,
    routes::Endpoint,
};
use orch_authz::{Caller, CapturedOperation};
use orch_db::{Conn, queries::link::CommandRow};
use orch_link_contract::commands::{CommandKind, McpForget, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// One accepted deletion's original operation and canonical private-copy cleanup.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpDeletionCleanup {
    origin: CapturedOperation,
    server: Id,
    machine: Id,
    environment: Option<Id>,
    source: Json,
    deleted_at: Time,
    canonical: Json,
}
impl std::fmt::Debug for McpDeletionCleanup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpDeletionCleanup([private])")
    }
}
impl McpDeletionCleanup {
    /// Called only by the admitted native deletion writer, which records this exact command
    /// in the same transaction as the deletion. No connector work starts before that commit.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        offering: &orch_db::queries::resources::mcp::Offering,
        deleted_at: Time,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let server = offering.server_id;
        if origin.operation() != "McpServerDelete"
            || server.is_nil()
            || command.machine != offering.machine_id
            || command.kind != CommandKind::McpForget
            || command.key != SerialKey::Machine
            || command.body != serde_json::to_value(McpForget { server_id: server })?
        {
            return Err(EngineError::unreadable(
                "not the accepted MCP deletion's native cleanup",
            ));
        }
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(EngineError::unreadable(
                "the deletion cleanup requires its native transaction",
            ));
        }
        let subject = orch_db::queries::resources::mcp::subject(conn, server)
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("the native deletion subject is no longer live")
            })?;
        let governor = match subject.environment_id {
            Some(id) => ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            None => ObjectRef {
                kind: ObjectKind::McpServer,
                id: server,
            },
        };
        let actual = orch_db::queries::resources::mcp::offerings(conn, &[server]).await?;
        if !actual.iter().any(|current| {
            current.machine_id == offering.machine_id
                && current.environment_id == offering.environment_id
                && current.source == offering.source
        }) {
            return Err(EngineError::unreadable(
                "the native deletion copy association changed",
            ));
        }
        let facts = [
            Facts::one(
                Source::ManagedServer,
                KeyTargetKind::Object(ObjectKind::McpServer),
                targets::page_object(conn, governor)
                    .await
                    .map_err(EngineError::unreadable)?,
            ),
            Facts::collection(
                Source::ForgottenCopies,
                vec![(
                    KeyTargetKind::Object(ObjectKind::Machine),
                    vec![
                        targets::machine_subject(conn, offering.machine_id)
                            .await
                            .map_err(EngineError::unreadable)?,
                    ],
                )],
            ),
        ];
        let expected = sources::bind_required(McpServerDelete::ROUTE, &mcp_actions::DELETE, &facts)
            .map_err(EngineError::unreadable)?
            .required_facets()
            .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        if !expected.iter().all(|facet| admitted.contains(facet))
            || !caller.permits_facets(admitted)
        {
            return Err(EngineError::unreadable(
                "the deletion did not admit this actual native copy",
            ));
        }
        Ok(Self {
            origin,
            server,
            machine: offering.machine_id,
            environment: offering.environment_id,
            source: offering.source.clone(),
            deleted_at,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }

    pub(super) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if self.origin.operation() != "McpServerDelete"
            || self.origin.user().is_nil()
            || self.server.is_nil()
            || command.kind != "mcp.forget"
            || command.machine_id != self.machine
            || command.serial_key() != SerialKey::Machine.wire().as_deref()
            || command.body
                != serde_json::to_value(McpForget {
                    server_id: self.server,
                })?
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key(),
                &command.kind,
                &command.body,
            )? != self.canonical
            || command.deadline_at <= clock.now()
        {
            return Ok(None);
        }
        let accepted: Option<Id> = sqlx::query_scalar(
            "SELECT s.id FROM mcp_servers s JOIN mcp_server_offerings o ON o.server_id=s.id
                WHERE s.id=$1 AND s.deleted_at=$2 AND o.machine_id=$3 AND o.source=$4
                  AND o.environment_id IS NOT DISTINCT FROM $5 FOR SHARE OF s,o",
        )
        .bind(self.server)
        .bind(self.deleted_at.get())
        .bind(self.machine)
        .bind(&self.source)
        .bind(self.environment)
        .fetch_optional(conn)
        .await
        .map_err(orch_db::DbError::from)?;
        // This is removal of the accepted resource's copied file, never a new import,
        // read or privilege. Its actual deletion and native command deadline are the fence.
        Ok((accepted.is_some() && command.deadline_at > clock.now())
            .then(|| vec![(None, Some(Time::new(command.deadline_at)))]))
    }
}
