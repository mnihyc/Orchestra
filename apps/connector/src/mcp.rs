//! `mcp`: the tool servers the harnesses on this machine have configured, as the running connector finds them.
//!
//! The connector that runs on the data directory is asked over its local socket: it holds the harnesses and the
//! environments whose configuration it reads. `list` shows each server by name, harness, place and transport, and
//! whether a copy of it is kept for Orch; `forget` deletes such a copy. `import` asks the system over the
//! running connector's authenticated link, as the machine's owner. The system files and audits the copy,
//! then uses the same machine command as an import from the Tools page. A refusal is printed.

use std::path::Path;

use orch_connector_core::local_socket::{Reply, Request, ask};

use crate::arguments::Mcp;

/// Carries out `command` for the connector running on `dir`, and says what came of it.
pub async fn mcp(dir: &Path, command: &Mcp) -> anyhow::Result<String> {
    match command {
        Mcp::List => match ask(dir, &Request::McpList).await? {
            Reply::Mcp { servers } if servers.is_empty() => Ok(
                "No tool server is configured in the harnesses this connector holds, or none may be imported \
                 here (`mcp.import`)."
                    .to_owned(),
            ),
            Reply::Mcp { servers } => Ok(servers
                .iter()
                .map(|server| {
                    format!(
                        "{:<24} {:<12} {:<10} {}{}",
                        server.name,
                        server.harness,
                        server.transport,
                        server.place,
                        if server.imported { "  (imported)" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")),
            other => Err(unexpected(&other)),
        },
        Mcp::Import { name, from } => match ask(dir,&Request::McpImport{name:name.clone(),from:from.clone(),place:None}).await? {
            Reply::Done{said}=>Ok(said),
            other=>Err(unexpected(&other)),
        },
        Mcp::Forget { name } => {
            match ask(
                dir,
                &Request::McpForget {
                    name: name.clone(),
                    from: None,
                },
            )
            .await?
            {
                Reply::Done { said } => Ok(said),
                Reply::Error { message } => anyhow::bail!(message),
                other => Err(unexpected(&other)),
            }
        }
    }
}

fn unexpected(reply: &Reply) -> anyhow::Error {
    match reply {
        Reply::Error { message } => anyhow::anyhow!("{message}"),
        other => anyhow::anyhow!("the connector answered something else: {other:?}"),
    }
}
