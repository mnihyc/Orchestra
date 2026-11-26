//! Authored defaults for a thread whose provider, model or both may come from a pool.

use orch_provider_contract::pools::{Leaf, Target};
use orch_types::Json;
use serde::{Deserialize, Serialize};

/// A model name on the selected account, or an independently selected model target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelDefault {
    Name(OfferingName),
    Target(Target),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OfferingName {
    Offering { name: String },
}

impl ModelDefault {
    /// The known offering, before making any choice from a pool.
    pub fn offering(&self) -> Option<&str> {
        match self {
            Self::Name(OfferingName::Offering { name }) => Some(name),
            Self::Target(Target::Single {
                resource: Leaf::Model { backend },
            }) => backend
                .offering
                .as_ref()
                .map(|value| value.offering.as_str()),
            _ => None,
        }
    }
}

/// Read a setting projection without claiming a representative of a pool.
pub fn offering(value: &Json) -> Option<&str> {
    match value.get("type")?.as_str()? {
        "offering" => value.get("name")?.as_str(),
        "single" => value
            .pointer("/resource/backend/offering/offering")?
            .as_str(),
        _ => None,
    }
}

/// A concrete model selected on the thread's bound provider.
pub fn named(name: &str) -> Json {
    serde_json::json!({"type":"offering","name":name})
}
