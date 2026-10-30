//! The single access-decision implementation.
//!
//! Everything that asks who may do what asks here, and nothing else decides.
//! A decision is read from the pages upward for each request: the thing's
//! own page, the defaults of the place it lies in, and those of the places
//! above it, up to the system; against who the caller is, which is where
//! they are members, which roles reach them there, the groups they are in
//! and where they are suspended. A change is seen by the next request: the
//! one thing kept between requests, the identities a person holds (their
//! groups, roles and places of page-only viewing among them), is used again
//! only while the database says nothing it was worked out from has changed
//! since (`holders`); what a caller would cache belongs under the [epoch].
//!
//! The pieces, from the caller outward:
//!
//! - [`Caller`] is who asks and with what: a browser session, or an API
//!   token with its [scopes and subtree](tokens). [`Principal`] is the
//!   caller with what is known of them, read once for a request.
//! - [`nodes`] decides about nodes: [`nodes::check`] for one, the listings
//!   for many, [`nodes::effective`] for what a person holds and through
//!   what, and the children and ancestors of a node as the caller may see
//!   them.
//! - [`pages`] reads a page for display and changes it, with the audit
//!   rows and the epoch; [`defaults`] does the same for the defaults page of
//!   a place.
//! - [`giving`] says who may give what: the one rule behind an entry, a
//!   choice, a role, a row of a defaults page and a share with agents, which
//!   a page also tells beforehand; [`covers`] counts what a role held in a
//!   place or a share of it with what is in it reaches.
//! - [`engine`] is the rule behind both, shaped for any thing that has a
//!   page: a thing is described by a [`engine::Site`], and nodes are only
//!   the first kind that is.
//!
//! Every question takes the time it is asked at, because entries and roles
//! end by themselves.

#![forbid(unsafe_code)]

pub mod admins;
pub mod approvals;
pub mod caps;
pub mod chats;
pub mod covers;
pub mod defaults;
pub mod depends;
pub mod documents;
pub mod doors;
pub mod engine;
pub mod environments;
pub mod epoch;
pub mod everything;
pub mod external;
pub mod giving;
pub mod holders;
pub mod missing;
pub mod nodes;
pub mod options;
pub mod orgs;
pub mod ownership;
pub mod pages;
pub mod previews;
pub mod reach;
pub mod requests;
pub mod resources;
pub mod standing;
pub mod suspension;
pub mod things;
pub mod tokens;
pub mod transfer;
pub mod usable;

use orch_types::Id;

pub use engine::{Frame, Site};
pub use holders::Principal;
pub use tokens::{Caller, KeptOut, TokenGrant};

/// What an act is, for the one thing that depends on it: what archive
/// leaves of a permission. Most acts are `Use`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Act {
    /// Any act that is not one of the two below.
    #[default]
    Use,
    /// Deleting the node. Its Manage permission is still held under
    /// archive for this.
    Delete,
    /// Bringing the archived node back. Its Manage and Archive
    /// permissions are still held under archive for this.
    Unarchive,
}

/// What is decided by policy and not by a page, which the caller resolves
/// because settings are not read here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Policy {
    /// Whether the policy `sharing.allow_external` resolves true at the
    /// place of the thing asked about: its owners may then name somebody
    /// from outside the root org themselves. Left as it is, owners may not.
    pub owners_name_outsiders: bool,
    /// Whether the policy `sharing.allow_holders` resolves true at the place
    /// of the thing asked about: somebody who holds a thing without owning it
    /// or managing its page may then share it, up to what they hold. Left as
    /// it is, only its owners and whoever manages its page share; a caller
    /// that has the settings resolves it (true as shipped).
    pub holders_share: bool,
}

/// Why a question could not be answered, or an act cannot go on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] orch_db::DbError),
    /// There is no such node or resource, or it was deleted, or its home was.
    #[error("no such node or resource: {0}")]
    NotFound(Id),
    /// There is no such user.
    #[error("no such user: {0}")]
    NoSuchUser(Id),
    /// Creation needs an owner who is an active member of the place's root org.
    #[error("the place has no active first owner: {place}")]
    NoActiveOwner {
        /// The place where the thing would be made.
        place: Id,
    },
    /// The caller is suspended where the thing is: altogether (`org` is
    /// none), or in that org.
    #[error("the caller is suspended")]
    Suspended {
        /// The org they are suspended in, outermost first.
        org: Option<Id>,
    },
    /// The caller holds what the act needs, and the node is archived: the
    /// act is refused as a mutation under an archived node.
    #[error("the node is archived")]
    Archived {
        /// The archived node: the nearest one on the way up.
        node: Id,
    },
    /// A permission is named that a thing of that kind does not have, as
    /// `node.view` on a provider.
    #[error("{name} is no permission of a {kind}")]
    NoSuchPermission {
        /// The name as it was given.
        name: String,
        /// The kind of thing it was asked of.
        kind: orch_access_contract::permissions::ObjectKind,
    },
    /// Something stored does not read as what it should be.
    #[error("stored data does not read: {0}")]
    Stored(String),
}

impl From<orch_access_contract::roles::UnknownName> for Error {
    fn from(error: orch_access_contract::roles::UnknownName) -> Self {
        Self::Stored(error.to_string())
    }
}
