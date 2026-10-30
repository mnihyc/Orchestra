//! The suite every implementation of a contract passes, for tests and for orchctl plugin check.
//!
//! Each extension point of Orch has a contract, built-in implementations, and room for external ones that speak
//! the same contract over the plugin transport. This crate holds the one suite of each contract: the cases that
//! tell an implementation that keeps to it from one that does not. The built-in implementations' tests run them,
//! and `orchctl plugin check` runs them against a program somebody wants to install, so a plugin that passes has
//! met exactly what a built-in one meets.
//!
//! ```text
//! report      one result for each case, in the words an operator reads; what an exit status follows
//! transport   the transport's own cases, for a plugin of any kind, on a link of their own
//! summarizer  a summarizer's cases, through its contract's trait
//! driver      an environment driver's cases, with units made from a request the caller gives
//! ```
//!
//! A suite takes the implementation as its contract's trait, so the same cases run against a built-in
//! implementation and against a plugin behind its bridge (`orch-plugin`). A case that does not apply to what an
//! implementation declares is skipped and says why; a skip is no pass, and the report says it as one.
//!
//! The other families take controlled fixtures in their own formats: a disposable issuer or notification
//! target, an executor's two environments, an adapter's sessions, or a translation example. Native harness
//! behavior and process/file boundaries need their implementation's real-run tests as well; a descriptor
//! check alone cannot prove them. The command requires those contexts rather than inventing a live recipient.

#![forbid(unsafe_code)]

pub mod agent;
pub mod blob;
pub mod driver;
pub mod executor;
pub mod identity;
pub mod native;
pub mod notifier;
pub mod protocol;
pub mod provider;
pub mod report;
pub mod scripted;
pub mod summarizer;
pub mod translator;
pub mod transport;
