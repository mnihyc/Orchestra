//! Independent provider facts, model lists and credential presentation beside the credential's keeper.

pub mod adapter;
pub mod address;
pub mod dispatch;
pub mod endpoints;
pub mod presenter;
pub mod reports;

use orch_provider_contract::{CredentialPresenter, FactsSource, ModelsSource};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A registered provider program may implement any declared subset; undeclared methods answer unsupported.
pub trait Program: FactsSource + ModelsSource + CredentialPresenter {}
impl<T: FactsSource + ModelsSource + CredentialPresenter + ?Sized> Program for T {}

/// Looks up a configured program whose first successful declaration may follow a failed startup.
pub type ProgramLookup = dyn Fn(&str) -> Option<Arc<dyn Program>> + Send + Sync;

/// Programs registered by the machine's owner, shared by both keeping roles and the forwarder.
#[derive(Default)]
pub struct Programs {
    programs: BTreeMap<String, Arc<dyn Program>>,
    lookup: Option<Arc<ProgramLookup>>,
}

impl std::fmt::Debug for Programs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Programs")
            .field("names", &self.programs.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Programs {
    /// Uses the same local supervisor to find configured programs that become available after startup.
    #[must_use]
    pub fn with_lookup(mut self, lookup: Arc<ProgramLookup>) -> Self {
        self.lookup = Some(lookup);
        self
    }

    /// Registers the negotiated bridge once; no manager request can add code to this registry.
    pub fn register<T: Program + 'static>(
        &mut self,
        name: &str,
        program: Arc<T>,
    ) -> Result<(), String> {
        let descriptor = FactsSource::describe(program.as_ref());
        descriptor.check().map_err(|error| error.to_string())?;
        let reserved = |name: &str| {
            name == "none" || orch_provider_contract::choice::builtin::ALL.contains(&name)
        };
        if name.trim().is_empty()
            || name.chars().count() > orch_provider_contract::limits::TEXT_MAX_CHARS
            || name.chars().any(char::is_control)
            || reserved(name)
            || reserved(&descriptor.name)
        {
            return Err(
                "a provider program needs its own nonempty name, different from a built-in source"
                    .to_owned(),
            );
        }
        if self.programs.contains_key(name) {
            return Err("a provider program with that name is already registered".to_owned());
        }
        self.programs.insert(name.to_owned(), program);
        Ok(())
    }

    /// The selected locally registered program; a missing program never falls back to another source.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Program>> {
        self.programs
            .get(name)
            .cloned()
            .or_else(|| self.lookup.as_ref().and_then(|lookup| lookup(name)))
    }
}
