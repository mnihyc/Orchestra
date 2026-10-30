//! Routes components.
//!
//! One module for each part of the API. A module binds its handlers in a
//! `mount` function; [`mount`] calls them all, so the application serves
//! exactly what the modules bind.

pub mod access;
pub mod admin;
pub mod approvals;
pub mod auth;
pub mod browsers;
pub mod capabilities;
pub mod chats;
mod destinations;
pub mod directory;
pub mod displays;
pub mod document_tree;
pub mod documents;
pub mod downloads;
pub mod environments;
pub mod everything;
pub mod exports;
pub mod first_steps;
pub mod fleet;
pub mod forks;
pub mod imports;
pub mod links;
pub mod loss;
pub mod meta;
pub mod nodes;
pub mod notifications;
pub mod personal;
pub mod port_entry;
pub mod port_requests;
pub mod ports;
pub mod processes;
pub mod profiles;
pub mod resources;
pub mod search;
pub mod settings;
pub mod share;
pub mod storage;
pub mod terminals;
pub mod threads;
pub mod topics;
pub(crate) mod trace;
pub mod transfer;
pub mod trust;
pub mod usage;
pub mod users;
pub mod workspace;

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
    meta::mount(registry);
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
    downloads::mount(registry);
    notifications::mount(registry);
    terminals::mount(registry);
    browsers::mount(registry);
    displays::mount(registry);
    topics::mount_announcements(registry);
    crate::input_schedules::mount(registry);
    capabilities::mount(registry);
    usage::mount(registry);
    storage::mount(registry);
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
