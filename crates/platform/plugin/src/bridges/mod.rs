//! Bridges components.
//!
//! A bridge makes a plugin an ordinary implementation of its contract's trait, so that the code that uses the
//! trait cannot tell an external implementation from a built-in one. There is one for each kind. Each
//! maps the trait's methods to the methods of the plugin transport, which are the trait's methods in
//! `snake_case` (for adapters, the agent-role commands of the link), and the trait's arguments to the parameters
//! of the request.
//!
//! **The parameters.** A method that takes one argument that is a structure sends that structure as its
//! parameters. A method of several arguments sends an object with a member for each, named as the trait names it.
//! A method that takes none sends no parameters. A result is the structure the trait returns; where the trait
//! returns a list, the result is an object with the list in a member named for what it holds, so that a later
//! version can add members beside it. Each bridge module has the table of its methods.
//!
//! **What a plugin does not implement.** A plugin declares the methods it implements when it says hello, and a
//! method it did not declare is never called: the bridge answers as the trait's own default does for an
//! implementation that does not have it. An optional operation is therefore unsupported, a validation passes, a
//! preparation of an image is nothing to do, and what depends on it is shown as unavailable.
//!
//! **What goes wrong.** An error the plugin answers with is mapped to the trait's error by its code. A plugin that
//! is down, went away during the request, or did not answer in time is unavailable, and that is retryable where
//! the trait's error can say so.
//!
//! **What the trait says without waiting.** `describe` and the other methods of the traits that are not
//! asynchronous answer from what the plugin declared when it said hello. The ones that cannot, because they
//! must ask the plugin, wait for the answer on the calling thread ([`crate::requests::Call::wait_blocking`]): the
//! translator's, and the notifier's `link_url`.

pub mod agent;
pub mod driver;
pub mod executor;
pub mod notifier;
pub mod provider;
pub mod summarizer;
pub mod translator;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use orch_agent_contract::adapter::AdapterError;
use orch_plugin_contract::PluginKind;
use orch_plugin_contract::negotiation::Negotiated;
use orch_types::Json;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::requests::{CallError, UNAVAILABLE};
use crate::supervisor::Plugin;

/// Why a plugin cannot be made into an implementation of a trait.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// It has not said hello, so what it is is not known.
    #[error("the plugin {0:?} has not said hello yet, so what it is is not known")]
    NotNegotiated(String),
    /// It is a plugin of another kind.
    #[error("the plugin {name:?} is a {found}, and this bridge is for a {expected}")]
    WrongKind {
        /// The plugin's name.
        name: String,
        /// The kind the bridge is for.
        expected: PluginKind,
        /// The kind the plugin is.
        found: PluginKind,
    },
    /// Its descriptor cannot be read as its kind's descriptor.
    #[error("the descriptor of the plugin {name:?} cannot be read: {detail}")]
    Descriptor {
        /// The plugin's name.
        name: String,
        /// What is wrong with it.
        detail: String,
    },
}

/// A text that lives for the rest of the process. The traits answer `&'static str` for the name of a harness
/// kind, an executor and the two ends of a translator; a plugin's are known only when it has said hello. A text
/// is kept once, however often it is asked for, so a plugin that is restarted does not leak again.
pub(crate) fn interned(text: &str) -> &'static str {
    static KNOWN: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(kept) = known.get(text) {
        return kept;
    }
    let kept: &'static str = Box::leak(text.to_owned().into_boxed_str());
    known.insert(kept);
    kept
}

/// Checks that `plugin` is of `kind` and has said hello.
pub(crate) fn negotiated_as(
    plugin: &Plugin,
    kind: PluginKind,
) -> Result<Arc<Negotiated>, BridgeError> {
    if plugin.kind() != kind {
        return Err(BridgeError::WrongKind {
            name: plugin.name().to_owned(),
            expected: kind,
            found: plugin.kind(),
        });
    }
    plugin
        .negotiated()
        .ok_or_else(|| BridgeError::NotNegotiated(plugin.name().to_owned()))
}

/// A plugin's descriptor, read as the descriptor of its kind, and read again when the plugin has said hello
/// anew, which is when it may have changed.
pub(crate) struct Described<T> {
    plugin: Plugin,
    seen: Mutex<(Arc<Negotiated>, Arc<T>)>,
}

impl<T> Clone for Described<T> {
    fn clone(&self) -> Self {
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        Self {
            plugin: self.plugin.clone(),
            seen: Mutex::new((Arc::clone(&seen.0), Arc::clone(&seen.1))),
        }
    }
}

impl<T> std::fmt::Debug for Described<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Described")
            .field("plugin", &self.plugin.name())
            .finish_non_exhaustive()
    }
}

impl<T: DeserializeOwned> Described<T> {
    pub(crate) fn new(plugin: Plugin, kind: PluginKind) -> Result<Self, BridgeError> {
        let negotiated = negotiated_as(&plugin, kind)?;
        let descriptor = Self::read(&plugin, &negotiated)?;
        Ok(Self {
            seen: Mutex::new((negotiated, Arc::new(descriptor))),
            plugin,
        })
    }

    fn read(plugin: &Plugin, negotiated: &Negotiated) -> Result<T, BridgeError> {
        serde_json::from_value(negotiated.descriptor.clone()).map_err(|error| {
            BridgeError::Descriptor {
                name: plugin.name().to_owned(),
                detail: error.to_string(),
            }
        })
    }

    /// The descriptor as of the last hello.
    pub(crate) fn get(&self) -> Arc<T> {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(now) = self.plugin.negotiated()
            && !Arc::ptr_eq(&now, &seen.0)
            // A descriptor that was accepted at the handshake reads; one that does not is not taken.
            && let Ok(descriptor) = Self::read(&self.plugin, &now)
        {
            *seen = (now, Arc::new(descriptor));
        }
        Arc::clone(&seen.1)
    }
}

/// The parameters of a request, written.
pub(crate) fn params<T: Serialize>(method: &str, value: &T) -> Result<Json, CallError> {
    serde_json::to_value(value).map_err(|error| CallError::Malformed {
        method: method.to_owned(),
        detail: format!("the parameters cannot be written: {error}"),
    })
}

/// The result of a request, read as what the contract states for the method.
pub(crate) fn result<T: DeserializeOwned>(method: &str, value: Json) -> Result<T, CallError> {
    serde_json::from_value(value).map_err(|error| CallError::Malformed {
        method: method.to_owned(),
        detail: error.to_string(),
    })
}

/// A member of an object result, read as what the contract states for it.
pub(crate) fn member<T: DeserializeOwned>(
    method: &str,
    mut value: Json,
    name: &str,
) -> Result<T, CallError> {
    let taken = value.get_mut(name).map(Json::take);
    serde_json::from_value(taken.unwrap_or(Json::Null)).map_err(|error| CallError::Malformed {
        method: method.to_owned(),
        detail: format!("the member {name:?}: {error}"),
    })
}

/// A request's parameters that are an object with the members `members`, the ones that are `null` left out.
pub(crate) fn object<const N: usize>(members: [(&str, Json); N]) -> Json {
    Json::Object(
        members
            .into_iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

/// The error of an adapter, and of an executor which is started on an adapter's behalf, for what the program
/// answered. The codes are those of the link's commands, which the contract's error knows; any other is passed on
/// as it is. A program that is down is an adapter that cannot be reached, and that is retryable.
pub(crate) fn adapter_error(error: CallError) -> AdapterError {
    match error {
        CallError::Failed(remote) => {
            AdapterError::from_code(&remote.code, &remote.message, remote.retryable)
        }
        CallError::Unavailable { .. } | CallError::Lost { .. } => AdapterError::Other {
            code: UNAVAILABLE.to_owned(),
            message: error.to_string(),
            retryable: true,
        },
        CallError::Timeout { .. } => AdapterError::Timeout,
        CallError::Cancelled | CallError::Malformed { .. } => {
            AdapterError::Internal(error.to_string())
        }
    }
}
