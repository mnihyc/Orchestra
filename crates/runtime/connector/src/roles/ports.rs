//! Physical public listeners, served identically by an environment connector and a node.

use std::sync::Arc;

use async_trait::async_trait;
use orch_link_contract::commands::{Command, CommandError, CommandKind, CommandResult, ErrorCode};
use orch_link_contract::events::Event;
use orch_link_contract::ports::PortState;
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use tokio_util::sync::CancellationToken;

use super::{Call, RoleHandler, Services};

/// The handler belongs to a role actually offered by this machine; it invents no node role.
pub struct PortsRole {
    services: Arc<Services>,
    role: Role,
}

impl PortsRole {
    /// One handler per physical machine, whichever of its environment or node roles hosts it.
    #[must_use]
    pub fn new(services: Arc<Services>, role: Role) -> Self {
        Self { services, role }
    }
}

#[async_trait]
impl RoleHandler for PortsRole {
    fn role(&self) -> Role {
        self.role
    }
    fn takes(&self) -> &'static [CommandKind] {
        &[
            CommandKind::HostPorts,
            CommandKind::PortPublish,
            CommandKind::PortClose,
        ]
    }
    async fn handle(&self, _call: Call, command: Command) -> Result<CommandResult, CommandError> {
        let command = self.services.policy.gate(command)?.command;
        match command {
            Command::HostPorts(body) => {
                let policy = self.services.policy.policy().ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::Unsupported,
                        "ports (environment role): this runtime has no physical-machine policy",
                    )
                })?;
                let mut taken = crate::host_ports::taken_now();
                taken.extend(self.services.port_publisher.held_ports());
                Ok(CommandResult::HostPorts(crate::host_ports::answer(
                    &crate::host_ports::policy_limits(&policy.ports),
                    &body,
                    &taken,
                    self.services.clock.time(),
                )))
            }
            Command::PortPublish(body) => Ok(CommandResult::PortPublish(
                self.services.port_publisher.publish(body).await,
            )),
            Command::PortClose(body) => Ok(CommandResult::PortClose(
                self.services.port_publisher.close(&body).await,
            )),
            _ => Err(CommandError::new(
                ErrorCode::Unsupported,
                "this is not a public port command",
            )),
        }
    }
}

/// Narrowing the one live policy also withdraws existing sockets and all of their connections.
pub async fn watch(services: Arc<Services>, stop: CancellationToken) {
    let mut changes = services.policy.changes();
    let mut ticker = tokio::time::interval(crate::host_publish::CEILING_LOOK_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ()=stop.cancelled()=>return,
            _=ticker.tick()=>{},
            result=changes.changed()=>{if result.is_err(){return;}}
        }
        services.port_publisher.prune_records();
        if services.port_publisher.held_ports().is_empty() {
            continue;
        }
        let ports = services.policy.policy().map_or_else(
            || orch_config::server::Ports {
                ranges: Some(Vec::new()),
                ..Default::default()
            },
            |policy| crate::host_ports::policy_listeners(&policy.ports),
        );
        for (opening_id, revision, reason) in services.port_publisher.close_forbidden(&ports).await
        {
            let event = Event::PortState(PortState {
                opening_id,
                revision,
                state: orch_link_contract::ports::PortPublishState::Failed,
                reason: Some(reason),
            });
            if let Err(error) = services.publisher.publish(Stream::Machine, event).await {
                tracing::warn!(%error,"a public port withdrawal could not be reported");
            }
        }
    }
}
