//! Extension identity and preserved x_ fields.
//!
//! Every implementation of an extension point says what it is in a
//! descriptor, and the rest of the system acts on what that declares. Two
//! things about a descriptor are the same for every kind and are here: who
//! the implementation is ([`PluginIdentity`]), and the members that are its
//! own. A member whose name starts with `x_` belongs to the implementation:
//! it is stored and shown, and never interpreted. Every other member that a
//! reader does not know is ignored, so that a descriptor written for a later
//! version can still be read. [`Extensions`] holds the first and drops the
//! second; a descriptor carries one, flattened beside its own members.

use std::collections::BTreeMap;

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::registration::PluginKind;

/// What the name of a member that belongs to its implementation starts with.
pub const EXTENSION_PREFIX: &str = "x_";

/// Who an implementation is: the extension point it implements, the name it
/// is registered and selected by, and its own version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PluginIdentity {
    /// The extension point it implements.
    pub kind: PluginKind,
    /// Its name, unique among the implementations of that kind on one host.
    pub name: String,
    /// Its own version, as it writes it.
    pub version: String,
}

/// How an external implementation is doing, as its host sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Started or attached, and answering.
    Running,
    /// Down, and being started or attached to again.
    Restarting,
    /// Stopped for good: it could not be started, or its handshake failed.
    Failed,
}

/// One external implementation as the component that hosts it reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PluginStatus {
    /// The extension point it implements.
    pub kind: PluginKind,
    /// Its name.
    pub name: String,
    /// Its own version.
    pub version: String,
    /// How it is doing.
    pub state: PluginState,
}

/// A name that cannot be a member of an implementation's own.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a member of an implementation's own is named x_…, and {0:?} is not")]
pub struct NotAnExtension(pub String);

/// The members of a descriptor that belong to its implementation: those
/// whose names start with [`EXTENSION_PREFIX`]. Read from a descriptor it
/// keeps exactly those and drops every other member it is handed, which are
/// the ones the reader does not know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extensions(BTreeMap<String, Json>);

impl Extensions {
    /// No members.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `name` is the name of a member of an implementation's own.
    #[must_use]
    pub fn owns(name: &str) -> bool {
        name.starts_with(EXTENSION_PREFIX)
    }

    /// Sets the member `name`, which has to start with [`EXTENSION_PREFIX`].
    pub fn insert(&mut self, name: &str, value: Json) -> Result<(), NotAnExtension> {
        if Self::owns(name) {
            self.0.insert(name.to_owned(), value);
            Ok(())
        } else {
            Err(NotAnExtension(name.to_owned()))
        }
    }

    /// The member `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Json> {
        self.0.get(name)
    }

    /// Every member, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Json)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Serialize for Extensions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter())
    }
}

impl<'de> Deserialize<'de> for Extensions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut members = BTreeMap::<String, Json>::deserialize(deserializer)?;
        members.retain(|name, _| Self::owns(name));
        Ok(Self(members))
    }
}

impl JsonSchema for Extensions {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Extensions".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "object", "patternProperties": { "^x_": true } })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
    struct Descriptor {
        name: String,
        #[serde(flatten)]
        extensions: Extensions,
    }

    #[test]
    fn members_of_its_own_are_kept_and_unknown_ones_are_ignored() {
        let read: Descriptor = serde_json::from_value(json!({
            "name": "firecracker",
            "x_kernel": "/var/lib/fc/vmlinux",
            "x_limits": { "vcpus": 8 },
            "added_in_a_later_version": true
        }))
        .unwrap();
        assert_eq!(
            read.extensions.get("x_kernel"),
            Some(&json!("/var/lib/fc/vmlinux"))
        );
        assert_eq!(read.extensions.len(), 2);
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            json!({ "name": "firecracker", "x_kernel": "/var/lib/fc/vmlinux", "x_limits": { "vcpus": 8 } })
        );
    }

    #[test]
    fn only_a_name_with_the_prefix_can_be_set() {
        let mut extensions = Extensions::new();
        assert_eq!(extensions.insert("x_note", json!("kept")), Ok(()));
        assert_eq!(
            extensions.insert("title", json!("Another title")),
            Err(NotAnExtension("title".into()))
        );
        assert_eq!(extensions.iter().count(), 1);
    }

    #[test]
    fn a_descriptor_says_in_its_schema_that_it_may_carry_them() {
        let schema = serde_json::to_value(schemars::schema_for!(Descriptor)).unwrap();
        assert!(
            schema["patternProperties"]["^x_"].is_boolean()
                || schema["patternProperties"]["^x_"].is_object()
        );
        assert_eq!(schema["required"], json!(["name"]));
    }
}
