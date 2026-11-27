//! Persist a primary choice before its journaled answer permits reconnecting.

use crate::{enrollment::DataDir, policy::PolicyStore};
use orch_config::loader::Validate;
use orch_link_contract::{
    commands::{CommandError, ErrorCode},
    gateways::{GatewayChoice, GatewaySelection},
};
use orch_types::Id;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub struct Changed {
    pub choice: GatewayChoice,
    pub changed: bool,
}

struct Pending {
    id: Id,
    choice: GatewayChoice,
}

pub struct Requests {
    directory: PathBuf,
    policy: Arc<PolicyStore>,
    reconnect: Arc<dyn Fn(GatewayChoice) + Send + Sync>,
    pending: Mutex<Option<Pending>>,
}

fn failed(error: impl std::fmt::Display) -> CommandError {
    tracing::warn!(%error, "the primary gateway choice could not be saved");
    CommandError::new(
        ErrorCode::Internal,
        "The machine could not save its gateway choice.",
    )
}

impl Requests {
    pub fn new(
        directory: PathBuf,
        policy: Arc<PolicyStore>,
        reconnect: Arc<dyn Fn(GatewayChoice) + Send + Sync>,
    ) -> Self {
        Self {
            directory,
            policy,
            reconnect,
            pending: Mutex::new(None),
        }
    }

    pub fn request(&self, id: Id, choice: &GatewayChoice) -> Result<Changed, CommandError> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.is_some() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "Wait for the previous gateway change to finish.",
            ));
        }
        let changed = self.save(choice)?;
        *pending = Some(Pending {
            id,
            choice: choice.clone(),
        });
        Ok(changed)
    }

    /// The local owner can supersede a system request without waiting for its connection.
    pub fn request_local(&self, choice: &GatewayChoice) -> Result<Changed, CommandError> {
        let changed = self.save(choice)?;
        (self.reconnect)(choice.clone());
        Ok(changed)
    }

    fn save(&self, choice: &GatewayChoice) -> Result<Changed, CommandError> {
        let data = DataDir::open(&self.directory).map_err(failed)?;
        let mut config = data.config(|_| None).map_err(failed)?;
        let current = if self.policy.is_managed() {
            config.connector.gateway.clone()
        } else {
            self.policy.config().connector.gateway
        };
        if choice == &current {
            return Ok(Changed {
                choice: choice.clone(),
                changed: false,
            });
        }
        config.connector.gateway = choice.clone();
        config.validate().map_err(|_| {
            CommandError::new(
                ErrorCode::Unsupported,
                "Choose Automatic or one advertised gateway.",
            )
        })?;
        if choice.selection == GatewaySelection::Selected
            && !data
                .record()
                .map_err(failed)?
                .gateways
                .iter()
                .any(|gateway| Some(gateway.id.as_str()) == choice.gateway_id.as_deref())
        {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "The selected gateway is no longer advertised.",
            ));
        }
        if self.policy.is_managed() {
            data.update_config(|config| config.connector.gateway = choice.clone())
                .map_err(failed)?;
        }
        self.policy.change_gateway(choice).map_err(failed)?;
        Ok(Changed {
            choice: choice.clone(),
            changed: true,
        })
    }

    pub fn answered(&self, id: Id) {
        let accepted = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.as_ref().is_none_or(|pending| pending.id != id) {
                return;
            }
            pending.take()
        };
        let Some(accepted) = accepted else {
            return;
        };
        // A local owner's later choice must not be replaced by this older completion.
        let current = DataDir::inspect(&self.directory).and_then(|data| data.config(|_| None));
        if current.is_ok_and(|config| config.connector.gateway == accepted.choice) {
            (self.reconnect)(accepted.choice);
        }
    }
}
