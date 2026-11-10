//! The actual environment offering and creation parent across native copy waits.

use super::{ImportSelection, may_import, select_import};
use crate::EngineError;
use crate::operation_scope::{
    continuing::{Error, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    environment::EnvironmentMcpImportCreate,
    environment_sources::{MCP_IMPORT, McpCreateSource as Role},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError, queries::resources};
use orch_types::{Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Actual native inputs and source identity; none of these fields is a permission.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    machine: Id,
    owner: Option<Id>,
    scope: Json,
    fingerprint: Hash,
    environment: Id,
    command: orch_link_contract::commands::McpImport,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpImportInput([native])")
    }
}
impl Input {
    /// Freeze the already selected native offering and newly allocated copy identity.
    /// The physical machine's owner is retained as a source-change guard, not as the new resource's owner.
    pub async fn capture(
        conn: &mut Conn,
        selected: &ImportSelection,
        server: Id,
    ) -> Result<Self, EngineError> {
        let machine = resources::machine(conn, selected.machine())
            .await?
            .ok_or_else(|| EngineError::unreadable("the import machine is absent"))?;
        Ok(Self {
            machine: selected.machine(),
            owner: machine.owner_id,
            scope: selected.api_scope().clone(),
            fingerprint: selected.fingerprint()?,
            environment: selected.environment().id,
            command: selected.command(server),
        })
    }
    /// Actual creation namespace.
    pub fn environment(&self) -> Id {
        self.environment
    }
    /// Actual native connector recipient.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// The new server owned by this one native copy, never a pre-existing Read governor.
    pub fn server(&self) -> Id {
        self.command.server_id
    }
    /// Exact original native command; authority stays out of this machine body.
    pub fn command(&self) -> orch_link_contract::commands::McpImport {
        self.command.clone()
    }
    /// Preserve the original native Environment lane for the definition copy.
    pub fn serial(&self) -> orch_link_contract::commands::SerialKey {
        orch_link_contract::commands::SerialKey::Environment(self.environment())
    }

    /// Reuse a retained copy only for the same immutable native request.
    pub fn matches_request(
        &self,
        environment: Id,
        source: &str,
        scope: &Json,
        directory: Option<&str>,
        name: &str,
    ) -> bool {
        self.environment() == environment
            && self.command.source == source
            && &self.scope == scope
            && self.command.directory.as_deref() == directory
            && self.command.name == name
    }
}

fn native_error(error: super::Error) -> Error {
    match error {
        super::Error::NotFound | super::Error::Invalid { .. } | super::Error::Offline => {
            Error::Refused
        }
        super::Error::Internal(error) => Error::Native(error),
    }
}

/// HTTP and native command checkpoints consume this same exact offering producer.
pub struct Offering;
impl Source for Offering {
    type Role = Role;
    type Input = Input;
    type Value = ImportSelection;
    const ROUTE: Route = EnvironmentMcpImportCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &MCP_IMPORT;

    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(ImportSelection, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let selected = select_import(
                conn,
                input.environment(),
                &input.command.source,
                input.scope.clone(),
                input.command.directory.clone(),
                &input.command.name,
            )
            .await
            .map_err(native_error)?;
            let machine = resources::machine(conn, input.machine)
                .await?
                .ok_or(Error::Refused)?;
            if selected.machine() != input.machine
                || selected.fingerprint()? != input.fingerprint
                || machine.owner_id != input.owner
                || selected.command(input.server()) != input.command
                || resources::mcp::recorded(conn, input.server()).await?
            {
                return Err(Error::Refused);
            }
            let parent = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: input.environment(),
                },
            )
            .await
            .map_err(|error| match error {
                targets::TargetError::Missing => Error::Refused,
                targets::TargetError::Db(error) => Error::from(error),
                targets::TargetError::Authz(error) => Error::from(error),
            })?;
            Ok((
                selected,
                vec![
                    Facts::one(
                        Role::Environment,
                        KeyTargetKind::Object(ObjectKind::Environment),
                        parent.clone(),
                    ),
                    Facts::one(
                        Role::Parent,
                        KeyTargetKind::Object(ObjectKind::McpServer),
                        parent,
                    ),
                ],
            ))
        })
    }

    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _: &'a ImportSelection,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !may_import(conn, principal, input.environment())
                .await
                .map_err(native_error)?
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }

    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a ImportSelection,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(input.environment())
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.machine)
                .execute(conn)
                .await
                .map_err(DbError::from)?;
            Ok(())
        })
    }

    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a ImportSelection,
        fresh: &'a ImportSelection,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened.preserves(fresh)) })
    }
}
