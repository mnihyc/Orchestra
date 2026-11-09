//! One native MCP deletion and its exact recorded-copy cleanup, shared by Delete and Move.
use crate::{
    EngineError,
    commands::{CommandAuthority, Outgoing},
};
use orch_access_contract::{
    audit::AuditAction,
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyFacet, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{
    Conn,
    queries::{
        link,
        resources::mcp::{self as rows, McpRow, Offering},
    },
};
use orch_link_contract::commands::{CommandBody, McpForget, SerialKey};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cleanup {
    id: Id,
    machine: Id,
    proof: super::ForgetCommand,
}
/// Private factual operands of one already admitted native ending; never a reusable actor.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deletion {
    origin: CapturedOperation,
    server: Id,
    fingerprint: Hash,
    source_home: Option<Id>,
    source_org: Option<Id>,
    offerings: Hash,
    home: Option<Id>,
    org: Option<Id>,
    name: String,
    deleted_at: Time,
    audit_origin: Json,
    cleanup: Vec<Cleanup>,
}
impl std::fmt::Debug for Deletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpDeletion([native])")
    }
}
fn refused() -> EngineError {
    EngineError::unreadable("The accepted native MCP deletion changed.")
}
/// The original native Delete selects cleanup only for currently active online copies.
pub async fn copies(conn: &mut Conn, server: Id) -> Result<Vec<Offering>, EngineError> {
    let mut selected = Vec::new();
    for offering in rows::offerings(conn, &[server]).await? {
        if orch_db::queries::resources::machine(conn, offering.machine_id)
            .await?
            .is_some_and(|machine| machine.online && machine.is_active())
        {
            selected.push(offering);
        }
    }
    Ok(selected)
}
async fn offering_stamp(conn: &mut Conn, server: Id) -> Result<Hash, EngineError> {
    let mut copies = rows::offerings(conn, &[server]).await?;
    copies.sort_by_key(|copy| copy.machine_id);
    let shape:Vec<_>=copies.iter().map(|copy|serde_json::json!({"machine":copy.machine_id,"environment":copy.environment_id,"source":copy.source})).collect();
    crate::jobs::command_identity::hash(&serde_json::to_value(shape)?)
}
impl Deletion {
    /// Capture only the current native Delete or Move source, under its complete canonical admission.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        row: &McpRow,
        copies: &[Offering],
        deleted_at: Time,
        origin: &orch_db::audit::Origin,
    ) -> Result<Self, EngineError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(refused());
        }
        let operation = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if !matches!(
            operation.operation(),
            "McpServerDelete" | "McpServerMoveOut"
        ) {
            return Err(refused());
        }
        let governor = ObjectRef {
            kind: if row.environment_id.is_some() {
                ObjectKind::Environment
            } else {
                ObjectKind::McpServer
            },
            id: row.environment_id.unwrap_or(row.id),
        };
        let source = crate::operation_scope::targets::page_object(conn, governor)
            .await
            .map_err(EngineError::unreadable)?;
        let expected = KeyFacet {
            operation: operation.operation().into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Object(ObjectKind::McpServer),
            target: source,
        };
        let admitted = caller.admitted_facets().ok_or_else(refused)?;
        if caller.admitted_required_roles().is_none()
            || !admitted.contains(&expected)
            || !caller.permits_facets(admitted)
        {
            return Err(refused());
        }
        let (source_home, source_org) = crate::copy_out::source_place(
            conn,
            crate::copy_out::Subject::Object(ObjectRef {
                kind: ObjectKind::McpServer,
                id: row.id,
            }),
        )
        .await
        .map_err(|error| match error {
            crate::copy_out::Error::Native(error) => error,
            _ => refused(),
        })?;
        let mut cleanup = Vec::new();
        for offering in copies {
            let command = Outgoing::new(
                orch_types::new_id(),
                offering.machine_id,
                SerialKey::Machine,
                &McpForget { server_id: row.id },
            )?;
            let proof = super::ForgetCommand::capture_recorded_deletion(
                conn, caller, offering, deleted_at, &command,
            )
            .await?;
            cleanup.push(Cleanup {
                id: command.id,
                machine: command.machine,
                proof,
            });
        }
        Ok(Self {
            origin: operation,
            server: row.id,
            fingerprint: super::duplicating::fingerprint(row)?,
            source_home,
            source_org,
            offerings: offering_stamp(conn, row.id).await?,
            home: row.home_id,
            org: row.org_id,
            name: row.name.clone(),
            deleted_at,
            audit_origin: serde_json::to_value(origin)?,
            cleanup,
        })
    }
    /// Factual original source association only; completion never reconstructs an invoker.
    pub async fn matches_source(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        let Some(row) = rows::servers(conn, &[self.server]).await?.pop() else {
            return Ok(false);
        };
        if super::duplicating::fingerprint(&row)? != self.fingerprint {
            return Ok(false);
        }
        let place = crate::copy_out::source_place(
            conn,
            crate::copy_out::Subject::Object(ObjectRef {
                kind: ObjectKind::McpServer,
                id: self.server,
            }),
        )
        .await
        .map_err(|error| match error {
            crate::copy_out::Error::Native(error) => error,
            _ => refused(),
        })?;
        Ok(place == (self.source_home, self.source_org)
            && offering_stamp(conn, self.server).await? == self.offerings)
    }
    /// Exact physical targets in the already selected native cleanup, without authority or names.
    pub fn copy_machines(&self) -> impl Iterator<Item = Id> + '_ {
        self.cleanup.iter().map(|copy| copy.machine)
    }
    /// Immutable original actor of this factual packet, not a restored caller.
    pub fn user(&self) -> Id {
        self.origin.user()
    }
    /// Closed actual Move purpose, distinct from an independently accepted Delete.
    pub fn moving(&self) -> bool {
        self.origin.operation() == "McpServerMoveOut"
    }
    /// The exact private source ending accepted by this original operation.
    pub fn server(&self) -> Id {
        self.server
    }
    /// Apply only in its caller's native transaction, after the current first-effect check.
    pub async fn apply(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        clock: &dyn Clock,
    ) -> Result<Vec<Outgoing>, EngineError> {
        let caller = principal.caller();
        if !orch_db::queries::transaction_guard::in_transaction(conn)
            || caller.user() != self.origin.user()
            || caller.admitted_operation() != Some(self.origin.operation())
            || caller.admitted_required_roles().is_none()
            || !caller.key_deadline_allows(clock)
        {
            return Err(refused());
        }
        let locked = rows::lock_update(conn, self.server, None).await?;
        if !orch_db::queries::transaction_guard::in_transaction(conn)
            || locked.is_none()
            || !self.matches_source(conn).await?
        {
            return Err(refused());
        }
        let row = locked.ok_or_else(refused)?;
        let governor = ObjectRef {
            kind: if row.environment_id.is_some() {
                ObjectKind::Environment
            } else {
                ObjectKind::McpServer
            },
            id: row.environment_id.unwrap_or(row.id),
        };
        let actual = crate::operation_scope::targets::page_object(conn, governor)
            .await
            .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().ok_or_else(refused)?;
        let required = KeyFacet {
            operation: self.origin.operation().into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Object(ObjectKind::McpServer),
            target: actual,
        };
        if !admitted.contains(&required) || !caller.permits_facets(admitted) {
            return Err(refused());
        }
        for cleanup in &self.cleanup {
            let target = crate::operation_scope::targets::machine_subject(conn, cleanup.machine)
                .await
                .map_err(EngineError::unreadable)?;
            let required = KeyFacet {
                operation: self.origin.operation().into(),
                action: KeyAction::Manage,
                resource_kind: KeyTargetKind::Object(ObjectKind::Machine),
                target,
            };
            if !admitted.contains(&required) {
                return Err(refused());
            }
        }
        rows::delete(conn, self.server, self.deleted_at.get()).await?;
        let action = orch_db::audit::action!(AuditAction::McpDeleted);
        let mut audit = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.origin.user()),
            action.as_str(),
            "mcp_server",
            Some(self.server),
            "Deleted an MCP server",
        );
        audit.history = action.is_history_line();
        audit.node_id = self.home;
        audit.org_id = self.org;
        audit.details = serde_json::json!({"server":self.server,"name":self.name});
        let origin = serde_json::from_value(self.audit_origin.clone())?;
        orch_db::audit::append(conn, &audit, &origin, self.deleted_at.get()).await?;
        let mut commands = Vec::new();
        for cleanup in &self.cleanup {
            let mut command = Outgoing::new(
                cleanup.id,
                cleanup.machine,
                SerialKey::Machine,
                &McpForget {
                    server_id: self.server,
                },
            )?;
            command.authority = Some(CommandAuthority::mcp_forget(cleanup.proof.clone()));
            command.deadline = Some(Time::new(
                clock.now() + chrono::Duration::seconds(i64::from(McpForget::KIND.deadline_secs())),
            ));
            let authority = command
                .authority
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            link::insert_command_with_authority(
                conn,
                &link::NewCommand {
                    id: command.id,
                    machine_id: command.machine,
                    kind: command.kind.as_str(),
                    body: &command.body,
                    serial_key: command.key.wire().as_deref(),
                    deadline_at: command.deadline.ok_or_else(refused)?.get(),
                },
                clock.now(),
                authority.as_ref(),
            )
            .await?;
            commands.push(command);
        }
        Ok(commands)
    }
}
