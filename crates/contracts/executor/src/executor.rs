//! Executor lifecycle and loopback endpoints.
//!
//! An executor is what carries out a harness's tool calls where the work is: commands and file edits. It
//! runs in the environment, as part of the environment's exec node: the node starts it, it hands back a
//! **loopback address**, and the connector serves the harness's `exec` tunnels by connecting to that
//! address. A harness that runs in the environment carries out its own tool calls and asks nothing of it.
//!
//! The trait is [`Executor`]. An executor keeps nothing between sessions: one serves the sessions of its
//! environment, and any number of them run on one host at once, each under the directory it is given.
//! Orch's own tools, the `orch` server, are beside every executor and in none, so an executor cannot leave
//! them out, change them, or see the calls made to them.
//!
//! Over the plugin transport the methods are `describe`, `available`, `start` and `stop`; all four are
//! required.

use async_trait::async_trait;
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::SandboxMode;
use orch_plugin_contract::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::EnvironmentContext;
use crate::descriptor::{ExecutorAvailability, ExecutorDescriptor};

/// Where a started executor can be reached, and what it reports of itself there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorEndpoint {
    /// The loopback address tunnels connect to: `127.0.0.1:<port>`, optionally with a scheme such as `ws://`.
    /// The port is one the system gave it, never a fixed one.
    pub address: String,
    /// A token the connection has to present, where the executor asks for one.
    #[serde(default)]
    pub token: Option<Secret>,
    /// The build it is.
    pub version: String,
    /// The tool sets it serves, for an executor that has more than one.
    #[serde(default)]
    pub tool_sets: Vec<String>,
    /// What the harness's sandbox can enforce here, probed at start; only what works.
    #[serde(default)]
    pub sandbox_modes: Vec<SandboxMode>,
}

/// Why an address is not an executor's loopback address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    /// It is not `host:port`.
    #[error("{0:?} is not host:port")]
    Form(String),
    /// It names a host other than the machine itself.
    #[error("an executor is reached on the loopback address only, and {0:?} is not")]
    NotLoopback(String),
    /// The port is no port, or zero: a port the system gave is never zero.
    #[error("{0:?} is no port the system gave")]
    Port(String),
}

/// The hosts that mean the machine itself.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

impl ExecutorEndpoint {
    /// The port of the address, when the address is on the machine itself: `127.0.0.1`, `localhost` or
    /// `[::1]`, with a scheme or without, and a port that is not zero.
    pub fn port(&self) -> Result<u16, EndpointError> {
        let rest = self
            .address
            .split_once("://")
            .map_or(self.address.as_str(), |(_, rest)| rest);
        let authority = rest
            .split_once('/')
            .map_or(rest, |(authority, _)| authority);
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| EndpointError::Form(self.address.clone()))?;
        if !LOOPBACK_HOSTS.contains(&host) {
            return Err(EndpointError::NotLoopback(host.to_owned()));
        }
        match port.parse::<u16>() {
            Ok(port) if port != 0 => Ok(port),
            _ => Err(EndpointError::Port(port.to_owned())),
        }
    }
}

/// One implementation per executor, used by the environment role of the connector.
#[async_trait]
pub trait Executor: Send + 'static {
    /// Its kind: `codex-exec-server`, `orch-exec`, or the name of the package it comes in.
    fn kind(&self) -> &'static str;

    /// What it declares about itself: whether it speaks a protocol of its harness's own or offers its tools
    /// as an MCP server; its tool sets, when it has more than one; and the variables that carry thread and
    /// tool-call ids. An executor keeps nothing between sessions, and any number of them run on one host at
    /// once.
    fn describe(&self) -> ExecutorDescriptor;

    /// Whether it can be started in this environment now, which of its tool sets it can offer, and which
    /// sandbox modes its harness can enforce there.
    async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability;

    /// Starts the executor inside the environment and returns the loopback address tunnels connect to.
    async fn start(&mut self, env: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError>;

    /// Stops it, and everything it started.
    async fn stop(&mut self) -> Result<(), AdapterError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use orch_agent_contract::descriptor::SandboxMode;
    use serde_json::json;

    fn endpoint(address: &str) -> ExecutorEndpoint {
        ExecutorEndpoint {
            address: address.to_owned(),
            token: None,
            version: "0.1.0".into(),
            tool_sets: vec![],
            sandbox_modes: vec![],
        }
    }

    #[test]
    fn an_executor_is_reached_on_the_loopback_address_and_a_port_the_system_gave() {
        assert_eq!(endpoint("127.0.0.1:41231").port(), Ok(41231));
        assert_eq!(endpoint("ws://127.0.0.1:41231/exec").port(), Ok(41231));
        assert_eq!(endpoint("localhost:7000").port(), Ok(7000));
        assert_eq!(endpoint("http://[::1]:7000").port(), Ok(7000));
        assert_eq!(
            endpoint("10.0.0.5:7000").port(),
            Err(EndpointError::NotLoopback("10.0.0.5".into()))
        );
        assert_eq!(
            endpoint("0.0.0.0:7000").port(),
            Err(EndpointError::NotLoopback("0.0.0.0".into()))
        );
        assert_eq!(
            endpoint("127.0.0.1:0").port(),
            Err(EndpointError::Port("0".into()))
        );
        assert_eq!(
            endpoint("127.0.0.1:http").port(),
            Err(EndpointError::Port("http".into()))
        );
        assert_eq!(
            endpoint("127.0.0.1").port(),
            Err(EndpointError::Form("127.0.0.1".into()))
        );
    }

    #[test]
    fn a_started_executor_reports_where_it_is_what_it_is_and_what_its_sandbox_can_do_here() {
        let started: ExecutorEndpoint = serde_json::from_value(json!({
            "address": "127.0.0.1:41231", "version": "0.1.0", "tool_sets": ["plain", "claude-code"],
            "sandbox_modes": ["open"], "token": "exec-token"
        }))
        .unwrap();
        assert_eq!(started.sandbox_modes, [SandboxMode::Open]);
        assert!(!format!("{started:?}").contains("exec-token"));
        let bare: ExecutorEndpoint =
            serde_json::from_value(json!({ "address": "127.0.0.1:1", "version": "1" })).unwrap();
        assert!(bare.token.is_none() && bare.tool_sets.is_empty());
    }

    /// An executor that cannot be started where it is asked to: the build its harness needs is not there.
    struct Missing;

    #[async_trait]
    impl Executor for Missing {
        fn kind(&self) -> &'static str {
            "needs-a-build"
        }
        fn describe(&self) -> ExecutorDescriptor {
            serde_json::from_value(json!({
                "kind": "needs-a-build", "title": "Needs a build", "protocol": "harness", "sandbox_modes": ["open"]
            }))
            .unwrap()
        }
        async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
            match env.tool("harness") {
                Some(tool) => {
                    ExecutorAvailability::available(&tool.version, vec![SandboxMode::Open])
                }
                None => {
                    ExecutorAvailability::unavailable("the harness build is not in the tool cache")
                }
            }
        }
        async fn start(
            &mut self,
            env: &EnvironmentContext,
        ) -> Result<ExecutorEndpoint, AdapterError> {
            let availability = self.available(env).await;
            if availability.available {
                Ok(endpoint("127.0.0.1:41999"))
            } else {
                Err(AdapterError::HarnessUnavailable(
                    availability.reason.unwrap_or_default(),
                ))
            }
        }
        async fn stop(&mut self) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    #[test]
    fn an_executor_that_cannot_start_in_an_environment_says_why_and_refuses_to_start() {
        let env: EnvironmentContext = serde_json::from_value(json!({
            "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 1, "data_dir": "/d",
            "workspace_root": "/w", "home": null, "tools_dir": null, "tools": [], "mounts": [], "user": "agent",
            "shell": { "name": "sh", "path": "/bin/sh" }, "os": "linux", "arch": "aarch64", "path_style": "posix",
            "limits": {}, "variables": {}, "mcp_servers_enabled": false
        }))
        .unwrap();
        let mut executor = Missing;
        let availability = block_on(executor.available(&env));
        assert!(!availability.available);
        assert_eq!(executor.describe().check(), Ok(()));
        assert!(matches!(
            block_on(executor.start(&env)),
            Err(AdapterError::HarnessUnavailable(_))
        ));
        assert_eq!(block_on(executor.stop()), Ok(()));
    }
}
