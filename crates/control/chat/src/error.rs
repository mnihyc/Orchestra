//! What can go wrong with a chat, in the words of the part that answers the request.
//!
//! A refusal says what was refused and nothing of how it is shown to a
//! client: the part that serves requests turns each into the problem a client
//! acts on. A chat that a person may not read is [`Error::NotFound`] for a
//! direct or a group chat, and a missing permission of the node for a node's
//! chat, as the access rules have it.

use orch_access_contract::permissions::Permission;
use orch_types::Id;

/// Why a chat did not allow something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Chats are off for the node (`chat.enabled`).
    Disabled,
    /// The other person of a direct chat is suspended or has left the org.
    Closed,
    /// Direct and group chats are off in the org (`chat.direct`).
    DirectOff,
    /// Somebody named is no member of the org and may not be brought in.
    NotMember,
    /// The group has as many people as it may.
    Full,
}

impl Why {
    /// The word a client is told.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Closed => "closed",
            Self::DirectOff => "direct_off",
            Self::NotMember => "not_member",
            Self::Full => "full",
        }
    }
}

/// Why something done to a chat was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] orch_db::DbError),
    /// The access engine could not answer.
    #[error(transparent)]
    Authz(orch_authz::Error),
    /// A setting could not be read.
    #[error(transparent)]
    Settings(#[from] orch_settings::Error),
    /// There is no such chat, or it is not the caller's to know.
    #[error("there is no such chat")]
    NotFound,
    /// This chat reserves role and group mentions to current managers.
    #[error("role and group mentions require current chat management")]
    MentionRestricted,
    /// Another device made a deliberate read choice after this observation.
    #[error("the chat's read position was changed meanwhile")]
    StaleRead {
        /// The current read revision.
        revision: i64,
    },
    /// The caller is suspended where the chat is: altogether, or in that org.
    #[error("the caller is suspended")]
    Suspended {
        /// The org they are suspended in; none for altogether.
        org: Option<Id>,
    },
    /// The node, or the root org, is archived, and a chat under it is read-only.
    #[error("the node is archived")]
    Archived {
        /// The archived node.
        node: Id,
    },
    /// The caller lacks a permission on the node of a node's chat.
    #[error("{permission} is required on the node of this chat", permission = .permission.as_str())]
    Lacks {
        /// The node.
        node: Id,
        /// What they lack.
        permission: Permission,
    },
    /// The chat does not allow it, for a reason a client is told.
    #[error("the chat does not allow it: {}", .0.word())]
    NotAllowed(Why),
    /// The caller reads the chat as an admin and is not in it.
    #[error("an admin who reads a chat they are not in only reads it")]
    Inspecting,
    /// Somebody from outside the org may not be brought in by the caller.
    #[error("somebody from outside the org may not be brought in by this person")]
    Outsider {
        /// The nearest people who may.
        ask: Vec<Id>,
        /// How many more could.
        ask_more: i64,
    },
    /// The words of the request are not right.
    #[error("{path}: {message}")]
    Invalid {
        /// Where in the request.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// Only the author may do it.
    #[error("only the author does that")]
    NotTheAuthor,
    /// A secret of the node could not be read, so that nothing can be written
    /// that might hold one.
    #[error("the secrets that text must be masked of cannot be read: {0}")]
    Unmasked(String),
    /// What is asked for waits for a part that is not built.
    #[error("{0}")]
    Unsupported(&'static str),
    /// Something that was built or stored does not read as it should.
    #[error("{0}")]
    Internal(String),
    /// One destination of a forward refused, so nothing was written anywhere.
    #[error("a destination of the forward refused: {error}")]
    Destination {
        /// The destination, as it was given.
        to: serde_json::Value,
        /// Why it refused.
        error: Box<Error>,
    },
}

impl From<orch_authz::Error> for Error {
    fn from(error: orch_authz::Error) -> Self {
        match error {
            orch_authz::Error::NotFound(_) => Self::NotFound,
            orch_authz::Error::Suspended { org } => Self::Suspended { org },
            orch_authz::Error::Archived { node } => Self::Archived { node },
            other => Self::Authz(other),
        }
    }
}

impl Error {
    /// A fault of the request at `path`.
    pub fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.into(),
            message: message.into(),
        }
    }
}
