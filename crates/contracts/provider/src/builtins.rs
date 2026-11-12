//! The built-in sources of a key's facts a program carries: each one's name, its declaration, the addresses it is the default for, and how it is made with the keeping machine's transport.
//!
//! The vendors' readers are extensions, and neither the API nor the connector knows one of them by name. The
//! program that composes each registers the same built-ins in a [`BuiltinSources`] table and passes it in: the
//! API names a provider's default source from it, offers the built-ins and checks a choice of one; the
//! connector makes the source it reads with, giving it its own transport ([`VendorClient`]) and clock. Which
//! built-in is the default of a provider follows one rule that both ask: [`crate::choice::default_source`].

use std::sync::Arc;

use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_types::{Clock, Json};
use url::Url;

use crate::source::{FactsSource, SourceDescriptor};

/// The keeping machine's bounded JSON transport, through which a built-in reads its vendor.
#[async_trait]
pub trait VendorClient: Send + Sync {
    /// Gets `path` at the address of `base`, with `key` presented as a bearer token to that address alone.
    /// Nothing for a refusal, a failed exchange, a reply over `reply_bytes` or one that is not JSON.
    async fn get(&self, base: &Url, path: &str, key: &Secret, reply_bytes: usize) -> Option<Json>;

    /// The same bounded, same-origin read with extension-declared authentication/version headers.
    /// A bearer-only transport can still serve ordinary reads; it never substitutes an unsupported auth shape.
    async fn get_headers(
        &self,
        base: &Url,
        path: &str,
        headers: &std::collections::BTreeMap<String, Secret>,
        reply_bytes: usize,
    ) -> Option<Json> {
        if headers.len() != 1 {
            return None;
        }
        let value = headers
            .get("Authorization")
            .or_else(|| headers.get("authorization"))?;
        let key = Secret::new(value.expose().strip_prefix("Bearer ")?.to_owned());
        self.get(base, path, &key, reply_bytes).await
    }
}

/// Makes a built-in source with the keeping machine's transport and the clock its reads are stamped with.
pub type MakeSource = fn(Arc<dyn VendorClient>, Arc<dyn Clock>) -> Arc<dyn FactsSource>;

/// A built-in source of a key's facts, as a program registers it.
#[derive(Clone, Copy)]
pub struct BuiltinSource {
    /// Its unique registered name.
    pub name: &'static str,
    /// The hosts it is the default source for, as the page that adds a provider names them.
    pub hosts: &'static [&'static str],
    /// What it declares, offered before any credential is read.
    pub descriptor: fn() -> SourceDescriptor,
    /// Whether an endpoint at this address is one it reads: then it is the default source of a key there.
    pub matches: fn(&Url) -> bool,
    /// The source itself.
    pub make: MakeSource,
}

impl std::fmt::Debug for BuiltinSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltinSource")
            .field("name", &self.name)
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

/// Why a table of built-in sources cannot be had.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuiltinFault {
    /// An empty/reserved name cannot identify a registered extension source.
    #[error("{0:?} is not an available source name")]
    Unknown(String),
    /// Two entries have the name.
    #[error("the built-in source {0:?} is registered twice")]
    Twice(String),
    /// An installed provider extension supplied an invalid or conflicting capability declaration.
    #[error("invalid capability declaration for {0:?}")]
    Capabilities(String),
}

/// The built-in sources of a key's facts a program registered, in the order a default is looked for.
#[derive(Debug, Clone)]
pub struct BuiltinSources {
    entries: Vec<BuiltinSource>,
    capabilities: crate::capabilities::Registry,
}

impl Default for BuiltinSources {
    fn default() -> Self {
        let mut capabilities = crate::capabilities::Registry::default();
        if let Ok(common) = crate::capabilities::catalogue() {
            // The shipped bundle is source-validated; a malformed bundle claims no support.
            let _ = capabilities.register("common", common);
        }
        Self {
            entries: Vec::new(),
            capabilities,
        }
    }
}

impl BuiltinSources {
    /// The table of registered entries, in their order, with unique nonempty names.
    pub fn new(entries: impl IntoIterator<Item = BuiltinSource>) -> Result<Self, BuiltinFault> {
        let mut table = Self::default();
        for entry in entries {
            if entry.name.trim().is_empty()
                || entry.name.trim() != entry.name
                || entry.name.chars().count() > crate::limits::TEXT_MAX_CHARS
                || entry.name.chars().any(char::is_control)
                || matches!(
                    entry.name,
                    crate::choice::builtin::ADAPTER | crate::choice::builtin::ENDPOINTS | "none"
                )
            {
                return Err(BuiltinFault::Unknown(entry.name.to_owned()));
            }
            if table.get(entry.name).is_some() {
                return Err(BuiltinFault::Twice(entry.name.to_owned()));
            }
            if let Some(raw) = (entry.descriptor)()
                .extensions
                .get("x_capability_declarations")
            {
                let declaration = serde_json::from_value(raw.clone())
                    .map_err(|_| BuiltinFault::Capabilities(entry.name.into()))?;
                table
                    .capabilities
                    .register(entry.name, declaration)
                    .map_err(|_| BuiltinFault::Capabilities(entry.name.into()))?;
            }
            table.entries.push(entry);
        }
        Ok(table)
    }

    /// Register a capability-only provider extension; offering no facts needs no artificial FactsSource.
    pub fn register_capabilities(
        &mut self,
        name: &str,
        declarations: crate::capabilities::Declarations,
    ) -> Result<(), crate::capabilities::RegistryError> {
        self.capabilities.register(name, declarations)
    }

    /// Removing a capability-only extension recomputes the same snapshot without changing existing providers.
    pub fn remove_capabilities(&mut self, name: &str) -> bool {
        self.capabilities.remove(name)
    }

    /// The same versioned declaration registry used for endpoint discovery, native overrides and service offerings.
    #[must_use]
    pub fn capabilities(&self) -> &crate::capabilities::Registry {
        &self.capabilities
    }

    /// Actual endpoint/model projection; no vendor is selected in shared code.
    #[must_use]
    pub fn endpoint_capabilities(
        &self,
        protocol: &str,
        base: &Url,
        model: &str,
    ) -> crate::capabilities::EndpointProjection {
        crate::capabilities::project_endpoint(
            self.capabilities.declarations(),
            protocol,
            base,
            model,
        )
    }

    /// The built-in named `name`, where it is registered.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&BuiltinSource> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// Every built-in, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, BuiltinSource> {
        self.entries.iter()
    }

    /// The first built-in, in order, that reads an endpoint at one of `endpoints`.
    #[must_use]
    pub fn reading(&self, endpoints: &[Url]) -> Option<&BuiltinSource> {
        self.entries
            .iter()
            .find(|entry| endpoints.iter().any(|base| (entry.matches)(base)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::choice::builtin;
    use crate::source::{FactsReport, SourceAsk, SourceError, SourceErrorCode};

    /// A source that never tells anything: what a table's entries are made into in a test.
    struct Silent;

    #[async_trait]
    impl FactsSource for Silent {
        fn describe(&self) -> SourceDescriptor {
            SourceDescriptor::new("silent", "Silent", "Tells nothing.")
        }

        async fn read_facts(&self, _ask: SourceAsk) -> Result<FactsReport, SourceError> {
            Err(SourceError::new(
                SourceErrorCode::NotTold,
                "nothing is told",
            ))
        }
    }

    fn silent(_client: Arc<dyn VendorClient>, _clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
        Arc::new(Silent)
    }

    fn described() -> SourceDescriptor {
        SourceDescriptor::new("stand-in", "Stand-in", "Reads a stand-in vendor.")
    }

    fn at_openrouter(base: &Url) -> bool {
        base.host_str() == Some("openrouter.ai")
    }

    fn at_deepseek(base: &Url) -> bool {
        base.host_str() == Some("api.deepseek.com")
    }

    /// A built-in that stands in for the vendor of `name`, reading the addresses at its host.
    pub(crate) fn stand_in(name: &'static str, hosts: &'static [&'static str]) -> BuiltinSource {
        let matches: fn(&Url) -> bool = if name == "openrouter" {
            at_openrouter
        } else {
            at_deepseek
        };
        BuiltinSource {
            name,
            hosts,
            descriptor: described,
            matches,
            make: silent,
        }
    }

    /// The table a program registers, with stand-ins for the two vendors.
    pub(crate) fn table() -> BuiltinSources {
        BuiltinSources::new([
            stand_in("openrouter", &["openrouter.ai"]),
            stand_in("deepseek", &["api.deepseek.com"]),
        ])
        .unwrap()
    }

    #[test]
    fn a_table_holds_each_registered_source_once_without_a_closed_vendor_list() {
        let table = table();
        assert_eq!(
            table.iter().map(|entry| entry.name).collect::<Vec<_>>(),
            ["openrouter", "deepseek"]
        );
        assert_eq!(
            table.get("deepseek").map(|entry| entry.hosts),
            Some(&["api.deepseek.com"][..])
        );
        assert!(
            table.get(builtin::ADAPTER).is_none(),
            "the harness's source is no vendor's"
        );
        assert_eq!(
            BuiltinSources::new([stand_in("openrouter", &[]), stand_in("openrouter", &[])])
                .unwrap_err(),
            BuiltinFault::Twice("openrouter".to_owned())
        );
        let mut acme = stand_in("openrouter", &[]);
        acme.name = "acme";
        assert!(BuiltinSources::new([acme]).unwrap().get("acme").is_some());
        acme.name = "";
        assert_eq!(
            BuiltinSources::new([acme]).unwrap_err(),
            BuiltinFault::Unknown(String::new())
        );
        assert_eq!(BuiltinSources::default().iter().count(), 0);
    }

    #[test]
    fn extensions_register_new_names_without_changing_the_core_and_names_remain_unique() {
        let table = table();
        let mut added = stand_in("openrouter", &[]);
        added.name = "acme";
        let extended = BuiltinSources::new(table.iter().copied().chain([added])).unwrap();
        assert_eq!(extended.get("acme").unwrap().name, "acme");
        assert_eq!(
            extended.get("openrouter").unwrap().hosts,
            table.get("openrouter").unwrap().hosts
        );
        for name in ["", "none", "adapter", "endpoints", "bad\nname", " padded "] {
            added.name = name;
            assert!(matches!(
                BuiltinSources::new([added]),
                Err(BuiltinFault::Unknown(_))
            ));
        }
    }

    #[test]
    fn the_first_built_in_that_reads_one_of_the_endpoints_is_found_and_none_for_another_vendor() {
        let table = table();
        let at = |text: &str| Url::parse(text).unwrap();
        assert_eq!(
            table
                .reading(&[
                    at("https://api.openai.com/v1"),
                    at("https://api.deepseek.com/anthropic")
                ])
                .map(|entry| entry.name),
            Some("deepseek")
        );
        assert_eq!(
            table
                .reading(&[
                    at("https://api.deepseek.com"),
                    at("https://openrouter.ai/api/v1")
                ])
                .map(|entry| entry.name),
            Some("openrouter"),
            "the table's order decides, not the endpoints'"
        );
        assert!(table.reading(&[at("https://api.anthropic.com")]).is_none());
        assert!(table.reading(&[]).is_none());
    }

    #[test]
    fn a_registered_built_in_is_made_with_the_keeping_machines_transport() {
        struct Nowhere;
        #[async_trait]
        impl VendorClient for Nowhere {
            async fn get(
                &self,
                _base: &Url,
                _path: &str,
                _key: &Secret,
                _reply_bytes: usize,
            ) -> Option<Json> {
                None
            }
        }
        let entry = table().get("openrouter").copied().unwrap();
        let clock: Arc<dyn Clock> = Arc::new(orch_types::SystemClock);
        let source = (entry.make)(Arc::new(Nowhere), clock);
        assert_eq!(source.describe().name, "silent");
        assert_eq!((entry.descriptor)().name, "stand-in");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod capability_extensions {
    use super::*;
    use serde_json::json;
    #[test]
    fn a_provider_without_facts_registers_endpoint_capabilities_without_changing_an_existing_endpoint()
     {
        let mut sources = BuiltinSources::default();
        let existing = Url::parse("https://existing.example.invalid/v1").unwrap();
        let original = sources.endpoint_capabilities("fixture-wire", &existing, "opaque-model");
        let declaration=serde_json::from_value(json!({"endpoint_rules":[{"when":{"required":["host","model"],"properties":{"host":{"const":"added.example.invalid"},"model":{"const":"lookup-model"}}},"native_tools":{"web_search":{"request":{"tool":{"type":"fixture-lookup"}}}},"offerings":{"lookup":{"enabled":true,"kind":"service"}}}]})).unwrap();
        sources.register_capabilities("added", declaration).unwrap();
        assert_eq!(
            sources.iter().count(),
            0,
            "capability-only providers have no invented facts rows"
        );
        assert_eq!(
            sources.endpoint_capabilities("fixture-wire", &existing, "opaque-model"),
            original
        );
        let added = sources.endpoint_capabilities(
            "fixture-wire",
            &Url::parse("https://added.example.invalid").unwrap(),
            "lookup-model",
        );
        assert!(added.native_tools["web_search"].is_some());
        assert!(
            !added.offerings["lookup"].enabled,
            "discovery never enables a paid service"
        );
        assert!(sources.remove_capabilities("added"));
        assert_eq!(
            sources.endpoint_capabilities("fixture-wire", &existing, "opaque-model"),
            original
        );
    }
}
