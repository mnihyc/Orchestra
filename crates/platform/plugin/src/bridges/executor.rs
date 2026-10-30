//! External Executor bridge.
//!
//! [`ExecutorBridge`] is an [`Executor`] whose implementation is a program. An executor starts something inside
//! an environment that speaks the protocol a harness expects, and hands back a loopback address. It is part of the
//! environment's exec node, and every environment of a host has an exec node of its own: the program keeps
//! everything it has under the directory it is given, listens on ports the system gives it, and shares nothing with
//! another of itself, so any number run on one host at once.
//!
//! | Method | Required | Parameters | Result |
//! |--------|----------|------------|--------|
//! | `describe` | yes | none | the executor's descriptor |
//! | `available` | yes | the environment context, which is the parameters | whether it can be started there now, and what it can offer |
//! | `start` | yes | the environment context | the endpoint: the loopback `address`, a `token` if one is needed, the `version`, the `tool_sets` and `sandbox_modes` |
//! | `stop` | yes | none | none |
//!
//! The address a program reports is held to the rule of the contract before it is passed on: it is on the machine
//! itself, and its port is one the system gave. An executor that reports another is refused, and stopped again.
//! What goes wrong is told in the harness vocabulary of the adapter's errors, since an executor is started on an
//! adapter's behalf; any code a program uses that the vocabulary does not know is passed on as it is.
//!
//! An [`ExecutorFactory`] makes an executor for each environment: the connector asks it for one when it starts an
//! environment's exec node.

use async_trait::async_trait;
use orch_agent_contract::adapter::AdapterError;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{ExecutorAvailability, ExecutorDescriptor};
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_plugin_contract::PluginKind;
use orch_types::Json;

use super::{BridgeError, Described, adapter_error, interned, params, result};
use crate::supervisor::Plugin;

const AVAILABLE: &str = "available";
const START: &str = "start";
const STOP: &str = "stop";

/// An executor that is a program.
pub struct ExecutorBridge {
    plugin: Plugin,
    descriptor: Described<ExecutorDescriptor>,
    kind: &'static str,
}

impl ExecutorBridge {
    /// The executor that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let descriptor =
            Described::<ExecutorDescriptor>::new(plugin.clone(), PluginKind::Executor)?;
        let kind = interned(&descriptor.get().kind);
        Ok(Self {
            plugin,
            descriptor,
            kind,
        })
    }
}

#[async_trait]
impl Executor for ExecutorBridge {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn describe(&self) -> ExecutorDescriptor {
        (*self.descriptor.get()).clone()
    }

    async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
        let answer = async {
            let sent = params(AVAILABLE, env)?;
            result::<ExecutorAvailability>(AVAILABLE, self.plugin.call(AVAILABLE, sent).await?)
        };
        match answer.await {
            Ok(availability) => availability,
            Err(error) => ExecutorAvailability::unavailable(&error.to_string()),
        }
    }

    async fn start(&mut self, env: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        let answer = async {
            let sent = params(START, env)?;
            result::<ExecutorEndpoint>(START, self.plugin.call(START, sent).await?)
        };
        let endpoint = answer.await.map_err(adapter_error)?;
        if let Err(fault) = endpoint.port() {
            // Whatever it started is of no use at an address that cannot be trusted; it is stopped again.
            let _ = self.stop().await;
            return Err(AdapterError::Internal(format!(
                "the executor reported an address that cannot be used: {fault}"
            )));
        }
        Ok(endpoint)
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.plugin
            .call(STOP, Json::Null)
            .await
            .map(|_| ())
            .map_err(adapter_error)
    }
}

/// What makes an executor for an environment out of a plugin.
#[derive(Debug, Clone)]
pub struct ExecutorFactory {
    plugin: Plugin,
    descriptor: Described<ExecutorDescriptor>,
}

impl ExecutorFactory {
    /// The factory of the executors that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let descriptor = Described::new(plugin.clone(), PluginKind::Executor)?;
        Ok(Self { plugin, descriptor })
    }

    /// The name the plugin is registered by.
    #[must_use]
    pub fn name(&self) -> &str {
        self.plugin.name()
    }

    /// An executor for one environment.
    pub fn create(&self) -> Result<ExecutorBridge, BridgeError> {
        Ok(self.executor())
    }

    /// Makes an executor from the validated declaration, retaining it while the program restarts.
    #[must_use]
    pub fn executor(&self) -> ExecutorBridge {
        ExecutorBridge {
            plugin: self.plugin.clone(),
            descriptor: self.descriptor.clone(),
            kind: interned(&self.descriptor.get().kind),
        }
    }
}
