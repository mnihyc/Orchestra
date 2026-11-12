//! Routes components.
//!
//! One module for each part of the API. A module binds its handlers in a
//! `mount` function; [`mount`] calls them all, so the application serves
//! exactly what the modules bind.

use crate::registry::Registry;

use crate::sign_in;

use orch_db::secrets::MasterKey;
/// Gives the deployment's master key to every place in the requests that seals or opens a stored
/// secret with it: an authenticator at sign-in, the secrets of the settings, and the keys of
/// providers. A program started from a secrets directory calls this once, as it calls [`mount`];
/// a place that was not given the key looks for a file of its own, and a program that was not
/// started in a container finds none.
pub fn install_master_key(key: &MasterKey) {
    sign_in::codes::install_master_key(key.clone());
    settings::install_master_key(key.clone());
    resources::sealing::install_master_key(key.clone());
    // What an agent writes in a chat is masked with the same key as what a person writes.
    let _installed = orch_engine::tools::chat_message::install_master_key(key.clone());
    // What a harness's frames carry is masked with the secrets of the thread's place before it is kept.
    let _kept = orch_engine::raw_events::install_master_key(key.clone());
    // Search: indexed item text uses the deployment key to mask resolved secrets.
    let _indexed = orch_engine::projections::install_master_key(key.clone());
}

/// Binds every request that has a handler.
pub fn mount(registry: &mut Registry) {
    crate::ask::mount(registry);
    meta::mount(registry);
    gateways::mount(registry);
    auth::mount(registry);
    nodes::mount(registry);
    access::mount(registry);
    settings::mount(registry);
    users::mount(registry);
    profiles::mount(registry);
    directory::mount(registry);
    personal::mount(registry);
    first_steps::mount(registry);
    threads::mount(registry);
    forks::mount(registry);
    approvals::mount(registry);
    resources::mount(registry);
    environments::mount(registry);
    loss::mount(registry);
    chats::mount(registry);
    documents::mount(registry);
    document_tree::mount(registry);
    topics::mount(registry);
    admin::mount(registry);
    admin_messages::mount(registry);
    downloads::mount(registry);
    duplicates::mount(registry);
    notifications::mount(registry);
    terminals::mount(registry);
    browsers::mount(registry);
    displays::mount(registry);
    topics::mount_announcements(registry);
    crate::input_schedules::mount(registry);
    capabilities::mount(registry);
    usage::mount(registry);
    storage::mount(registry);
    summaries::mount(registry);
    workspace::mount(registry);
    processes::mount(registry);
    search::mount(registry);
    links::mount(registry);
    everything::mount(registry);
    share::mount(registry);
    exports::mount(registry);
    imports::mount(registry);
    transfer::mount(registry);
    ports::mount(registry);
    port_entry::mount(registry);
    port_requests::mount(registry);
    trust::mount(registry);
}

#[path = "access.rs"]
pub mod access;

#[path = "admin.rs"]
pub mod admin;

#[path = "admin_messages.rs"]
pub mod admin_messages;

#[path = "approvals.rs"]
pub mod approvals;

#[path = "auth.rs"]
pub mod auth;

#[path = "browsers.rs"]
pub mod browsers;

#[path = "capabilities.rs"]
pub mod capabilities;

#[path = "chats.rs"]
pub mod chats;

#[path = "directory.rs"]
pub mod directory;

#[path = "displays.rs"]
pub mod displays;

#[path = "document_tree.rs"]
pub mod document_tree;

#[path = "documents.rs"]
pub mod documents;

#[path = "downloads.rs"]
pub mod downloads;

#[path = "duplicates.rs"]
pub mod duplicates;

#[path = "environments.rs"]
pub mod environments;

#[path = "everything.rs"]
pub mod everything;

#[path = "exports.rs"]
pub mod exports;

#[path = "first_steps.rs"]
pub mod first_steps;

#[path = "fleet.rs"]
pub mod fleet;

#[path = "forks.rs"]
pub mod forks;

#[path = "gateways.rs"]
pub mod gateways;

#[path = "imports.rs"]
pub mod imports;

#[path = "interactive_tickets.rs"]
pub mod interactive_tickets;

#[path = "links.rs"]
pub mod links;

#[path = "loss.rs"]
pub mod loss;

#[path = "meta.rs"]
pub mod meta;

#[path = "nodes.rs"]
pub mod nodes;

#[path = "notifications.rs"]
pub mod notifications;

#[path = "personal.rs"]
pub mod personal;

#[path = "port_entry.rs"]
pub mod port_entry;

#[path = "port_requests.rs"]
pub mod port_requests;

#[path = "ports.rs"]
pub mod ports;

#[path = "processes.rs"]
pub mod processes;

#[path = "profiles.rs"]
pub mod profiles;

#[path = "resources.rs"]
pub mod resources;

#[path = "search.rs"]
pub mod search;

#[path = "settings.rs"]
pub mod settings;

#[path = "share.rs"]
pub mod share;

#[path = "storage.rs"]
pub mod storage;

#[path = "summaries.rs"]
pub mod summaries;

#[path = "terminals.rs"]
pub mod terminals;

#[path = "threads.rs"]
pub mod threads;

#[path = "topics.rs"]
pub mod topics;

#[path = "trace.rs"]
pub mod trace;

#[path = "transfer.rs"]
pub mod transfer;

#[path = "trust.rs"]
pub mod trust;

#[path = "usage.rs"]
pub mod usage;

#[path = "users.rs"]
pub mod users;

#[path = "workspace.rs"]
pub mod workspace;
