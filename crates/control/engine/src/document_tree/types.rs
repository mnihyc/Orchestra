//! Typed document destinations, actors, complete operation plans and replay receipts.

use chrono::{DateTime, Utc};
use orch_authz::Principal;
use orch_types::Id;
use serde::{Deserialize, Serialize};

/// The existing bound for one complete document management operation, including folders.
pub const ITEMS_MAX: usize = 50;

/// Deployment policy, supplied by the server rather than guessed from a request or database default.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Ordinary soft-delete retention.
    pub retention: std::time::Duration,
    /// The existing content-version window for this producer.
    pub version_window_secs: i64,
}

impl Options {
    /// The engine's existing retention and agent document version policy.
    #[must_use]
    pub fn for_engine(engine: &crate::Engine) -> Self {
        Self {
            retention: engine.config().soft_delete,
            version_window_secs: crate::tools::edit::VERSION_WINDOW_SECS,
        }
    }
}

/// An existing document directory; neither variant is an execution workspace directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Directory {
    /// The virtual docs directory beside a hierarchy node.
    NodeDocs(Id),
    /// A real document container.
    Container(Id),
}

/// An organization target, distinct from hierarchy nodes and document content blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Item {
    /// A text document with its existing history and files.
    Document(Id),
    /// A document-only container.
    Container(Id),
}

impl Item {
    /// The native identity retained when an item moves.
    #[must_use]
    pub const fn id(self) -> Id {
        match self {
            Self::Document(id) | Self::Container(id) => id,
        }
    }
}

/// Authority is the actual caller; an agent's accountable owner never substitutes for its reach.
#[derive(Clone, Copy)]
pub enum Actor<'a> {
    /// A current person or scoped-token principal.
    Person(&'a Principal),
    /// The agent of this live calling thread, reloaded for each operation.
    Agent {
        /// The calling thread.
        thread: Id,
    },
}

/// Organization operations. Only Delete consumes a separately obtained complete deletion plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Make one empty folder in an existing directory.
    CreateFolder,
    /// Copy current material with fresh identities and destination ownership.
    Copy,
    /// Move an item or a complete tree, optionally changing its basename.
    Move,
    /// Rename one text document under its ordinary Edit authority.
    Rename,
    /// Soft-delete the complete selected trees.
    Delete,
    /// Restore the exact selected deletion cohort.
    Restore,
    /// Restore one earlier text-document version.
    RestoreVersion,
}

/// Canonical arguments, independent of the caller's display paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The operation.
    pub command: Command,
    /// Selected identities; empty only for CreateFolder.
    pub items: Vec<Item>,
    /// Existing destination; required for creation, copy and move.
    pub to: Option<Directory>,
    /// One explicit basename for creation or a single source's new name.
    pub name: Option<String>,
    /// Native call_ref or client request identity, scoped to this actor.
    pub request_ref: String,
    /// Earlier version, only for RestoreVersion.
    pub version: Option<i32>,
}

/// One exact current tree member; suitable for a bounded, fully authorized preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Its type and identity.
    pub item: Item,
    /// Its immediate directory.
    pub parent: Directory,
    /// The hierarchy node beside which the whole document tree stands.
    pub node: Id,
    /// Its root organization.
    pub root: Id,
    /// Its stable path component, including .md for text documents.
    pub basename: String,
    /// Human title, independent of a legacy disambiguated basename.
    pub title: String,
    /// Current content or container revision.
    pub revision: i64,
    /// First accountable owner.
    pub owner: Id,
    /// The current soft deletion, when present.
    pub deleted_at: Option<DateTime<Utc>>,
    /// The operation that deleted this member, distinct from an earlier independent deletion.
    pub deletion: Option<Id>,
    /// Current attached-file count; zero for a folder.
    pub files: i64,
    /// Retained version count; zero for a folder.
    pub versions: i64,
}

/// Per-entry authority for a person or the actual calling agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// May open this item or navigate this folder.
    pub read: bool,
    /// May edit this document or create entries inside this folder.
    pub write: bool,
    /// May manage this item; recursive operations still check every member.
    pub manage: bool,
}

/// One typed result in a caller-filtered directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// The stored entry.
    pub entry: Entry,
    /// Its actual current authority.
    pub standing: Standing,
}

/// A containing folder for structural navigation; hidden names stay absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ancestor {
    /// Native container identity.
    pub id: Id,
    /// Its immediate directory.
    pub parent: Directory,
    /// Actual hierarchy anchor.
    pub node: Id,
    /// Root organization.
    pub root: Id,
    /// Exact component only when the caller may read it.
    pub basename: Option<String>,
    /// Folder label only when readable.
    pub title: Option<String>,
    /// Its own current authority.
    pub standing: Standing,
}

/// One batch of scoped candidates and their required, possibly redacted ancestors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRead {
    /// Readable candidate entries.
    pub entries: Vec<Listed>,
    /// Structural ancestry; this never grants parent or sibling access.
    pub ancestors: Vec<Ancestor>,
}

/// The ordinary text header producer leaves content batching to its caller.
#[derive(Debug)]
pub struct TextCreation {
    /// The real text document row.
    pub document: orch_db::queries::documents::DocumentRow,
    /// False for the same stable identity replay; do not apply creation material twice.
    pub created: bool,
}

/// Existing single-document API copy options, bound to the same durable operation receipt.
/// A human title stays separate from the canonical basename; no source permission page is copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextCopy {
    /// The API's stable destination identity.
    pub id: Id,
    /// Optional ordinary human title, including characters sanitized in a path component.
    pub title: Option<String>,
    /// The creator explicitly chose their ordinary private permission page.
    pub private: bool,
}

/// Exact tree and material membership pinned by a deletion preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipStamp {
    /// All affected entries, sorted by identity and without omitted descendants.
    pub entries: Vec<Entry>,
    /// Native authority/audience epoch, including shares, owners, roles and membership changes.
    pub authority_epoch: i64,
    /// Exact relevant stored permission rows/entries, including an audience-preserving grant replacement.
    pub authority_fingerprint: String,
    /// Earliest grant expiry; crossing it requires a new preview even without an epoch write.
    pub authority_until: Option<DateTime<Utc>>,
}

/// Equality with the complete native deletion membership, without its descriptive rows.
/// A digest supplies no authority and must be protected before crossing a restricted transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionDigest {
    /// Every native membership/material/authority field contributes to this value.
    pub fingerprint: orch_types::Hash,
    /// The same earliest authority expiry used by the complete confirmation.
    pub authority_until: Option<DateTime<Utc>>,
}

impl MembershipStamp {
    /// Produce the equality witness of this exact complete native stamp.
    pub fn deletion_digest(&self) -> Result<DeletionDigest, super::Error> {
        let mut digest = orch_types::Hash::hasher();
        digest.update(b"orch.document-tree.deletion-membership.v1\0");
        digest.update(&serde_json::to_vec(self)?);
        Ok(DeletionDigest {
            fingerprint: orch_types::Hash::finish(&digest),
            authority_until: self.authority_until,
        })
    }
}

/// Two factual representations of the same native complete deletion confirmation.
#[derive(Debug, Clone, Copy)]
pub enum DeletionConfirmation<'a> {
    /// The ordinary full preview, with the original exact equality semantics.
    Membership(&'a MembershipStamp),
    /// A transport-protected complete-stamp digest, with no omitted membership tail.
    Digest(&'a DeletionDigest),
}

impl DeletionConfirmation<'_> {
    pub(super) fn matches(
        self,
        actual: &MembershipStamp,
        now: orch_types::Time,
    ) -> Result<bool, super::Error> {
        let until = match self {
            Self::Membership(stamp) => stamp.authority_until,
            Self::Digest(digest) => digest.authority_until,
        };
        if until.is_some_and(|until| until <= now.get()) {
            return Ok(false);
        }
        Ok(match self {
            Self::Membership(expected) => expected == actual,
            Self::Digest(expected) => *expected == actual.deletion_digest()?,
        })
    }
}

/// A complete authorized deletion plan; no confirmation capability is stored by the domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The exact current membership/material stamp.
    pub stamp: MembershipStamp,
    /// When the ordinary retention period would end.
    pub restore_until: DateTime<Utc>,
    /// The number of people gaining ordinary access, when applicable.
    pub gaining: i64,
    /// The number of people losing ordinary access, when applicable.
    pub losing: i64,
}

/// One identity allocated by a copied tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copied {
    /// Original identity.
    pub source: Item,
    /// Fresh destination identity.
    pub destination: Item,
}

/// The durable result of one completed request, committed with its changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The durable operation identity.
    pub operation: Id,
    /// Current affected entries, including fresh copied entries.
    pub entries: Vec<Entry>,
    /// Original-to-fresh map for a copy.
    pub copied: Vec<Copied>,
    /// People gaining ordinary access; identities are not disclosed here.
    pub gaining: i64,
    /// People losing ordinary access.
    pub losing: i64,
    /// Retention deadline for a deletion.
    pub restore_until: Option<DateTime<Utc>>,
}
