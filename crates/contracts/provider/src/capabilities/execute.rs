//! Open execution adapters retain their existing credential, first-effect and ledger producers.
use super::*;
use crate::{SourceError, SourceErrorCode};
use async_trait::async_trait;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Actual ordinary/tool source identities, never supplied as capability arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallSource {
    /// Existing logical call id, stable on replay.
    pub request_id: Id,
    /// Actual source thread.
    pub thread_id: Option<Id>,
    /// Actual source turn.
    pub turn_id: Option<Id>,
    /// Actual hierarchy place for non-thread calls.
    pub node_id: Option<Id>,
    /// Existing frozen responsibility; never an account session or permission proof.
    pub for_user_id: Option<Id>,
}

/// Resolved intent is not admission. Execution still rechecks actual source authority and limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCall {
    /// Actual source identities.
    pub source: CallSource,
    /// Capability id.
    pub capability: String,
    /// Exact selected route.
    pub route: ResolvedRoute,
    /// Existing common item family expected from this capability.
    pub item: String,
    /// Declared ledger unit.
    pub ledger_unit: String,
    /// Input, without credential material.
    pub input: Json,
}

/// Existing canonical presentation; the ordinary item/blob writer owns persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityItem {
    /// Declared common item family.
    pub family: String,
    /// Actual native operation id; content/timing is not a substitute.
    pub native_ref: Option<String>,
    /// Existing item payload, with blobs retained through the ordinary producer.
    pub payload: Json,
}

/// Common results and existing normalized usage facts, never a second ledger writer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityResult {
    /// Actual common items.
    pub items: Vec<CapabilityItem>,
    /// Actual normalized request/timing facts; missing is not a measured zero.
    pub usage: Json,
}

/// Model, API and later program-session implementations register the same execution port.
#[async_trait]
pub trait CapabilityExecutor: Send + Sync {
    /// Only installed code declares the codecs and facilities it really implements.
    fn support(&self) -> Vec<ExecutionSupport> {
        Vec::new()
    }
    /// Use the existing current-authority, credential-custody, IO and usage paths.
    async fn execute(&self, call: CapabilityCall) -> Result<CapabilityResult, SourceError>;
}

/// No closed backend enum or vendor dispatch in the core.
#[derive(Default, Clone)]
pub struct Executors {
    entries: BTreeMap<String, Arc<dyn CapabilityExecutor>>,
}
impl std::fmt::Debug for Executors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executors")
            .field("names", &self.names())
            .finish()
    }
}
impl Executors {
    /// Compose trusted code, not executable declarations. Names remain unique.
    pub fn register(
        &mut self,
        name: &str,
        executor: Arc<dyn CapabilityExecutor>,
    ) -> Result<(), RegistryError> {
        if name.trim().is_empty() {
            return Err(RegistryError::MissingName);
        }
        if self.entries.contains_key(name) {
            return Err(RegistryError::Conflict {
                noun: "executor",
                id: name.into(),
            });
        }
        self.entries.insert(name.into(), executor);
        Ok(())
    }
    /// Installed names are facts for the same route resolver.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
    /// Supply these facts to the same resolver that previews a capability's actual route.
    #[must_use]
    pub fn support(&self) -> BTreeMap<String, Vec<ExecutionSupport>> {
        self.entries
            .iter()
            .map(|(name, executor)| (name.clone(), executor.support()))
            .collect()
    }
    /// An unavailable adapter never falls back to another provider or kind.
    pub async fn execute(&self, call: CapabilityCall) -> Result<CapabilityResult, SourceError> {
        let executor = call
            .route
            .executor
            .as_ref()
            .and_then(|name| self.entries.get(name))
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::Unsupported,
                    "the selected capability executor is unavailable",
                )
            })?;
        if call.route.execution.as_ref().is_some_and(|required| {
            !executor
                .support()
                .iter()
                .any(|support| support.satisfies(required))
        }) {
            return Err(SourceError::new(
                SourceErrorCode::Unsupported,
                "the selected executor does not implement this consumer's codecs or execution constraints",
            ));
        }
        let expected = call.item.clone();
        let result = executor.execute(call).await?;
        if result.items.iter().any(|item| item.family != expected) {
            return Err(SourceError::new(
                SourceErrorCode::Malformed,
                "the capability returned another common item family",
            ));
        }
        Ok(result)
    }
}
