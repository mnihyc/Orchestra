//! The MCP servers of the harnesses' own configuration here: names reported, one definition copied into the connector's own directory when it is imported, started from that copy, and forgotten.
//!
//! A machine that has a harness set up usually has tool servers in that harness's own configuration. They
//! remain the program's own configuration for local execution. Discovery reports their names independently
//! of permission to import; importing and enabling adds a managed resource without editing the native file.
//!
//! * **Found.** The connector asks the adapter of each harness process it hosts that declares
//!   `mcp_discovery` for the servers its harness has configured, in its user's own configuration and in the
//!   environment's workspace, and reports each by name, harness, place and transport in the environment's
//!   facts ([`McpImports::rediscover`]). No command line, no address and no secret is reported. Import policy
//!   governs copies, while native names remain visible.
//! * **Imported.** One definition is read out of where it was found, through the adapter of that harness, and
//!   kept as a copy in the connector's own data directory ([`McpImports::import`]). Orch starts that copy
//!   (`mcp.start` with `local`); native execution keeps its own file, and `mcp.forget` deletes only the copy.
//!
//! The adapters are asked through the harness processes of the agent role, so a harness that is not
//! running here has nothing to say: the servers found are those of the harnesses the connector hosts.

use std::path::{Component, Path};
use std::sync::Arc;

use orch_agent_contract::discovery::{McpFound, McpImportRequest, McpScope, McpSearch};
use orch_envhost::environment::EnvHost;
use orch_link_contract::commands::{
    CommandError, ErrorCode, McpImport, McpImportResult, McpSnapshotTransfer, Sealed,
};
use orch_link_contract::events::EnvironmentFacts;
use orch_types::Id;

use crate::harnesses::Hosting;

/// Finds and imports the tool servers of the harnesses' configuration, for the environments of a host.
pub struct McpImports {
    host: EnvHost,
    hosting: Option<Arc<Hosting>>,
    keys: Arc<crate::roles::Keys>,
}

impl std::fmt::Debug for McpImports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpImports").finish_non_exhaustive()
    }
}

/// The scope of a configuration, as a command words it.
fn scope_of(word: &str) -> Result<McpScope, CommandError> {
    match word {
        "user" => Ok(McpScope::User),
        "directory" => Ok(McpScope::Directory),
        other => Err(CommandError::new(
            ErrorCode::Unsupported,
            format!("{other:?} is no place a harness configures tool servers in"),
        )),
    }
}

/// Native paths stay local; the environment advertises only workspace-relative project names.
pub(crate) fn in_workspace(mut found: McpFound, root: &Path) -> Option<McpFound> {
    if found.scope == McpScope::Directory {
        let directory = Path::new(found.directory.as_deref()?);
        let relative = if directory.is_absolute() {
            directory.strip_prefix(root).ok()?
        } else {
            directory
        };
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return None;
        }
        found.directory = Some(relative.to_string_lossy().replace('\\', "/"));
    }
    Some(found)
}

fn native_directory(
    root: &Path,
    scope: &McpScope,
    directory: Option<&str>,
) -> Result<Option<String>, CommandError> {
    if *scope == McpScope::User {
        return Ok(None);
    }
    let directory = directory.ok_or_else(|| {
        CommandError::new(
            ErrorCode::Unsupported,
            "a project import names its workspace directory",
        )
    })?;
    let path = Path::new(directory);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CommandError::new(
            ErrorCode::Unsupported,
            "a project import stays within its environment workspace",
        ));
    }
    Ok(Some(root.join(path).to_string_lossy().into_owned()))
}

impl McpImports {
    /// Imports over the environments of `host`, asking the harness processes of `hosting` (a machine that
    /// hosts none finds nothing).
    #[must_use]
    pub fn new(
        host: EnvHost,
        hosting: Option<Arc<Hosting>>,
        keys: Arc<crate::roles::Keys>,
    ) -> Self {
        Self {
            host,
            hosting,
            keys,
        }
    }

    /// The servers the harnesses here have configured, by name: those of their users' own configuration and
    /// those of the `directories` given, each once.
    pub async fn found(&self, directories: &[String]) -> Vec<McpFound> {
        let Some(hosting) = &self.hosting else {
            return Vec::new();
        };
        let mut found: Vec<McpFound> = Vec::new();
        for process in hosting.with_mcp_discovery() {
            let search = McpSearch {
                directories: directories.to_vec(),
            };
            match process
                .call(move |adapter| adapter.discover_mcp(search))
                .await
            {
                Ok(listed) => {
                    for mut server in listed {
                        server.profile = Some(orch_agent_contract::discovery::McpProfile {
                            provider_id: process.provider_id,
                            agent_id: process.agent_id,
                        });
                        if !found.contains(&server) {
                            found.push(server);
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, kind = %process.kind, "a harness could not say which tool servers it has configured");
                }
            }
        }
        found
    }

    /// Looks again at what the harnesses here have configured, for one environment, keeps it in the
    /// environment's facts and says the facts if they changed. Import restrictions do not hide native names.
    pub async fn rediscover(&self, environment_id: Id) -> Result<EnvironmentFacts, CommandError> {
        let environment = self.host.environment(environment_id)?;
        let root = environment.facts().workspace_root;
        let mut directories = vec![root.clone()];
        if let Some(hosting) = &self.hosting {
            directories.extend(hosting.mcp_directories(environment_id, &root));
        }
        directories.sort();
        directories.dedup();
        let found = self
            .found(&directories)
            .await
            .into_iter()
            .filter_map(|found| in_workspace(found, Path::new(&root)))
            .collect();
        environment.mcp().set_found(found);
        Ok(self.host.facts_changed(environment_id).await?)
    }

    /// The agent role's session changes, shared without retaining the environment role itself.
    pub fn changes(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.hosting.as_ref().map(|hosting| hosting.mcp_changes())
    }

    /// `mcp.import`: reads the definition of one server out of the harness's configuration, through that
    /// harness's adapter, and keeps a copy of it on this machine. The definition is not in the answer.
    pub async fn import(&self, body: &McpImport) -> Result<McpImportResult, CommandError> {
        if let Some(transfer) = &body.transfer {
            match transfer {
                McpSnapshotTransfer::Issue { recipient } => {
                    let (definition, mut reply) = self.host.mcp_transfer_read(body)?;
                    let bytes = serde_json::to_string(&definition).map_err(|_| {
                        CommandError::new(
                            ErrorCode::Internal,
                            "the native MCP snapshot could not be encoded",
                        )
                    })?;
                    reply.snapshot = Some(Sealed {
                        value: self.keys.seal_with(
                            &recipient.public_key,
                            &transfer_info(body, recipient.machine_id)?,
                            &bytes,
                        )?,
                    });
                    return Ok(reply);
                }
                McpSnapshotTransfer::Receive {
                    recipient,
                    snapshot,
                } => {
                    let text = self
                        .keys
                        .open_with(&transfer_info(body, *recipient)?, &snapshot.value)?;
                    let definition = serde_json::from_str(&text).map_err(|_| {
                        CommandError::new(ErrorCode::Internal, "the sealed MCP snapshot is invalid")
                    })?;
                    return Ok(self.host.mcp_transfer_receive(body, &definition)?);
                }
            }
        }
        if body.stored.is_some() {
            return Ok(self.host.mcp_stored_copy(body)?);
        }
        let environment_id = body.environment_id.ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "an offering import needs its actual environment",
            )
        })?;
        let environment = self.host.environment(environment_id)?;
        if !environment.mcp().import_allowed() {
            return Err(orch_envhost::error::HostError::McpImportDisabled.into());
        }
        let scope = scope_of(&body.scope)?;
        let request = McpImportRequest {
            server_id: body.server_id,
            source: body.source.clone(),
            scope,
            name: body.name.clone(),
            directory: native_directory(
                Path::new(&environment.facts().workspace_root),
                &scope,
                body.directory.as_deref(),
            )?,
        };
        let Some(hosting) = &self.hosting else {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "no harness is hosted on this machine to read the definition from",
            ));
        };
        let mut reason = format!(
            "no harness of the kind {} here has configured a server called {}",
            body.source, body.name
        );
        for process in hosting.with_mcp_discovery().into_iter().filter(|process| {
            process.kind == body.source
                && body.profile.is_none_or(|profile| {
                    profile.provider_id == process.provider_id
                        && profile.agent_id == process.agent_id
                })
        }) {
            let asked = request.clone();
            match process.call(move |adapter| adapter.read_mcp(asked)).await {
                Ok(definition) => {
                    return Ok(self.host.mcp_import(body, &definition, None)?);
                }
                Err(error) if error.code == ErrorCode::NotFound.as_str() => {
                    reason = error.detail;
                }
                Err(error) => return Err(error),
            }
        }
        Err(CommandError::new(ErrorCode::NotFound, reason))
    }
}

fn transfer_info(body: &McpImport, recipient: Id) -> Result<Vec<u8>, CommandError> {
    let source = body.stored.as_ref().ok_or_else(|| {
        CommandError::new(
            ErrorCode::Unsupported,
            "a sealed copy needs its exact stored native source",
        )
    })?;
    serde_json::to_vec(&("orch/mcp/stored-copy/v1", source, body.server_id, recipient)).map_err(
        |_| {
            CommandError::new(
                ErrorCode::Internal,
                "the native MCP snapshot purpose could not be encoded",
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn project_imports_return_to_the_exact_native_directory_named_by_discovery() {
        let root = Path::new("/workspace");
        for relative in ["", "project", "project/api"] {
            let absolute = root.join(relative);
            let found = McpFound {
                profile: None,
                definition_fingerprint: None,
                name: "docs".into(),
                source: "codex".into(),
                scope: McpScope::Directory,
                directory: Some(absolute.to_string_lossy().into_owned()),
                transport: orch_agent_contract::discovery::McpTransport::Stdio,
                digest: "synthetic-definition".into(),
            };
            let Some(found) = in_workspace(found, root) else {
                panic!("the discovery belongs to this workspace")
            };
            assert_eq!(found.directory.as_deref(), Some(relative));
            let Ok(Some(directory)) =
                native_directory(root, &McpScope::Directory, found.directory.as_deref())
            else {
                panic!("a discovered project imports from its actual native directory")
            };
            assert_eq!(Path::new(&directory), absolute);
        }
        assert!(native_directory(root, &McpScope::Directory, Some("../outside")).is_err());
        assert!(native_directory(root, &McpScope::Directory, None).is_err());
        assert!(matches!(
            native_directory(root, &McpScope::User, None),
            Ok(None)
        ));
    }
}
