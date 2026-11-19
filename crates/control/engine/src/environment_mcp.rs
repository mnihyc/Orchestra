//! Native environment MCP policy and machine-owner/delegation prerequisites.

use crate::EngineError;
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{Permission, ResourceKind},
};
use orch_authz::{
    Policy, Principal,
    resources::{KeyAbout, Keying},
};
use orch_db::{
    Conn,
    queries::{environment_facts, environments::hosting, resources},
};
use orch_link_contract::policy::{LocalPolicy, PolicyKey};
use orch_types::{Id, Json};

mod copy_command;
pub use copy_command::CopyCommand;
mod forget_command;
pub use forget_command::{ForgetCommand, ForgetCustody, RecordedDeletionCustody};
/// Native copy input and the original-operation offering checkpoints.
pub mod importing;

/// Native policy lookup retains a missing environment separately from internal failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The actual live environment is absent.
    #[error("the environment is not available")]
    NotFound,
    /// The native import scope/directory input is invalid.
    #[error("{message}")]
    Invalid {
        /// Native request field.
        field: &'static str,
        /// Existing native explanation.
        message: &'static str,
    },
    /// The stored environment has no connected machine for this copy.
    #[error("the environment has no connected machine")]
    Offline,
    /// Its native facts or authority could not be read.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

/// The actual selected stored offering, not a definition or permission inferred from output JSON.
pub struct ImportSelection {
    environment: hosting::EnvironmentState,
    machine: Id,
    source: String,
    api_scope: Json,
    machine_scope: String,
    directory: Option<String>,
    name: String,
    offering: Json,
}
impl ImportSelection {
    /// The exact native source association, before any connector output exists.
    fn fingerprint(&self) -> Result<orch_types::Hash, EngineError> {
        crate::jobs::command_identity::hash(&serde_json::json!({
            "environment": self.environment.id,
            "org": self.environment.org_id,
            "owner": self.environment.owner_id,
            "home": self.environment.home_node_id,
            "placement": self.environment.placement,
            "workspace": self.environment.workspace_root,
            "deleted": self.environment.deleted_at,
            "generation": self.environment.generation,
            "machine": self.machine,
            "source": self.source,
            "scope": self.api_scope,
            "directory": self.directory,
            "name": self.name,
            "offering": self.offering,
        }))
    }
    /// Actual parent namespace of this import.
    pub fn environment(&self) -> &hosting::EnvironmentState {
        &self.environment
    }
    /// Actual recipient of its native copy.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// Exact private command input, with its newly allocated native server identity.
    pub fn command(&self, server: Id) -> orch_link_contract::commands::McpImport {
        orch_link_contract::commands::McpImport {
            expected_fingerprint: None,
            environment_id: self.environment.id,
            server_id: server,
            source: self.source.clone(),
            scope: self.machine_scope.clone(),
            name: self.name.clone(),
            directory: self.directory.clone(),
        }
    }
    /// The original API scope word recorded beside the native result.
    pub fn api_scope(&self) -> &Json {
        &self.api_scope
    }
    /// Original source key recorded by the native copy.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Original offering name recorded by the native copy.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Original directory selection, absent for user scope.
    pub fn directory(&self) -> Option<&str> {
        self.directory.as_deref()
    }
    /// Immutable native source identity across later waits.
    pub fn preserves(&self, fresh: &Self) -> bool {
        self.environment.id == fresh.environment.id
            && self.environment.org_id == fresh.environment.org_id
            && self.environment.owner_id == fresh.environment.owner_id
            && self.environment.home_node_id == fresh.environment.home_node_id
            && self.environment.placement == fresh.environment.placement
            && self.environment.workspace_root == fresh.environment.workspace_root
            && self.environment.deleted_at == fresh.environment.deleted_at
            && self.environment.generation == fresh.environment.generation
            && self.machine == fresh.machine
            && self.source == fresh.source
            && self.api_scope == fresh.api_scope
            && self.machine_scope == fresh.machine_scope
            && self.directory == fresh.directory
            && self.name == fresh.name
            && self.offering == fresh.offering
    }
}

/// The native scope conversion, directory validation and stored offering selection before IO.
pub async fn select_import(
    conn: &mut Conn,
    environment: Id,
    source: &str,
    scope: Json,
    directory: Option<String>,
    name: &str,
) -> Result<ImportSelection, Error> {
    let env = hosting::environment(conn, environment)
        .await?
        .ok_or(Error::NotFound)?;
    let machine = env.machine_id.ok_or(Error::Offline)?;
    let scope_word = scope.as_str().ok_or(Error::Invalid {
        field: "scope",
        message: "the import scope is not known",
    })?;
    let machine_scope = match scope_word {
        "project" => "directory",
        "user" => "user",
        _ => {
            return Err(Error::Invalid {
                field: "scope",
                message: "the import scope is not known",
            });
        }
    }
    .to_owned();
    if scope_word == "project"
        && directory
            .as_deref()
            .is_none_or(|directory| !crate::binding::directory_is_valid(directory))
    {
        return Err(Error::Invalid {
            field: "directory",
            message: "a directory import names a relative workspace directory",
        });
    }
    if scope_word == "user" && directory.is_some() {
        return Err(Error::Invalid {
            field: "directory",
            message: "a user's configuration has no workspace directory",
        });
    }
    let offering = env
        .facts
        .get("mcp_found")
        .and_then(Json::as_array)
        .and_then(|found| {
            found.iter().find(|found| {
                found["source"] == source
                    && found["scope"] == machine_scope
                    && found["name"] == name
                    && found["directory"].as_str() == directory.as_deref()
            })
        })
        .cloned()
        .ok_or(Error::NotFound)?;
    Ok(ImportSelection {
        environment: env,
        machine,
        source: source.to_owned(),
        api_scope: scope,
        machine_scope,
        directory,
        name: name.to_owned(),
        offering,
    })
}
impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// The same stored environment/machine/policy tuple used by ordinary native MCP requests.
pub async fn machine_policy(
    conn: &mut Conn,
    environment: Id,
) -> Result<
    (
        hosting::EnvironmentState,
        Option<resources::MachineRow>,
        LocalPolicy,
    ),
    Error,
> {
    let env = hosting::environment(conn, environment)
        .await?
        .filter(|env| env.deleted_at.is_none())
        .ok_or(Error::NotFound)?;
    let machine = match env.machine_id {
        Some(machine) => resources::machine(conn, machine).await?,
        None => None,
    };
    let policy = environment_facts::of(conn, environment)
        .await?
        .and_then(|facts| facts.policy)
        .filter(|policy| policy.is_object())
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)?
        .unwrap_or_default();
    Ok((env, machine, policy))
}

/// Existing Environment management plus the machine owner's MCP-import key or delegation.
/// Scope admission supplies no ordinary authority here.
pub async fn may_import(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
) -> Result<bool, Error> {
    let holding = orch_authz::resources::effective(
        conn,
        principal,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: environment,
        },
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?;
    if !holding
        .held
        .iter()
        .any(|held| held.permission == Permission::EnvironmentManage)
    {
        return Ok(false);
    }
    let (_, _, policy) = machine_policy(conn, environment).await?;
    if !policy.mcp_import {
        return Ok(false);
    }
    let Some(machine) = environment_facts::of(conn, environment)
        .await?
        .and_then(|facts| facts.machine_id)
    else {
        return Ok(false);
    };
    let Some(machine) = resources::machine(conn, machine).await? else {
        return Ok(false);
    };
    if machine.owner_id == Some(principal.user()) && principal.caller().reaches(None) {
        return Ok(true);
    }
    Ok(matches!(
        orch_authz::resources::may_change_policy_key(
            conn,
            principal,
            machine.id,
            KeyAbout::Environments,
            policy.delegates(PolicyKey::McpImport),
            Policy::default(),
        )
        .await
        .map_err(EngineError::from)?,
        Keying::Allowed
    ))
}
