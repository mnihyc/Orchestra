//! External plugin transport, supervision and trait bridges.
//!
//! An extension point of Orch can be implemented outside the workspace, in any language, as a program that speaks
//! one line of JSON at a time on its standard streams or on a local socket. This package is the host's side of
//! that. A component that hosts a kind of extension gives it the `[[plugin]]` entries of its configuration, and
//! gets back the implementations as the traits of their contracts, so that the code that uses them cannot tell an
//! external one from a built-in one:
//!
//! ```text
//! registration   the entries of the configuration, checked, and the set of plugins they start
//! transport      the lines: a program started and talked to over its standard streams, or a socket attached to
//! negotiation    hello: the versions, the kind, the methods that are required and those that are not
//! requests       overlapping requests matched with their answers; progress, cancellation, deadlines, pings
//! supervisor     one plugin kept: restarted with back-off, its log kept apart, stopped cleanly
//! bridges        each plugin as the Rust trait of its contract: agent, driver, executor, translator,
//!                summarizer, notifier, provider
//! ```
//!
//! The rules of the transport themselves (the messages, the versions, the deadlines, the pings, the pauses) are
//! the contract's, in `orch-plugin-contract`; the time they are judged by is a clock that the host passes in.
//!
//! A component starts its plugins once and keeps them for as long as it runs:
//!
//! ```ignore
//! let options = Options::new(env!("CARGO_PKG_VERSION"));
//! let plugins = Plugins::start(PluginHost::Api, &registrations, &builtin, &options).await?;
//! plugins.settled().await;
//! for (name, summarizer) in plugins.summarizers() { /* an Arc<dyn Summarizer> like a built-in one */ }
//! for report in plugins.reports() { /* what Management lists */ }
//! plugins.shutdown().await;
//! ```

#![forbid(unsafe_code)]

pub mod bridges;
pub mod negotiation;
pub mod registration;
pub mod requests;
pub mod schema;
pub mod supervisor;
pub mod transport;

pub use bridges::BridgeError;
pub use registration::{BuiltIn, LoadError, Plugins, hosted_by, registration_of};
pub use requests::{Call, CallError, Observer, RemoteError};
pub use supervisor::{Options, Plugin, Report, Timing};
