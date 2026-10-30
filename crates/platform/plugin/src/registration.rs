//! Load locally configured implementations.
//!
//! A component's configuration lists the external implementations it hosts as `[[plugin]]` entries. This module
//! turns them into running plugins: it checks each entry ([`PluginRegistration::start`]), refuses a name that a
//! built-in implementation of the kind has or that another entry has, leaves alone the entries of kinds that
//! another component hosts (one `orch.toml` serves the node, the API and the worker), and starts the rest. What
//! comes out is a [`Plugins`], the set a component holds for as long as it runs. From it the component takes the
//! implementations as the traits of their contracts ([`Plugins::drivers`], [`Plugins::summarizers`],
//! [`Plugins::notifiers`], [`Plugins::translators`], [`Plugins::adapters`], [`Plugins::executors`], [`Plugins::providers`]), so that the
//! rest of the component cannot tell an external implementation from a built-in one, and the report of each, for
//! Management ([`Plugins::reports`]).
//!
//! A plugin that cannot be started, or is refused, does not stop the component: it is listed as failed, with
//! the reason, and what depends on it is unavailable under its name. An entry that cannot be used at all, such as
//! one that gives both a command and an address, is an error of the configuration and stops the start.
//!
//! A configuration file written for another crate's types has its kind and its restart policy as words; they are
//! read here with the names the contract gives them ([`registration_of`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use orch_driver_contract::driver::EnvironmentDriver;
use orch_model_contract::translator::ProtocolTranslator;
use orch_notifier_contract::notifier::Notifier;
use orch_plugin_contract::registration::{PluginHost, RegistrationError, Restart};
use orch_plugin_contract::{PluginKind, PluginRegistration, PluginStatus};
use orch_summarizer_contract::summarizer::Summarizer;
use orch_types::Json;

use crate::bridges::agent::AdapterFactory;
use crate::bridges::driver::DriverBridge;
use crate::bridges::executor::ExecutorFactory;
use crate::bridges::notifier::NotifierBridge;
use crate::bridges::provider::ProviderBridge;
use crate::bridges::summarizer::SummarizerBridge;
use crate::bridges::translator::TranslatorBridge;
use crate::supervisor::{Options, Plugin, Report};

/// The names of the implementations that are built in, by kind: the ones a plugin may not take.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuiltIn(BTreeMap<PluginKind, BTreeSet<String>>);

impl BuiltIn {
    /// No built-in implementation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the names of the built-in implementations of `kind`.
    #[must_use]
    pub fn with<N: Into<String>>(
        mut self,
        kind: PluginKind,
        names: impl IntoIterator<Item = N>,
    ) -> Self {
        self.0
            .entry(kind)
            .or_default()
            .extend(names.into_iter().map(Into::into));
        self
    }

    fn names(&self, kind: PluginKind) -> Vec<&str> {
        self.0
            .get(&kind)
            .map(|names| names.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }
}

/// Why the entries of a configuration cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// A kind that is none of the extension points.
    #[error(
        "{0:?} is not a kind of extension: provider, driver, adapter, executor, translator, summarizer or notifier"
    )]
    Kind(String),
    /// A restart policy that is none of the three.
    #[error("plugin {name:?}: {value:?} is not a restart policy: never, on-failure or always")]
    Restart {
        /// The plugin's name.
        name: String,
        /// What was written.
        value: String,
    },
    /// An entry that cannot be used.
    #[error(transparent)]
    Entry(#[from] RegistrationError),
    /// Two entries of one kind have one name.
    #[error("two plugins of the kind {kind} are named {name:?}")]
    Duplicate {
        /// The kind.
        kind: PluginKind,
        /// The name.
        name: String,
    },
}

fn word<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Json::String(text.to_owned())).ok()
}

/// The entry for a plugin as a configuration file writes it, with its kind and its restart policy as words.
/// `command` and `connect` are as the file gives them: an empty command is no command, and an empty restart is
/// the default.
pub fn registration_of(
    kind: &str,
    name: &str,
    command: &[String],
    connect: Option<&str>,
    env: &BTreeMap<String, String>,
    restart: &str,
) -> Result<PluginRegistration, LoadError> {
    Ok(PluginRegistration {
        kind: word::<PluginKind>(kind).ok_or_else(|| LoadError::Kind(kind.to_owned()))?,
        name: name.to_owned(),
        command: (!command.is_empty()).then(|| command.to_vec()),
        connect: connect
            .filter(|address| !address.is_empty())
            .map(str::to_owned),
        env: env.clone(),
        restart: if restart.is_empty() {
            None
        } else {
            Some(word::<Restart>(restart).ok_or_else(|| LoadError::Restart {
                name: name.to_owned(),
                value: restart.to_owned(),
            })?)
        },
    })
}

/// The entries of `registrations` that `host` starts, each checked: usable, not named like a built-in
/// implementation of its kind, and named once.
pub fn hosted_by<'a>(
    host: PluginHost,
    registrations: &'a [PluginRegistration],
    builtin: &BuiltIn,
) -> Result<Vec<&'a PluginRegistration>, LoadError> {
    let mut seen = BTreeSet::new();
    let mut hosted = Vec::new();
    for registration in registrations {
        registration.start()?;
        registration.check_name(&builtin.names(registration.kind))?;
        if !seen.insert((registration.kind, registration.name.as_str())) {
            return Err(LoadError::Duplicate {
                kind: registration.kind,
                name: registration.name.clone(),
            });
        }
        if registration.kind.host() == host {
            hosted.push(registration);
        }
    }
    Ok(hosted)
}

/// The external implementations a component hosts, started.
///
/// Dropping it does not stop the plugins at once; [`Plugins::shutdown`] does, and tells the programs to finish.
#[derive(Debug, Clone)]
pub struct Plugins {
    host: PluginHost,
    plugins: Vec<Plugin>,
}

impl Plugins {
    /// Starts the plugins of `registrations` that `host` hosts, on the current runtime. It does not wait for them:
    /// [`Plugins::settled`] does.
    pub async fn start(
        host: PluginHost,
        registrations: &[PluginRegistration],
        builtin: &BuiltIn,
        options: &Options,
    ) -> Result<Self, LoadError> {
        let hosted = hosted_by(host, registrations, builtin)?;
        let mut plugins = Vec::with_capacity(hosted.len());
        for registration in hosted {
            plugins.push(Plugin::start(registration, options.clone()).await?);
        }
        Ok(Self { host, plugins })
    }

    /// The component that hosts them.
    #[must_use]
    pub fn host(&self) -> PluginHost {
        self.host
    }

    /// Waits until the first attempt to start each plugin has come to an end: it runs, or it has failed. A host
    /// that does not want to wait for ever bounds the wait itself.
    pub async fn settled(&self) {
        for plugin in &self.plugins {
            plugin.settled().await;
        }
    }

    /// Every plugin.
    #[must_use]
    pub fn all(&self) -> &[Plugin] {
        &self.plugins
    }

    /// The plugin of `kind` registered as `name`.
    #[must_use]
    pub fn get(&self, kind: PluginKind, name: &str) -> Option<&Plugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.kind() == kind && plugin.name() == name)
    }

    /// How each plugin is doing, for Management.
    #[must_use]
    pub fn reports(&self) -> Vec<Report> {
        self.plugins.iter().map(Plugin::report).collect()
    }

    /// Each plugin's state, as machine facts carry it.
    #[must_use]
    pub fn statuses(&self) -> Vec<PluginStatus> {
        self.plugins
            .iter()
            .map(|plugin| plugin.report().status())
            .collect()
    }

    /// The drivers that have said hello, by their registered names.
    #[must_use]
    pub fn drivers(&self) -> Vec<(String, Arc<dyn EnvironmentDriver>)> {
        self.bridged(PluginKind::Driver, |plugin| {
            DriverBridge::new(plugin).map(|bridge| Arc::new(bridge) as Arc<dyn EnvironmentDriver>)
        })
    }

    /// The summarizers that have said hello, by their registered names.
    #[must_use]
    pub fn summarizers(&self) -> Vec<(String, Arc<dyn Summarizer>)> {
        self.bridged(PluginKind::Summarizer, |plugin| {
            SummarizerBridge::new(plugin).map(|bridge| Arc::new(bridge) as Arc<dyn Summarizer>)
        })
    }

    /// The notifiers that have said hello, by their registered names.
    #[must_use]
    pub fn notifiers(&self) -> Vec<(String, Arc<dyn Notifier>)> {
        self.bridged(PluginKind::Notifier, |plugin| {
            NotifierBridge::new(plugin).map(|bridge| Arc::new(bridge) as Arc<dyn Notifier>)
        })
    }

    /// The translators that have said hello, by their registered names.
    #[must_use]
    pub fn translators(&self) -> Vec<(String, Arc<dyn ProtocolTranslator>)> {
        self.bridged(PluginKind::Translator, |plugin| {
            TranslatorBridge::new(plugin)
                .map(|bridge| Arc::new(bridge) as Arc<dyn ProtocolTranslator>)
        })
    }

    /// The provider programs that have said hello, each supplying its declared optional elements.
    #[must_use]
    pub fn providers(&self) -> Vec<(String, Arc<ProviderBridge>)> {
        self.bridged(PluginKind::Provider, |plugin| {
            ProviderBridge::new(plugin).map(Arc::new)
        })
    }

    /// The adapters that have said hello, by their registered names, each as what makes an adapter for a harness
    /// process: a connector asks the factory for one for every harness it starts.
    #[must_use]
    pub fn adapters(&self) -> Vec<(String, AdapterFactory)> {
        self.bridged(PluginKind::Adapter, AdapterFactory::new)
    }

    /// The executors that have said hello, by their registered names, each as what makes an executor for an
    /// environment.
    #[must_use]
    pub fn executors(&self) -> Vec<(String, ExecutorFactory)> {
        self.bridged(PluginKind::Executor, ExecutorFactory::new)
    }

    fn bridged<T, E>(
        &self,
        kind: PluginKind,
        bridge: impl Fn(Plugin) -> Result<T, E>,
    ) -> Vec<(String, T)> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.kind() == kind)
            .filter_map(|plugin| {
                bridge(plugin.clone())
                    .ok()
                    .map(|bridged| (plugin.name().to_owned(), bridged))
            })
            .collect()
    }

    /// Stops every plugin: programs are told to finish and killed if they have not within ten seconds, services
    /// are let go of.
    pub async fn shutdown(&self) {
        for plugin in &self.plugins {
            plugin.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, name: &str) -> PluginRegistration {
        registration_of(
            kind,
            name,
            &["/opt/orch-plugins/p".to_owned()],
            None,
            &BTreeMap::new(),
            "",
        )
        .unwrap()
    }

    #[test]
    fn an_entry_is_read_from_the_words_a_configuration_file_uses() {
        let read = registration_of(
            "driver",
            "firecracker",
            &["/opt/orch-plugins/fc".to_owned(), "--quiet".to_owned()],
            None,
            &BTreeMap::from([("FC_KERNEL".to_owned(), "/k".to_owned())]),
            "always",
        )
        .unwrap();
        assert_eq!(read.kind, PluginKind::Driver);
        assert_eq!(read.restart, Some(Restart::Always));
        assert_eq!(read.command.as_deref().map(<[String]>::len), Some(2));
        let attached = registration_of(
            "notifier",
            "chat",
            &[],
            Some("tcp://127.0.0.1:7411"),
            &BTreeMap::new(),
            "",
        )
        .unwrap();
        assert_eq!(attached.command, None);
        assert_eq!(attached.restart, None, "an empty policy is the default");
        assert_eq!(
            registration_of("frobnicator", "x", &[], None, &BTreeMap::new(), ""),
            Err(LoadError::Kind("frobnicator".into()))
        );
        assert!(matches!(
            registration_of(
                "driver",
                "x",
                &["p".into()],
                None,
                &BTreeMap::new(),
                "sometimes"
            ),
            Err(LoadError::Restart { .. })
        ));
    }

    #[test]
    fn a_component_loads_the_entries_of_the_kinds_it_hosts_and_leaves_the_others_to_their_hosts() {
        let registrations = [
            entry("driver", "firecracker"),
            entry("summarizer", "rules"),
            entry("notifier", "chat"),
            entry("adapter", "grok"),
        ];
        let names = |host| {
            hosted_by(host, &registrations, &BuiltIn::new())
                .unwrap()
                .iter()
                .map(|registration| registration.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(PluginHost::Node), ["firecracker"]);
        assert_eq!(names(PluginHost::Api), ["rules"]);
        assert_eq!(names(PluginHost::Worker), ["chat"]);
        assert_eq!(names(PluginHost::Connector), ["grok"]);
    }

    #[test]
    fn an_entry_that_cannot_be_used_stops_the_start_whatever_component_hosts_it() {
        let mut both = entry("driver", "fc");
        both.connect = Some("unix:/run/fc.sock".into());
        assert!(matches!(
            hosted_by(PluginHost::Api, &[both], &BuiltIn::new()),
            Err(LoadError::Entry(RegistrationError::CommandOrConnect(_)))
        ));
        let elsewhere = entry("notifier", "chat");
        assert!(
            matches!(
                hosted_by(
                    PluginHost::Api,
                    &[elsewhere.clone(), elsewhere],
                    &BuiltIn::new()
                ),
                Err(LoadError::Duplicate {
                    kind: PluginKind::Notifier,
                    ..
                })
            ),
            "a name is used once, even in a configuration for another component"
        );
    }

    #[test]
    fn a_plugin_may_not_take_the_name_of_a_built_in_implementation_of_its_kind_but_may_of_another()
    {
        let builtin = BuiltIn::new()
            .with(PluginKind::Driver, ["docker", "host"])
            .with(PluginKind::Notifier, ["email"]);
        assert!(matches!(
            hosted_by(PluginHost::Node, &[entry("driver", "docker")], &builtin),
            Err(LoadError::Entry(RegistrationError::NameTaken { .. }))
        ));
        assert_eq!(
            hosted_by(PluginHost::Api, &[entry("summarizer", "docker")], &builtin)
                .unwrap()
                .len(),
            1
        );
    }
}
