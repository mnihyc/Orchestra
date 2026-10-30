//! An idle, journal-acknowledged change of a physical connector's role composition.

use orch_link_contract::commands::{
    CommandError, ErrorCode, MachineRolesSet, MachineRolesSetResult,
};
use orch_types::Id;
use std::sync::Mutex;
use tokio::sync::{Notify, OwnedRwLockWriteGuard};

struct Pending {
    id: Id,
    answered: bool,
    admission: OwnedRwLockWriteGuard<()>,
}

/// Shared by the native command handler and its restart supervisor. Keeping the admission guard
/// prevents a new turn entering between saving the roles and rebuilding their handlers.
pub struct Requests {
    supported: bool,
    pending: Mutex<Option<Pending>>,
    changed: Notify,
}

impl Requests {
    /// Only a physical connector run by the ordinary supervisor can replace its role handlers.
    pub fn new(supported: bool) -> Self {
        Self {
            supported,
            pending: Mutex::new(None),
            changed: Notify::new(),
        }
    }

    /// A stored fact lets the page offer only what this running composition actually supports.
    pub fn capability(
        &self,
        policy: &crate::policy::PolicyStore,
    ) -> orch_api_contract::resources::MachineRoleCapability {
        let policy = policy.policy();
        orch_api_contract::resources::MachineRoleCapability {
            supported: self.supported,
            delegated: policy.as_ref().is_some_and(|policy| {
                policy.delegates(orch_link_contract::policy::PolicyKey::MachineRoles)
            }),
            enabled: policy.as_ref().is_some_and(|policy| policy.machine_roles),
        }
    }

    /// The one policy store persists roles without changing the currently active-role facts.
    pub fn request(
        &self,
        id: Id,
        body: &MachineRolesSet,
        policy: &crate::policy::PolicyStore,
        admission: OwnedRwLockWriteGuard<()>,
    ) -> Result<MachineRolesSetResult, CommandError> {
        if !self.supported {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "This program cannot change its roles here. orch-node provides Node; a unit takes its roles from its template. Add a separate orch-connector for Environment, Agent or Provider.",
            ));
        }
        let roles = orch_link_contract::enrollment_choices::connector_roles(&body.roles.join(","))
            .map_err(|detail| CommandError::new(ErrorCode::Unsupported, detail))?;
        if roles.len() != body.roles.len() {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "Choose each role as a separate name.",
            ));
        }
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.is_some() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "The connector is already reconnecting with its saved roles. Wait for it to come online.",
            ));
        }
        let reconnecting = policy.change_roles(&roles)?;
        if reconnecting {
            *pending = Some(Pending {
                id,
                answered: false,
                admission,
            });
        }
        Ok(MachineRolesSetResult {
            roles: roles.iter().map(crate::enrollment::role_name).collect(),
            reconnecting,
        })
    }

    /// Called only after the command's result is durable, so reconnect cannot lose its answer.
    pub fn answered(&self, id: Id) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(change) = pending.as_mut().filter(|change| change.id == id) {
            change.answered = true;
            self.changed.notify_one();
        }
    }

    /// The supervisor keeps the idle fence through stopping and rebuilding the connector.
    pub async fn until_answered(&self) -> OwnedRwLockWriteGuard<()> {
        loop {
            let notified = self.changed.notified();
            {
                let mut pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if pending.as_ref().is_some_and(|change| change.answered)
                    && let Some(change) = pending.take()
                {
                    return change.admission;
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyStore;
    use orch_config::ConnectorConfig;
    use orch_link_contract::{policy::PolicyKey, session::Role};
    use std::sync::Arc;

    #[tokio::test]
    async fn saved_roles_wait_for_the_durable_answer_and_keep_active_facts_until_restart()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.toml");
        let mut config = ConnectorConfig::default();
        config.policy.delegated = vec!["machine.roles".into()];
        let store = PolicyStore::connected(config, Some(path.clone()))?;
        store.offered_roles(&[Role::Environment, Role::Agent]);
        let requests = Requests::new(true);
        let turns = Arc::new(tokio::sync::RwLock::new(()));
        let id = orch_types::new_id();
        let result = requests.request(
            id,
            &MachineRolesSet {
                roles: vec!["agent".into(), "provider".into()],
            },
            &store,
            Arc::clone(&turns).try_write_owned()?,
        )?;
        assert!(result.reconnecting);
        assert_eq!(result.roles, ["agent", "provider"]);
        assert!(
            turns.try_read().is_err(),
            "new starts remain fenced until the new composition exists"
        );
        let saved: ConnectorConfig =
            orch_config::load_text(&std::fs::read_to_string(&path)?, "role-test", |_| None)?;
        assert_eq!(saved.connector.roles, ["agent", "provider"]);
        assert_eq!(
            store.policy().and_then(|policy| policy.roles),
            Some(vec![Role::Environment, Role::Agent])
        );
        assert!(
            !store.reload_from_file()?,
            "a file reload must not turn requested roles into active facts"
        );
        assert_eq!(
            requests
                .pending
                .lock()
                .map_err(|_| "role-change lock poisoned")?
                .as_ref()
                .map(|pending| pending.answered),
            Some(false)
        );
        requests.answered(orch_types::new_id());
        assert_eq!(
            requests
                .pending
                .lock()
                .map_err(|_| "role-change lock poisoned")?
                .as_ref()
                .map(|pending| pending.answered),
            Some(false)
        );
        requests.answered(id);
        let guard =
            tokio::time::timeout(std::time::Duration::from_secs(1), requests.until_answered())
                .await?;
        assert!(turns.try_read().is_err());
        drop(guard);
        assert!(turns.try_read().is_ok());
        let restarted = PolicyStore::connected(saved, Some(path))?;
        restarted.offered_roles(&[Role::Agent, Role::Provider]);
        assert_eq!(
            restarted.policy().and_then(|policy| policy.roles),
            Some(vec![Role::Agent, Role::Provider])
        );
        Ok(())
    }

    #[test]
    fn local_role_policy_and_the_separate_node_program_cannot_be_overridden()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut config = ConnectorConfig::default();
        config.policy.delegated = vec!["*".into()];
        let store = PolicyStore::connected(config, None)?;
        store.set_here(PolicyKey::MachineRoles, &serde_json::json!(false))?;
        let before = store.config().connector.roles;
        let roles = MachineRolesSet {
            roles: vec!["provider".into()],
        };
        let requests = Requests::new(true);
        let turns = Arc::new(tokio::sync::RwLock::new(()));
        let refusal = requests
            .request(
                orch_types::new_id(),
                &roles,
                &store,
                Arc::clone(&turns).try_write_owned()?,
            )
            .err()
            .ok_or("local off must refuse")?;
        assert!(refusal.detail.contains("machine.roles"));
        assert!(refusal.detail.contains("local"));
        assert_eq!(store.config().connector.roles, before);
        assert!(turns.try_read().is_ok());
        let node = Requests::new(false);
        assert!(
            node.request(
                orch_types::new_id(),
                &roles,
                &store,
                Arc::clone(&turns).try_write_owned()?
            )
            .is_err()
        );
        let node = MachineRolesSet {
            roles: vec!["node".into()],
        };
        assert!(
            requests
                .request(
                    orch_types::new_id(),
                    &node,
                    &store,
                    Arc::clone(&turns).try_write_owned()?
                )
                .err()
                .is_some_and(|error| error.detail.contains("orch-node"))
        );
        assert!(!requests.capability(&store).delegated);
        Ok(())
    }
}
