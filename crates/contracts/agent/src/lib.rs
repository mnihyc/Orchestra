//! Neutral harness adapter contract and declarations.
//!
//! Orch's business logic is written against this crate and never against a harness. An adapter drives one
//! harness kind and says what it is in a descriptor ([`descriptor`]); everything Orch offers for a harness,
//! from the options of the "new thread" dialog to whether "steer" is shown, is read from that declaration.
//! What an adapter does is the [`adapter::AgentAdapter`] trait, and what it reports is the neutral
//! vocabulary of [`events`].
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`descriptor`] | The declaration: arrangements, capabilities, modes, sign-in, versions |
//! | [`arrangement`] | The rule that chooses among arrangements |
//! | [`parameters`] | Parameters and their eligibility; what a model offers |
//! | [`session`] | Sessions, turns, approvals, where the agent's notes are kept and by whom |
//! | [`events`] | What a harness does, as neutral events |
//! | [`credentials`], [`login`] | Credential forms and their exchange; sign-in methods and their state |
//! | [`discovery`] | Sessions, models and MCP servers the harness already has |
//! | [`history`] | Archives, import fidelity, reports, the method that carries a conversation elsewhere |
//! | [`facts`] | A profile, a machine and an environment as they report themselves |
//! | [`conformance`] | What an adapter tells the shared native cases about its program |
//! | [`loopback`] | The addresses the connector serves a harness on |
//! | [`adapter`] | The trait, its errors, the sink for events |

#![forbid(unsafe_code)]

pub mod adapter;
pub mod arrangement;
pub mod background;
pub mod conformance;
pub mod credentials;
pub mod descriptor;
pub mod discovery;
pub mod events;
pub mod facts;
pub mod flow;
pub mod history;
pub mod login;
pub mod loopback;
pub mod native;
pub mod parameters;
pub mod requests;
pub mod review;
pub mod session;
pub mod tool_answers;

/// Registers the schemas that the API document refers to by name and the API's own contract cannot supply, since
/// this contract is above it: the descriptor of an agent program. The generators of the document call it with
/// the realtime wire types.
pub fn schemas(generator: &mut schemars::SchemaGenerator) {
    let _ = generator.subschema_for::<descriptor::AgentDescriptor>();
}
