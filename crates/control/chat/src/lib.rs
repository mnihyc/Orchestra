//! People's chats: messages, following, inspection and forwarding.
//!
//! A chat is people talking to people. A node has one from the moment it is
//! made; two people have one direct chat in a root org, and a member makes a
//! group for the people they name. Nothing in a chat reaches an agent unless a
//! person forwards it there or shares the chat with it.
//!
//! Everything here is a function over a database connection that the caller
//! holds, so that a request, its notifications and the announcement that
//! tells the sockets are one transaction. Who may do what is the access
//! engine's ([`orch_authz::chats`]); the modules here ask it, once per request,
//! through [`chats::load`], and keep the rules that are not about access:
//!
//! - [`chats`] loads a chat with what the caller may do in it, makes direct
//!   and group chats, and keeps their people.
//! - [`messages`] is posting, editing, deleting, reacting and pinning, each a
//!   row stamped in the chat's own sequence; [`mentions`] finds the people a
//!   message names; [`quotes`] counts the items of a thread that its chat
//!   quotes; [`attachments`] holds the images and files a message carries.
//! - [`following`] is a person's level in a chat, what is unread for them and
//!   their draft; [`notifications`] is what a message writes to the Inbox of
//!   those it concerns.
//! - [`forward`] copies messages into other chats and to people;
//!   [`inspection`] is the one trace an admin leaves by reading a chat they
//!   are not in; [`written`] is a chat written out for whoever reads it as
//!   text.
//! - [`views`] turns rows into the types of the API, given the names of the
//!   people they mention, and [`redaction`] masks what must not be stored.

#![forbid(unsafe_code)]

pub mod attachments;
pub mod audiences;
pub mod chats;
pub mod error;
pub mod following;
pub mod forward;
pub mod inspection;
pub mod mentions;
pub mod messages;
pub mod notifications;
pub mod quotes;
pub mod redaction;
pub mod views;
pub mod written;

pub use error::{Error, Why};

use chrono::{DateTime, Utc};
use orch_authz::Principal;
use orch_db::audit::Origin;

/// Who does something, when, and where the request came from: what every
/// change to a chat is made under.
#[derive(Debug, Clone, Copy)]
pub struct Acting<'a> {
    /// The caller, with what is known of them.
    pub principal: &'a Principal,
    /// The moment of the request, by the application's clock.
    pub now: DateTime<Utc>,
    /// Where the request came from, for the audit rows it causes.
    pub origin: &'a Origin,
}

impl Acting<'_> {
    /// The person who acts.
    #[must_use]
    pub fn user(&self) -> orch_types::Id {
        self.principal.user()
    }
}
