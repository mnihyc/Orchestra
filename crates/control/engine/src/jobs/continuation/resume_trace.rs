//! Test-only producer-call witness: duplicate same-ID calls cannot hide behind
//! the command supplier's successful deduplication or one durable journal row.

use super::Outgoing;
use orch_link_contract::commands::CommandKind;
use orch_types::Id;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, PoisonError},
};

static RESUMES: OnceLock<Mutex<HashMap<Id, Vec<Outgoing>>>> = OnceLock::new();

pub(super) struct Trace {
    operation: Id,
}

impl Trace {
    pub(super) fn new(operation: Id) -> Self {
        assert!(
            RESUMES
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(operation, Vec::new())
                .is_none(),
            "the owned resume witness is already installed"
        );
        Self { operation }
    }
    pub(super) fn commands(&self) -> Vec<Outgoing> {
        RESUMES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.operation)
            .expect("the owned resume witness remains installed")
            .clone()
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        RESUMES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.operation);
    }
}

pub(super) fn note(operation: Id, command: &Outgoing) {
    if command.kind == CommandKind::SessionContinuationResume
        && let Some(commands) = RESUMES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&operation)
    {
        commands.push(command.clone());
    }
}
