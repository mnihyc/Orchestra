//! Sealed environment attachment and tool intent retained before effects and rebuilt under current local policy.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use orch_link_contract::commands::{Command, CommandError, ErrorCode, McpStart};
use orch_types::Id;
use serde::{Deserialize, Serialize};

use crate::credentials::SecretStore;
use crate::roles::Services;

const PREFIX: &str = "environment-";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Kept {
    pub(crate) id: Id,
    pub(crate) command: Command,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Saved {
    version: u32,
    pub(crate) environment: Id,
    pub(crate) generation: i64,
    pub(crate) attach: Kept,
    pub(crate) executors: BTreeMap<String, Kept>,
    pub(crate) servers: BTreeMap<String, Kept>,
    #[serde(default)]
    pub(crate) displays: BTreeMap<Id, Kept>,
}

/// One writer orders startup intent against detach and stop tombstones.
pub(crate) struct Desired {
    store: Option<Arc<SecretStore>>,
    lock: Mutex<()>,
}

fn unavailable() -> CommandError {
    CommandError::new(
        ErrorCode::Internal,
        "the environment recovery journal is unavailable",
    )
}

fn target(command: &Command) -> Option<Id> {
    match command {
        Command::EnvAttach(body) => Some(body.environment_id),
        Command::EnvDetach(body) => Some(body.environment_id),
        Command::ExecStart(body) => Some(body.environment_id),
        Command::McpStart(McpStart::Given(body)) => Some(body.environment_id),
        Command::McpStart(McpStart::Local(body)) => Some(body.environment_id),
        Command::McpStop(body) => Some(body.environment_id),
        Command::DisplayAdopt(body) => Some(body.environment_id),
        _ => None,
    }
}

impl Desired {
    pub(crate) fn new(services: &Services) -> Self {
        let store = SecretStore::open(&services.data_dir, Arc::clone(&services.keys));
        if let Err(error) = &store {
            tracing::warn!(%error, "environment recovery could not open its sealed journal");
        }
        Self {
            store: store.ok().map(Arc::new),
            lock: Mutex::new(()),
        }
    }

    fn store(&self) -> Result<&SecretStore, CommandError> {
        self.store.as_deref().ok_or_else(unavailable)
    }

    fn load(&self, environment: Id) -> Result<Option<Saved>, CommandError> {
        let name = format!("{PREFIX}{environment}");
        let Some(value) = self.store()?.get(&name).map_err(|_| unavailable())? else {
            return Ok(None);
        };
        let saved: Saved = serde_json::from_str(&value).map_err(|_| unavailable())?;
        if saved.version != 1
            || saved.environment != environment
            || !matches!(&saved.attach.command, Command::EnvAttach(body) if body.environment_id == environment)
            || saved
                .executors
                .values()
                .chain(saved.servers.values())
                .chain(saved.displays.values())
                .any(|kept| target(&kept.command) != Some(environment))
        {
            return Err(unavailable());
        }
        Ok(Some(saved))
    }

    fn save(&self, saved: &Saved) -> Result<(), CommandError> {
        let value = serde_json::to_string(saved).map_err(|_| unavailable())?;
        self.store()?
            .put(&format!("{PREFIX}{}", saved.environment), &value)
            .map_err(|_| unavailable())
    }

    /// Records desired starts before their effects, and forgets desired work before stopping it.
    pub(crate) fn remember(
        &self,
        id: Id,
        command: &Command,
        generation: i64,
    ) -> Result<(), CommandError> {
        let _write = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        if let Command::DisplayClose(body) = command {
            for mut saved in self.records_unlocked()? {
                saved.displays.remove(&body.display_id);
                self.save(&saved)?;
            }
            return Ok(());
        }
        if let Command::McpForget(body) = command {
            for mut saved in self.records_unlocked()? {
                saved.servers.retain(|_, kept| {
                    !matches!(&kept.command,
                    Command::McpStart(McpStart::Local(local)) if local.server_id == body.server_id)
                });
                self.save(&saved)?;
            }
            return Ok(());
        }
        let Some(environment) = target(command) else {
            return Ok(());
        };
        if matches!(command, Command::EnvDetach(_)) {
            return self
                .store()?
                .remove(&format!("{PREFIX}{environment}"))
                .map_err(|_| unavailable());
        }
        let kept = Kept {
            id,
            command: command.clone(),
        };
        let mut saved = match command {
            Command::EnvAttach(_) => self
                .load(environment)?
                .filter(|saved| saved.generation == generation)
                .unwrap_or_else(|| Saved {
                    version: 1,
                    environment,
                    generation,
                    attach: kept.clone(),
                    executors: BTreeMap::new(),
                    servers: BTreeMap::new(),
                    displays: BTreeMap::new(),
                }),
            _ => match self.load(environment)? {
                Some(saved) => saved,
                None if matches!(command, Command::McpStop(_)) => return Ok(()),
                None => {
                    return Err(CommandError::new(
                        ErrorCode::NotFound,
                        "the environment is not attached for recovery",
                    ));
                }
            },
        };
        match command {
            Command::EnvAttach(_) => saved.attach = kept,
            Command::ExecStart(body) => {
                saved.executors.insert(body.executor.clone(), kept);
            }
            Command::McpStart(McpStart::Given(body)) => {
                saved.servers.insert(body.name.clone(), kept);
            }
            Command::McpStart(McpStart::Local(body)) => {
                saved.servers.insert(body.name.clone(), kept);
            }
            Command::McpStop(body) => {
                saved.servers.remove(&body.name);
            }
            Command::DisplayAdopt(body) => {
                saved.displays.insert(body.display_id, kept);
            }
            _ => return Ok(()),
        }
        self.save(&saved)
    }

    fn records_unlocked(&self) -> Result<Vec<Saved>, CommandError> {
        let mut saved = Vec::new();
        for name in self.store()?.names().map_err(|_| unavailable())? {
            let Some(id) = name.strip_prefix(PREFIX) else {
                continue;
            };
            let id = id.parse().map_err(|_| unavailable())?;
            if let Some(row) = self.load(id)? {
                saved.push(row);
            }
        }
        Ok(saved)
    }

    pub(crate) fn records(&self) -> Result<Vec<Saved>, CommandError> {
        let _read = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.records_unlocked()
    }

    pub(crate) fn current(&self, environment: Id) -> Result<Option<Saved>, CommandError> {
        let _read = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.load(environment)
    }

    pub(crate) fn retains(command: &Command) -> bool {
        target(command).is_some()
            || matches!(command, Command::McpForget(_) | Command::DisplayClose(_))
    }
}
