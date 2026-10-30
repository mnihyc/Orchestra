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

use crate::choice::builtin;
use crate::source::{FactsSource, SourceDescriptor};

/// The keeping machine's bounded JSON transport, through which a built-in reads its vendor.
#[async_trait]
pub trait VendorClient: Send + Sync {
    /// Gets `path` at the address of `base`, with `key` presented as a bearer token to that address alone.
    /// Nothing for a refusal, a failed exchange, a reply over `reply_bytes` or one that is not JSON.
    async fn get(&self, base: &Url, path: &str, key: &Secret, reply_bytes: usize) -> Option<Json>;
}

/// Makes a built-in source with the keeping machine's transport and the clock its reads are stamped with.
pub type MakeSource = fn(Arc<dyn VendorClient>, Arc<dyn Clock>) -> Arc<dyn FactsSource>;

/// A built-in source of a key's facts, as a program registers it.
#[derive(Clone, Copy)]
pub struct BuiltinSource {
    /// Its name, one of [`builtin::KEY_FACTS`].
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
    /// The name is none of [`builtin::KEY_FACTS`].
    #[error("{0:?} is not the name of a built-in source of a key's facts")]
    Unknown(String),
    /// Two entries have the name.
    #[error("the built-in source {0:?} is registered twice")]
    Twice(String),
}

/// The built-in sources of a key's facts a program registered, in the order a default is looked for.
#[derive(Debug, Clone, Default)]
pub struct BuiltinSources {
    entries: Vec<BuiltinSource>,
}

impl BuiltinSources {
    /// The table of `entries`, in their order. Each name is one of [`builtin::KEY_FACTS`], once.
    pub fn new(entries: impl IntoIterator<Item = BuiltinSource>) -> Result<Self, BuiltinFault> {
        let mut table = Self::default();
        for entry in entries {
            if !builtin::KEY_FACTS.contains(&entry.name) {
                return Err(BuiltinFault::Unknown(entry.name.to_owned()));
            }
            if table.get(entry.name).is_some() {
                return Err(BuiltinFault::Twice(entry.name.to_owned()));
            }
            table.entries.push(entry);
        }
        Ok(table)
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
        let matches: fn(&Url) -> bool = if name == builtin::OPENROUTER {
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
            stand_in(builtin::OPENROUTER, &["openrouter.ai"]),
            stand_in(builtin::DEEPSEEK, &["api.deepseek.com"]),
        ])
        .unwrap()
    }

    #[test]
    fn a_table_holds_each_known_built_in_once_and_nothing_else() {
        let table = table();
        assert_eq!(
            table.iter().map(|entry| entry.name).collect::<Vec<_>>(),
            builtin::KEY_FACTS
        );
        assert_eq!(
            table.get(builtin::DEEPSEEK).map(|entry| entry.hosts),
            Some(&["api.deepseek.com"][..])
        );
        assert!(
            table.get(builtin::ADAPTER).is_none(),
            "the harness's source is no vendor's"
        );
        assert_eq!(
            BuiltinSources::new([
                stand_in(builtin::OPENROUTER, &[]),
                stand_in(builtin::OPENROUTER, &[])
            ])
            .unwrap_err(),
            BuiltinFault::Twice("openrouter".to_owned())
        );
        let mut acme = stand_in(builtin::OPENROUTER, &[]);
        acme.name = "acme";
        assert_eq!(
            BuiltinSources::new([acme]).unwrap_err(),
            BuiltinFault::Unknown("acme".to_owned())
        );
        assert_eq!(BuiltinSources::default().iter().count(), 0);
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
            Some(builtin::DEEPSEEK)
        );
        assert_eq!(
            table
                .reading(&[
                    at("https://api.deepseek.com"),
                    at("https://openrouter.ai/api/v1")
                ])
                .map(|entry| entry.name),
            Some(builtin::OPENROUTER),
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
        let entry = table().get(builtin::OPENROUTER).copied().unwrap();
        let clock: Arc<dyn Clock> = Arc::new(orch_types::SystemClock);
        let source = (entry.make)(Arc::new(Nowhere), clock);
        assert_eq!(source.describe().name, "silent");
        assert_eq!((entry.descriptor)().name, "stand-in");
    }
}
