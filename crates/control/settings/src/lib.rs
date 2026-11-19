//! Settings evaluation over the shared catalogue.
//!
//! The registry, the rules by which the values of several scopes combine and
//! the shapes of a resolved value are the settings contract's. This package is
//! what needs the database: it reads the rows that are set along the chain of
//! scopes for a place, hands them to those rules, and answers.
//!
//! - [`resolver`] reads a [`Place`] (the system, a node, an environment, a
//!   provider's runtime) in a number of statements that does not depend on how
//!   many keys are asked for, and answers one key, all of them, or one typed
//!   value ([`resolver::get`]).
//! - [`mod@write`] writes a value at the system or at a node, or takes it back, in
//!   the caller's transaction and with its audit row. [`validation`] says what
//!   a write needs of whoever makes it, so that the caller asks the access
//!   engine the right question; who may write is not decided here.
//! - [`secrets`] stores the secrets of a node or of an environment sealed,
//!   lists their names down the hierarchy and opens them for the engine.
//! - [`merge`] assembles the layers the contract's rules combine; [`locks`]
//!   says what a lock above means for a write; [`provenance`] is the answer
//!   with where each part of it comes from; [`supplied`] holds the defaults
//!   that come from somewhere other than the registry.
//!
//! Nothing is cached: a change is seen by the next read.

#![forbid(unsafe_code)]

pub mod locks;
pub mod merge;
pub mod provenance;
pub mod resolver;
pub mod secrets;
pub mod supplied;
pub mod validation;
pub mod write;

use orch_db::DbError;
use orch_types::Id;

pub use provenance::SettingView;
pub use resolver::{
    ProviderAt, Snapshot, Wanted, cleanup_at_hosts, get, get_at_providers, resolve, resolve_all,
};
mod provider_runtime;
pub use provider_runtime::{
    ProviderSignIn, credential_home, credential_home_choice, harness_home_owned,
    machine_home_owned, profile_home, profile_home_choice, profile_home_in, provider_home,
    provider_home_choice, provider_sign_in,
};

/// What a setting is read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    /// The system itself: its own rows.
    System,
    /// An org, a project, a topic or a thread: the rows of the system and of
    /// every node from its root org down to it.
    Node(Id),
    /// One environment: the rows on the path of its home node, then its own
    /// values.
    Environment(Id),
    /// One provider's runtime: the rows of the system and of the orgs
    /// down to its home, then its own values.
    Provider(Id),
    /// One host's maintenance policy, inherited from the system.
    Host(Id),
}

/// Where a setting is written: a row of the system or of a node. The own
/// values of an environment and of a provider's runtime are written with the
/// environment and the provider themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// The system.
    System,
    /// A node.
    Node(Id),
}

impl From<Target> for Place {
    fn from(target: Target) -> Self {
        match target {
            Target::System => Self::System,
            Target::Node(id) => Self::Node(id),
        }
    }
}

/// Why a setting could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// There is no such node, environment or provider, or it was deleted.
    #[error("there is no such place")]
    NotFound,
    /// There is no such key in the registry.
    #[error("{0:?} is no setting")]
    UnknownKey(String),
    /// The value that holds is not of the type that was asked for.
    #[error("the value of {key} is not of the type asked for: {reason}")]
    NotOfThatType {
        /// The key.
        key: String,
        /// What the conversion said.
        reason: String,
    },
    /// Something stored does not read as it should: a kind of node the
    /// schema does not allow.
    #[error("what is stored for {what} cannot be read")]
    Unreadable {
        /// What it was.
        what: &'static str,
    },
}
