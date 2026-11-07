//! Accepted deletion custody for the exact forget journaled in its native transaction.

use crate::{
    EngineError,
    commands::Outgoing,
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
use orch_db::{Conn, DbError, queries::link::CommandRow};
use orch_link_contract::commands::{CommandKind, McpForget, SerialKey};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};

/// The actual accepted deletion and its selected native copy, without invoker authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedDeletionCustody {
    origin: CapturedOperation,
    server: Id,
    machine: Id,
    environment: Option<Id>,
    source: Json,
    deleted_at: Time,
}
impl std::fmt::Debug for RecordedDeletionCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecordedMcpDeletion([private])")
    }
}
impl RecordedDeletionCustody {
    pub(super) async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        offering: &orch_db::queries::resources::mcp::Offering,
        deleted_at: Time,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let server = offering.server_id;
        if origin.operation() != mcp_actions::DELETE.operation
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
        })
    }

    pub(super) async fn current(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
    ) -> Result<bool, EngineError> {
        if self.origin.operation() != mcp_actions::DELETE.operation
            || self.origin.user().is_nil()
            || self.server.is_nil()
            || command.machine_id != self.machine
            || command.body
                != serde_json::to_value(McpForget {
                    server_id: self.server,
                })?
        {
            return Ok(false);
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
        .map_err(DbError::from)?;
        Ok(accepted.is_some())
    }
}
