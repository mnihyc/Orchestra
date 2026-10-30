//! One runtime registration interface and policy gate for machine-local agent and provider programs.

use crate::{policy::PolicyStore, roles::Services};
use async_trait::async_trait;
use orch_api_contract::machine_programs::{
    MachineProgramCapability, MachineProgramKind, MachinePrograms, MachineProgramsCommand,
};
use orch_link_contract::{
    commands::{CommandError, ErrorCode},
    policy::PolicyKey,
    session::Role,
};

/// The composition owns its existing plugin supervisor; the connector routes commands to it.
#[async_trait]
pub trait Handler: Send + Sync {
    /// Meaningful supervisor state changes; the connector reports them over its existing live stream.
    fn changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
    }
    /// The last observed registry, without spawning or polling a program.
    fn snapshot(&self, policy: &PolicyStore) -> MachinePrograms;
    /// Register, test, remove or reload through that same supervisor.
    async fn execute(
        &self,
        request: MachineProgramsCommand,
        services: &Services,
    ) -> Result<MachinePrograms, CommandError>;
}

/// A registration switch belongs to the contract's role section.
#[must_use]
pub const fn policy_key(kind: MachineProgramKind) -> PolicyKey {
    match kind {
        MachineProgramKind::Agent => PolicyKey::AgentPrograms,
        MachineProgramKind::ProviderSource => PolicyKey::ProviderPrograms,
    }
}

/// Facts and local handover jointly decide what the manager may ask this connector to register.
#[must_use]
pub fn capability(store: &PolicyStore, kind: MachineProgramKind) -> MachineProgramCapability {
    let key = policy_key(kind);
    let policy = store.policy();
    let roles = policy.as_ref().and_then(|policy| policy.roles.as_ref());
    let supported = roles.is_some_and(|roles| match kind {
        MachineProgramKind::Agent => roles.contains(&Role::Agent),
        MachineProgramKind::ProviderSource => {
            roles.contains(&Role::Agent) || roles.contains(&Role::Provider)
        }
    });
    let delegated = policy.as_ref().is_some_and(|policy| policy.delegates(key));
    let enabled = policy
        .as_ref()
        .is_some_and(|policy| policy.get(key).as_bool() == Some(true));
    let remote = supported && delegated && enabled;
    let reason=(!remote).then(|| if !supported {"This running program does not offer that role; enroll a connector with it first.".to_owned()}
        else if policy.as_ref().is_some_and(|policy|policy.delegates(key)) {format!("{} ({} role) is off; the machine's managers control this setting.",key.as_str(),key.sections()[0].as_str())}
        else {format!("{} ({} role) is kept by the machine's operator; add the program locally and Reload, or hand this setting to Orch.",key.as_str(),key.sections()[0].as_str())});
    MachineProgramCapability {
        kind,
        supported,
        delegated,
        enabled,
        remote,
        policy_key: key.as_str().into(),
        reason,
    }
}

/// Rechecked immediately before persistence/activation as well as on command receipt.
pub fn require_registration(
    store: &PolicyStore,
    kind: MachineProgramKind,
) -> Result<(), CommandError> {
    let rule = capability(store, kind);
    if rule.remote {
        Ok(())
    } else {
        Err(CommandError::new(
            ErrorCode::Unsupported,
            rule.reason
                .unwrap_or_else(|| "Program registration is unavailable here.".into()),
        ))
    }
}
