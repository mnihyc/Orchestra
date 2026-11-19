//! The MCP servers of the harnesses' own configuration here: names reported, one definition copied into the connector's own directory when it is imported, started from that copy, and forgotten.
//!
//! A machine that has a harness set up usually has tool servers in that harness's own configuration. They
//! are not configured a second time and they are not used unasked: a server is **found**, then
//! **imported**, then enabled, and only the last step makes an agent act with it.
//!
//! * **Found.** The connector asks the adapter of each harness process it hosts that declares
//!   `mcp_discovery` for the servers its harness has configured, in its user's own configuration and in the
//!   environment's workspace, and reports each by name, harness, place and transport in the environment's
//!   facts ([`McpImports::rediscover`]). No command line, no address and no secret is reported. Where the
//!   machine's policy lets none be imported, none is reported.
//! * **Imported.** One definition is read out of where it was found, through the adapter of that harness, and
//!   kept as a copy in the connector's own data directory ([`McpImports::import`]). From then on the copy is
//!   what runs (`mcp.start` with `local`), whatever becomes of the harness's file; `mcp.forget` deletes it.
//!
//! The adapters are asked through the harness processes of the agent role, so a harness that is not
//! running here has nothing to say: the servers found are those of the harnesses the connector hosts.

use std::sync::Arc;

use orch_agent_contract::discovery::{McpFound, McpImportRequest, McpScope, McpSearch};
use orch_envhost::environment::EnvHost;
use orch_link_contract::commands::{CommandError, ErrorCode, McpImport, McpImportResult};
use orch_link_contract::events::EnvironmentFacts;
use orch_types::Id;

use crate::harnesses::Hosting;

/// Finds and imports the tool servers of the harnesses' configuration, for the environments of a host.
pub struct McpImports {
    host: EnvHost,
    hosting: Option<Arc<Hosting>>,
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

impl McpImports {
    /// Imports over the environments of `host`, asking the harness processes of `hosting` (a machine that
    /// hosts none finds nothing).
    #[must_use]
    pub fn new(host: EnvHost, hosting: Option<Arc<Hosting>>) -> Self {
        Self { host, hosting }
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
                    for server in listed {
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
    /// environment's facts and says the facts if they changed. Where the machine lets no tool server be
    /// imported nothing is reported.
    pub async fn rediscover(&self, environment_id: Id) -> Result<EnvironmentFacts, CommandError> {
        let environment = self.host.environment(environment_id)?;
        let allowed = environment.mcp().import_allowed();
        let found = if allowed {
            let root = environment.facts().workspace_root;
            self.found(&[root]).await
        } else {
            Vec::new()
        };
        environment.mcp().set_found(found);
        Ok(self.host.facts_changed(environment_id).await?)
    }

    /// `mcp.import`: reads the definition of one server out of the harness's configuration, through that
    /// harness's adapter, and keeps a copy of it on this machine. The definition is not in the answer.
    pub async fn import(&self, body: &McpImport) -> Result<McpImportResult, CommandError> {
        let environment = self.host.environment(body.environment_id)?;
        if !environment.mcp().import_allowed() {
            return Err(orch_envhost::error::HostError::McpImportDisabled.into());
        }
        let request = McpImportRequest {
            server_id: body.server_id,
            source: body.source.clone(),
            scope: scope_of(&body.scope)?,
            name: body.name.clone(),
            directory: body.directory.clone(),
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
        for process in hosting
            .with_mcp_discovery()
            .into_iter()
            .filter(|process| process.kind == body.source)
        {
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
