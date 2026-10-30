//! Browser WebSocket contracts, including terminal sockets.
//!
//! One WebSocket per browser tab carries every live update, and every
//! terminal has a socket of its own. This package is what the server in
//! the API and the client in the browser agree on: each frame of both
//! directions as a type with the exact form it has on the wire, and the
//! rules that need no socket to be stated.
//!
//! The protocol is state-based. The server tells the client which rows
//! changed, not what happened; HTTP remains the way to load and to
//! change anything, and the socket never accepts a mutation. A durable
//! feed numbers its rows, so that a client can say how far it has come
//! and be sent everything after, none skipped; an ephemeral feed
//! delivers events while somebody listens and nothing of what came
//! before.
//!
//! Nothing here opens a connection, reads a clock or keeps a file. The
//! arithmetic is given as plain values that are told what happened and
//! say what to do, so that the two ends are built on the same rules and
//! each can be tested without the other: what a client does with a frame
//! given how far it has come ([`sync`]), how a growing text is followed
//! by offsets and how a server merges what a slow reader would be sent
//! ([`delta`]), how feeds are named ([`feeds`]), and the limits, close
//! codes and reconnection of a socket ([`version`]).
//!
//! The types of the browser are generated from these: every frame
//! derives `JsonSchema`, and [`schemas`] visits them.

#![forbid(unsafe_code)]

pub mod browser;
pub mod delta;
pub mod document_tree;
pub mod documents;
pub mod feeds;
pub mod foreign;
pub mod frames;
pub mod pacing;
pub mod presence;
pub mod sync;
pub mod terminal;
pub mod typing;
pub mod version;

/// Visits the schema of every type that crosses the realtime socket or a
/// terminal's, so that the generator of the browser's types includes them.
///
/// The frames of both directions are visited, and with them the types of
/// everything they hold. The data of a `sync` or an `event` frame is a JSON
/// object whose members depend on the feed, so the types of that data are
/// visited here by name; a frame's own schema cannot reach them.
pub fn schemas(generator: &mut schemars::SchemaGenerator) {
    generator.subschema_for::<frames::ClientFrame>();
    generator.subschema_for::<frames::ServerFrame>();
    generator.subschema_for::<terminal::TerminalClientFrame>();
    generator.subschema_for::<terminal::TerminalServerFrame>();

    generator.subschema_for::<feeds::ThreadData>();
    generator.subschema_for::<feeds::TopicData>();
    generator.subschema_for::<feeds::EnvData>();
    generator.subschema_for::<feeds::OrgData>();
    // A row of the tree is flattened into the listing that carries it beside a minimal row; the
    // browser keeps the readable row by its own name.
    generator.subschema_for::<orch_api_contract::nodes::TreeNode>();
    generator.subschema_for::<feeds::UserData>();
    generator.subschema_for::<feeds::HealthData>();
    generator.subschema_for::<feeds::ChatData>();
    generator.subschema_for::<documents::DocData>();
    generator.subschema_for::<document_tree::DocumentTreeData>();
    generator.subschema_for::<document_tree::DocumentTreeView>();

    generator.subschema_for::<feeds::UserEvent>();
    generator.subschema_for::<typing::TypingNotice>();
    generator.subschema_for::<feeds::ResourceEvent>();
    generator.subschema_for::<feeds::Watched<orch_api_contract::processes::ResourceSample>>();
    generator.subschema_for::<feeds::Watched<feeds::ProcsData>>();
    generator.subschema_for::<feeds::Watched<feeds::TerminalsData>>();
    generator.subschema_for::<feeds::Watched<orch_api_contract::ports::EnvironmentPorts>>();
    generator.subschema_for::<feeds::Watched<feeds::GitData>>();
    generator.subschema_for::<feeds::JobData>();
    generator.subschema_for::<feeds::ForkData>();
    generator.subschema_for::<feeds::Watched<feeds::DirData>>();
    generator.subschema_for::<feeds::Watched<feeds::BrowsersData>>();
    generator.subschema_for::<feeds::Watched<feeds::DisplaysData>>();
    generator.subschema_for::<orch_api_contract::threads::RawEvent>();
    generator.subschema_for::<feeds::RawData>();
    // Job results are carried as JSON; the browser reads the native End outcome by its contract name.
    generator.subschema_for::<orch_api_contract::background::BackgroundEndResult>();
}
