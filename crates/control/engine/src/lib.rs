//! Durable product orchestration hosted inside the API.
//!
//! The thread engine's core is here: an input becomes a turn, and a turn
//! becomes a conversation. A person's input is queued (`inputs`), checked
//! against everything that stands between it and a harness (`preflight`),
//! turned into a turn with a lease and a segment (`leases`, `sessions`),
//! carried to the harness on the commands of the machine link (`turns`,
//! `harnesses`), and what the harness does comes back as events that are
//! applied exactly once (`ingest`, `approvals`).
//!
//! **What the engine keeps and what it asks.** Everything it remembers is
//! in the database and is changed by compare-and-set, so a process that is
//! restarted at any moment finds its work where it was left and goes on.
//! What it needs of the rest of the system it asks through three small
//! traits: [`commands::Commands`] (send a command to a machine and learn how
//! it ended), [`access::ResourceAccess`] (may this person use this
//! provider, this profile, this environment) and [`live::LiveSink`] (growing
//! text, forwarded at once). The decisions about nodes it asks of
//! `orch-authz` itself.
//!
//! **What is left to later parts** is said where it would be done, in the
//! module that would do it: writing files into a workspace, evaluating usage
//! caps, the tools of the `orch` server. Until they exist each of those
//! blocks or refuses, and never lets a turn through unchecked.

#![forbid(unsafe_code)]

// In-crate regression tests share the integration fixture's real database and scripted link.
#[cfg(test)]
extern crate self as orch_engine;

pub mod access;
pub mod adoption;
pub mod agent_history;
pub mod approval_review;
pub mod approval_rules;
pub mod approvals;
pub mod archive;
mod audit_facts;
pub mod background;
pub mod binding;
pub mod browsers;
mod cache_cleanup;
pub mod calls;
pub mod capabilities;
pub mod caps;
pub mod commands;
pub mod config;
pub mod context_fill;
pub mod continuations;
pub mod copies;
pub mod credentials;
pub mod delivery;
pub mod displays;
pub mod document_tree;
pub mod engine;
pub mod environments;
pub mod error;
pub mod export_layout;
pub mod exports;
pub mod fork_history;
pub mod fork_plan;
mod fork_source;
pub mod git_work;
pub mod goals;
pub mod harnesses;
pub mod history;
mod history_pages;
pub mod hosting;
pub mod ids;
pub mod imports;
pub mod ingest;
pub mod initial_files;
pub mod inputs;
mod instruction_sources;
pub mod instructions;
pub mod isolation;
pub mod jobs;
mod kept_images;
pub mod leases;
pub mod links;
pub mod live;
pub mod locks;
pub mod loss;
pub mod manifest;
pub mod material;
pub mod mcp;
pub mod membership;
pub mod membership_requests;
pub mod memory;
pub mod model_inputs;
pub mod models;
pub mod mount_copies;
pub mod mounts;
pub mod native_actions;
pub mod network;
pub mod notifications;
pub mod operation_access;
pub mod parcels;
pub mod paste;
pub mod places;
pub mod port_capabilities;
pub mod ports;
pub mod preflight;
pub mod preparation;
pub mod programs;
pub mod projections;
pub mod provider_fact_reads;
pub mod provider_policy;
pub mod provider_recovery;
pub mod provider_runtime;
pub mod provider_setup;
pub mod provider_sources;
pub mod providers;
pub mod rates;
pub mod raw_events;
pub mod reaper;
pub mod reconciliation;
pub mod recovery;
pub mod recycle;
pub mod redaction;
pub mod related;
pub mod reports;
pub mod request_actions;
pub mod resolved;
pub mod resource_facts;
pub mod resource_pause;
pub mod retention;
pub mod schedule;
pub mod selection;
pub mod semantic;
pub mod sessions;
pub mod sharing;
pub mod shells;
pub mod side_threads;
pub mod skills;
pub mod source;
mod source_retention;
pub mod storage;
mod streaming;
pub mod summaries;
pub mod temporary_environments;
pub mod thread_move;
pub mod token_density;
pub mod tools;
pub mod trust;
pub mod turns;
pub mod unit;
pub mod unit_subjects;
pub mod updates;
pub mod views;
mod volume_carry;
pub mod written;

pub use engine::{Engine, EngineParts};
pub use error::EngineError;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;
