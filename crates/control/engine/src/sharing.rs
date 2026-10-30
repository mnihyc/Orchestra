//! Sharing with agents: a copy written in the written form of each thing, or a live share by area held as entries on pages, to read or to write, of a thing, of a place with what is in it or of a place by itself; taking either back.
//!
//! **A live share is entries on pages and nothing else.** The agents of a place are named in the rows of the
//! areas that were shared (`thread.view` and `thread.interact` for conversations, `chat.view` and `chat.interact`
//! for chats, `doc.view`, `doc.edit` and, for a share to manage, `doc.manage` for documents), on the page of the thing, and for a place on its
//! defaults page as well, so that what is made there later is covered. Nothing is kept beside them: the entries
//! are listed, changed and taken back as any entry of a page is, by `access.manage` on the page and with the audit
//! row of a change of a page, and what the agent may do is worked out from them at every call. A share has no row
//! of its own; its identifier is made from the page's owner and the place whose agents are named, and the entries
//! of that pair are the share. **A place is shared in one of two ways, and both can stand for the same pair.** With
//! what is in it (`inside`, as it is unless the request says otherwise), the agents are named on the place's own page
//! (for its chat and the documents beside it) and on its defaults page (for the conversations, chats and documents
//! of what is inside it, what is made later included). By itself (`inside: false`), they are named on its own page
//! alone and nothing inside is reached. They are told apart by the defaults page: an area that has an entry on it is
//! the share with what is in it, an area that has an entry on the place's own page and none there is the share of the
//! place alone, and each has its own identifier.
//!
//! **A copy is a row** (`agent_shares`) and files: the thing in its written form as the person who shares may read
//! it, kept by whatever is attached as [`Parcels`] and written into the thread's workspace at its next turn. With
//! nothing attached a copy cannot be kept and is refused as unsupported, rather than made and lost.
//!
//! Both are all or nothing over the threads or places named. Each thread named gets a line of history and a
//! notice at its next turn; a wider place gives agents what they find when they list.

use crate::copies::Files;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::inputs::{InputError, InputRequest};
use crate::tools::definitions::offered;
use crate::tools::{ToolSettings, definition_of};
use crate::unit::{Finished, Unit};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::pages::{EntryAdd, EntryUpdate, Holder, PageChange, RowChange};
use orch_access_contract::permissions::{
    NodePermission, ObjectKind, Permission, PermissionName, Placement, Profile,
};
use orch_access_contract::roles::PlaceRef;
use orch_agent_contract::events::OrchTool;
use orch_api_contract::share::{
    ShareDocumentsLevel, ShareLevel, ShareParts, ShareTo, ShareWhat, ShareWithAgents,
};
use orch_api_contract::threads::{ContentPart, Delivery, ShareMode};
use orch_authz::pages::{self, ChangeError, Target};
use orch_authz::reach::AgentReach;
use orch_authz::{Act, Caller, Policy, Principal, documents as document_access, epoch, nodes};
use orch_chat::Acting;
use orch_chat::chats::{self as chat_access, Mode};
use orch_db::audit::{Actor, Entry as AuditEntry, Origin, append};
use orch_db::queries::access::{self, NewEntry, PageKey};
use orch_db::queries::documents as document_rows;
use orch_db::queries::hierarchy;
use orch_db::queries::shares::{self as rows, CopyRow, GrantRow, NewCopy, PlaceRow};
use orch_db::queries::threads;
use orch_db::{Conn, Tx};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError};

/// Where in a thread's directory a copy is written: the directory of every copy is this and its identifier.
pub const COPY_ROOT: &str = ".orch/shared";

/// The most threads or places one share names.
const DESTINATIONS_MAX: usize = orch_api_contract::share::SHARE_MAX_DESTINATIONS as usize;

/// The most characters of the words a person adds to a copy that go to the agent.
const COMMENT_MAX_CHARS: usize = orch_api_contract::threads::INPUT_TEXT_MAX_CHARS as usize;

/// The rows a live share is made of, in the order the areas are told.
const SHARE_PERMISSIONS: [Permission; 10] = [
    Permission::ThreadView,
    Permission::ThreadInteract,
    Permission::ChatView,
    Permission::ChatInteract,
    Permission::DocView,
    Permission::DocEdit,
    Permission::DocManage,
    // The export of a place: the rows by which the agents may copy what is shared into their workspace.
    Permission::ThreadExport,
    Permission::ChatExport,
    Permission::DocExport,
];

// ---------------------------------------------------------------- what is shared

/// What is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subject {
    /// A place: an org, a project, a topic or a thread.
    Node(Id),
    /// A chat: a node's, or a direct or group chat.
    Chat(Id),
    /// A document.
    Document(Id),
}

/// What a request named, found: whose page carries the entries of a live share and where the thing lies.
#[derive(Debug, Clone)]
pub struct Found {
    /// What it is.
    pub subject: Subject,
    /// The kind of the page's owner as it is stored: `node`, `document` or `chat`.
    pub kind: &'static str,
    /// The page's owner: for a node's chat, its node.
    pub object: Id,
    /// Its root org.
    pub org: Id,
    /// The node it lies at or beside; none for a direct or a group chat.
    pub node: Option<Id>,
    /// For a place: `org`, `project`, `topic` or `thread`.
    pub place_kind: Option<String>,
    /// What it is called: a title, or the name of the place or chat.
    pub name: String,
    /// Whether it is a direct or a group chat.
    pub people_chat: bool,
}

/// A share as it stands: a live share, which is the entries of one pair of page owner and place, or a copy.
#[derive(Debug, Clone)]
pub struct Record {
    /// Whether current document material is offered for first reading.
    pub read_first: bool,
    /// Its identifier.
    pub id: Id,
    /// What it is of.
    pub subject: Subject,
    /// The thread or the place whose agents it is for.
    pub to: Id,
    /// A copy or a live share.
    pub mode: ShareMode,
    /// For a live share of a place: whether it reaches what lies inside it. False for one thing, for a
    /// copy, and for a place shared by itself.
    pub inside: bool,
    /// The areas.
    pub parts: ShareParts,
    /// For a live share: the entries that carry it.
    pub grants: Vec<GrantRow>,
    /// For a copy: what was made.
    pub copy: Option<CopyRow>,
    /// For a copy with a comment: the input that was queued.
    pub input: Option<Id>,
    /// Who shared it.
    pub by: Option<Id>,
    /// When.
    pub at: DateTime<Utc>,
    /// Whether the request that returned it made it, and did not find it made by a request before.
    pub fresh: bool,
}

/// What a thread's agent may read beyond its own thread, and the copies it was given.
#[derive(Debug, Clone)]
pub struct ThreadShares {
    /// Independent current document policy and stable first-reading paths.
    pub material: crate::material::Material,
    /// How far its topic's policy lets it reach.
    pub policy: AgentReach,
    /// The shares that name it, and the copies.
    pub shares: Vec<Record>,
}

/// The ordinary agent-sharing producer examined in a transaction its caller must roll back.
#[derive(Debug, Clone)]
pub struct ShareTrial {
    /// Current live shares before the trial.
    pub before: Vec<Record>,
    /// Proposed live shares, never committed or announced.
    pub live: Vec<Record>,
    /// Destinations, with whether each is a place covering future threads.
    pub destinations: Vec<(Id, bool)>,
    /// Resolved requested areas.
    pub parts: ShareParts,
    /// Resolved source reach.
    pub inside: bool,
    /// For each copy destination: file count, bytes and whether this same client id already sent it. No parcel is kept.
    pub copies: Vec<(Id, i64, i64, bool)>,
    /// Frozen areas of an already sent copy, rather than a changed request with the same client id.
    pub copy_parts: BTreeMap<Id, ShareParts>,
}

/// Why something could not be shared, or taken back.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    /// There is no such thing, or the caller may not know of it.
    #[error("there is no such thing")]
    NotFound,
    /// The caller lacks what sharing needs, and this is all of it.
    #[error("the caller may not share this")]
    Denied(Box<Refusal>),
    /// The request does not fit.
    #[error("{field}: {message}")]
    Invalid {
        /// Where in the request.
        field: String,
        /// What is wrong.
        message: String,
    },
    /// The agent of a thread cannot be given the tools of the shared tree, or nothing was named that it could read.
    #[error("the tools of the shared tree are not available: {reason}")]
    Unavailable {
        /// `tools_off`, `no_tools` or `nothing_to_read`.
        reason: &'static str,
        /// The thread that cannot.
        thread: Option<Id>,
    },
    /// A copy larger than the limit.
    #[error("the copy is larger than {limit} bytes")]
    TooLarge {
        /// What it came to.
        bytes: i64,
        /// What `sharing.snapshot_max_bytes` allows there.
        limit: i64,
        /// The place the limit was resolved for.
        scope: Option<Id>,
    },
    /// The request is one that a side thread does not take.
    #[error("a side thread takes no share")]
    Side {
        /// Its parent.
        parent: Id,
    },
    /// The thread or a place above it is archived.
    #[error("the place is archived")]
    Archived {
        /// The archived place, the nearest on the way up.
        node: Id,
    },
    /// The caller is suspended where this is.
    #[error("the caller is suspended")]
    Suspended {
        /// The org they are suspended in; none for altogether.
        org: Option<Id>,
    },
    /// The page that carries the share could not be changed as asked.
    #[error("the page could not be changed")]
    Page(Box<ChangeError>),
    /// A copy was asked for and nothing is connected that could keep it.
    #[error("a copy cannot be kept: {0}")]
    Unsupported(&'static str),
    /// The comment that goes with a copy could not be sent.
    #[error("the comment could not be sent")]
    Input(Box<InputError>),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for ShareError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for ShareError {
    fn from(error: orch_authz::Error) -> Self {
        match error {
            orch_authz::Error::NotFound(_) => Self::NotFound,
            orch_authz::Error::Archived { node } => Self::Archived { node },
            orch_authz::Error::Suspended { org } => Self::Suspended { org },
            other => Self::Internal(other.into()),
        }
    }
}

impl From<ChangeError> for ShareError {
    fn from(error: ChangeError) -> Self {
        match error {
            ChangeError::Engine(inner) => inner.into(),
            other => Self::Page(Box::new(other)),
        }
    }
}

impl From<orch_chat::Error> for ShareError {
    fn from(error: orch_chat::Error) -> Self {
        match error {
            orch_chat::Error::NotFound | orch_chat::Error::Lacks { .. } => Self::NotFound,
            orch_chat::Error::Db(inner) => Self::Internal(inner.into()),
            orch_chat::Error::Authz(inner) => inner.into(),
            orch_chat::Error::Settings(inner) => Self::Internal(inner.into()),
            orch_chat::Error::Archived { node } => Self::Archived { node },
            orch_chat::Error::Suspended { org } => Self::Suspended { org },
            other => Self::Internal(EngineError::unreadable(other)),
        }
    }
}

impl From<InputError> for ShareError {
    fn from(error: InputError) -> Self {
        Self::Input(Box::new(error))
    }
}

fn invalid(field: &str, message: impl Into<String>) -> ShareError {
    ShareError::Invalid {
        field: field.to_owned(),
        message: message.into(),
    }
}

// ---------------------------------------------------------------- what keeps a copy

/// Why the files of a copy could not be kept or taken away.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParcelFault(pub String);

/// What keeps the files of a copy until the thread's next turn writes them into its workspace, and takes a
/// delivered copy out of it. The blob store and the workspace are other parts: the engine asks through this and
/// the program that connects them attaches it.
#[async_trait]
pub trait Parcels: Send + Sync {
    /// Keeps the files of the copy `copy`, which is to be written into the directory `path` of the thread. The
    /// answer is the hash of the blob that lists the files, which the row of the copy keeps and pins; none where
    /// the files are kept some other way.
    async fn keep(
        &self,
        conn: &mut Conn,
        copy: Id,
        path: &str,
        files: &Files,
    ) -> Result<Option<Vec<u8>>, ParcelFault>;

    /// Takes a copy that was taken back out of the workspace it was written into, if it was; and lets go of the
    /// files kept for it.
    async fn withdraw(&self, conn: &mut Conn, copy: &CopyRow) -> Result<(), ParcelFault>;

    /// Writes one file under the directory of a thread's workspace, with the writer that delivers the files of a
    /// message: a file that is there with the same bytes is left alone, and one that differs is not replaced.
    async fn write_file(
        &self,
        conn: &mut Conn,
        thread: Id,
        path: &str,
        bytes: &[u8],
    ) -> Result<FileWritten, ParcelFault>;
}

/// How a file written into a workspace stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWritten {
    /// It is there.
    Ready,
    /// It is being written: this many of its bytes have arrived.
    Downloading {
        /// How many.
        received: i64,
    },
    /// A file is there already with other bytes, and none is replaced.
    Differs,
}

impl Engine {
    /// Attaches what keeps the files of copies. What was attached before is replaced.
    pub fn attach_parcels(&self, parcels: Arc<dyn Parcels>) {
        *self
            .inner
            .parcels
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(parcels);
    }

    /// What keeps the files of copies, if it was attached.
    #[must_use]
    pub fn parcels(&self) -> Option<Arc<dyn Parcels>> {
        self.inner
            .parcels
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

// ---------------------------------------------------------------- the rows of a share

/// The permissions the areas of `parts` stand in.
fn permissions_of(parts: &ShareParts) -> Vec<Permission> {
    let mut out = Vec::new();
    match parts.timeline {
        Some(ShareLevel::Read) => out.push(Permission::ThreadView),
        Some(ShareLevel::Write) => out.extend([Permission::ThreadView, Permission::ThreadInteract]),
        None => {}
    }
    match parts.chat {
        Some(ShareLevel::Read) => out.push(Permission::ChatView),
        Some(ShareLevel::Write) => out.extend([Permission::ChatView, Permission::ChatInteract]),
        None => {}
    }
    match parts.documents {
        Some(ShareDocumentsLevel::Read) => out.push(Permission::DocView),
        Some(ShareDocumentsLevel::Edit) => out.extend([Permission::DocView, Permission::DocEdit]),
        Some(ShareDocumentsLevel::Manage) => out.extend([
            Permission::DocView,
            Permission::DocEdit,
            Permission::DocManage,
        ]),
        None => {}
    }
    out
}

/// The areas that rows stand in: a row to write holds the row to read as well.
fn parts_of(permissions: &BTreeSet<String>) -> ShareParts {
    let has = |permission: Permission| permissions.contains(permission.as_str());
    ShareParts {
        timeline: if has(Permission::ThreadInteract) {
            Some(ShareLevel::Write)
        } else if has(Permission::ThreadView) {
            Some(ShareLevel::Read)
        } else {
            None
        },
        chat: if has(Permission::ChatInteract) {
            Some(ShareLevel::Write)
        } else if has(Permission::ChatView) {
            Some(ShareLevel::Read)
        } else {
            None
        },
        documents: if has(Permission::DocManage) {
            Some(ShareDocumentsLevel::Manage)
        } else if has(Permission::DocEdit) {
            Some(ShareDocumentsLevel::Edit)
        } else if has(Permission::DocView) {
            Some(ShareDocumentsLevel::Read)
        } else {
            None
        },
    }
}

/// Which page a row of a share stands on, and which row of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Row {
    /// The page: `live`, or `defaults` for a place's gift to what is made inside it.
    defaults: bool,
    permission: Permission,
    /// On a defaults page, the row of a permission that has two.
    placement: Option<Placement>,
}

impl Ord for Row {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.defaults, self.permission.as_str(), self.profile()).cmp(&(
            other.defaults,
            other.permission.as_str(),
            other.profile(),
        ))
    }
}

impl PartialOrd for Row {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Row {
    fn profile(self) -> &'static str {
        match self.placement {
            None => "any",
            Some(Placement::Managed) => "managed",
            Some(Placement::Connected) => "connected",
        }
    }
}

/// The rows of the pages that carry `parts` of `found`; a place shared by itself (`inside` false) has the rows of
/// its own page alone.
fn rows_of(found: &Found, parts: &ShareParts, inside: bool) -> Vec<Row> {
    rows_in(found, permissions_of(parts), inside)
}

/// The Export rows of the areas of `parts`, on the pages that carry them: the rows that let the agents copy what is
/// shared into their workspace (the export of a place).
fn export_rows_of(found: &Found, parts: &ShareParts, inside: bool) -> Vec<Row> {
    let mut permissions = Vec::new();
    if parts.timeline.is_some() {
        permissions.push(Permission::ThreadExport);
    }
    if parts.chat.is_some() {
        permissions.push(Permission::ChatExport);
    }
    if parts.documents.is_some() {
        permissions.push(Permission::DocExport);
    }
    rows_in(found, permissions, inside)
}

/// The rows of `permissions` on the pages that carry them for `found`.
fn rows_in(found: &Found, permissions: Vec<Permission>, inside: bool) -> Vec<Row> {
    let mut out = Vec::new();
    let place = matches!(
        found.place_kind.as_deref(),
        Some("org" | "project" | "topic")
    );
    let thread = found.place_kind.as_deref() == Some("thread");
    for permission in permissions {
        match found.subject {
            Subject::Node(_) => {
                let own = match permission {
                    Permission::ThreadView
                    | Permission::ThreadInteract
                    | Permission::ThreadExport => thread,
                    _ => true,
                };
                if own {
                    out.push(Row {
                        defaults: false,
                        permission,
                        placement: None,
                    });
                }
                if place && inside {
                    if permission.profiles() {
                        for placement in [Placement::Managed, Placement::Connected] {
                            out.push(Row {
                                defaults: true,
                                permission,
                                placement: Some(placement),
                            });
                        }
                    } else {
                        out.push(Row {
                            defaults: true,
                            permission,
                            placement: None,
                        });
                    }
                }
            }
            Subject::Chat(_) | Subject::Document(_) => out.push(Row {
                defaults: false,
                permission,
                placement: None,
            }),
        }
    }
    out
}

/// The identifier of a live share: the page's owner and the place whose agents are named.
#[must_use]
pub fn live_id(kind: &str, object: Id, holder: Id) -> Id {
    ids::derived(
        "agent-share.live",
        &[
            kind.as_bytes(),
            object.as_bytes().as_slice(),
            holder.as_bytes().as_slice(),
        ],
    )
}

/// The identifier of the live share of a place by itself: the same pair as [`live_id`], told from the share of the
/// place with what is in it, which can stand beside it.
#[must_use]
pub fn live_id_alone(kind: &str, object: Id, holder: Id) -> Id {
    ids::derived(
        "agent-share.live.alone",
        &[
            kind.as_bytes(),
            object.as_bytes().as_slice(),
            holder.as_bytes().as_slice(),
        ],
    )
}

/// The identifier of a copy made by a request for one thread.
#[must_use]
pub fn copy_id(client_id: &str, thread: Id, kind: &str, object: Id) -> Id {
    ids::derived(
        "agent-share.copy",
        &[
            client_id.as_bytes(),
            thread.as_bytes().as_slice(),
            kind.as_bytes(),
            object.as_bytes().as_slice(),
        ],
    )
}

fn subject_of(kind: &str, object: Id) -> Option<Subject> {
    match kind {
        "node" => Some(Subject::Node(object)),
        "chat" => Some(Subject::Chat(object)),
        "document" => Some(Subject::Document(object)),
        _ => None,
    }
}

/// The live shares that a set of entries makes: one for each pair of page owner and place, and for the pair of a
/// place two when it is shared in both ways: the share with what is in it is the areas that have an entry on the
/// place's defaults page, the share of the place by itself the areas that have an entry on its own page only.
fn records_of(grants: Vec<GrantRow>) -> Vec<Record> {
    let mut groups: BTreeMap<(String, Id, Id), Vec<GrantRow>> = BTreeMap::new();
    for grant in grants {
        groups
            .entry((grant.object_kind.clone(), grant.object_id, grant.holder_id))
            .or_default()
            .push(grant);
    }
    let mut out = Vec::new();
    for ((kind, object, holder), grants) in groups {
        let Some(subject) = subject_of(&kind, object) else {
            continue;
        };
        let place = grants.iter().any(|grant| {
            matches!(
                grant.node_kind.as_deref(),
                Some("org" | "project" | "topic")
            )
        });
        let reaching: BTreeSet<String> = grants
            .iter()
            .filter(|grant| grant.page == "defaults" && grant.share_inside.is_none())
            .map(|grant| grant.permission.clone())
            .collect();
        let (inside, alone): (Vec<GrantRow>, Vec<GrantRow>) = if place {
            grants.into_iter().partition(|grant| {
                grant
                    .share_inside
                    .unwrap_or_else(|| reaching.contains(&grant.permission))
            })
        } else {
            (Vec::new(), grants)
        };
        for (reaches, grants) in [(true, inside), (false, alone)] {
            if grants.is_empty() {
                continue;
            }
            let permissions: BTreeSet<String> = grants
                .iter()
                .map(|grant| grant.permission.clone())
                .collect();
            let first = grants
                .iter()
                .min_by_key(|grant| (grant.created_at, grant.entry_id));
            out.push(Record {
                read_first: grants
                    .iter()
                    .any(|grant| grant.permission == "doc.view" && grant.read_first),
                id: if place && !reaches {
                    live_id_alone(&kind, object, holder)
                } else {
                    live_id(&kind, object, holder)
                },
                subject,
                to: holder,
                mode: ShareMode::Live,
                inside: reaches,
                parts: parts_of(&permissions),
                by: first.and_then(|grant| grant.created_by),
                at: first.map_or_else(Utc::now, |grant| grant.created_at),
                grants,
                copy: None,
                input: None,
                fresh: true,
            });
        }
    }
    out.sort_by_key(|record| (record.at, record.id));
    out
}

fn copy_record(row: CopyRow) -> Option<Record> {
    let subject = subject_of(&row.object_kind, row.object_id)?;
    let has = |word: &str| row.include.iter().any(|each| each == word);
    let parts = match subject {
        Subject::Node(_) => ShareParts {
            timeline: has("timeline").then_some(ShareLevel::Read),
            chat: has("chat").then_some(ShareLevel::Read),
            documents: has("documents").then_some(ShareDocumentsLevel::Read),
        },
        Subject::Chat(_) => ShareParts {
            chat: Some(ShareLevel::Read),
            ..ShareParts::default()
        },
        Subject::Document(_) => ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        },
    };
    Some(Record {
        read_first: row.read_first,
        id: row.id,
        subject,
        to: row.thread_id,
        mode: ShareMode::Snapshot,
        inside: false,
        parts,
        grants: Vec::new(),
        by: Some(row.created_by),
        at: row.created_at,
        input: None,
        copy: Some(row),
        fresh: true,
    })
}

/// The words of the areas a copy holds, as the table stores them.
fn include_of(parts: &ShareParts) -> Vec<String> {
    let mut out = Vec::new();
    if parts.timeline.is_some() {
        out.push("timeline".to_owned());
    }
    if parts.chat.is_some() {
        out.push("chat".to_owned());
    }
    if parts.documents.is_some() {
        out.push("documents".to_owned());
    }
    out
}

/// The client value of the input that carries the comment of a copy.
fn comment_client(copy: Id) -> String {
    format!("share-comment:{copy}")
}

// ---------------------------------------------------------------- finding what a request names

impl Engine {
    /// The thing a request names; nothing is told of what is there but cannot be seen.
    async fn find_subject(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        origin: &Origin,
        now: DateTime<Utc>,
        subject: Subject,
    ) -> Result<(Found, Option<chat_access::Loaded>), ShareError> {
        match subject {
            Subject::Node(id) => {
                let place = rows::places(conn, &[id])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or(ShareError::NotFound)?;
                self.may_see_node(conn, principal, id).await?;
                Ok((node_found(&place), None))
            }
            Subject::Document(id) => {
                let row = document_rows::get(conn, id)
                    .await?
                    .filter(|row| row.deleted_at.is_none())
                    .ok_or(ShareError::NotFound)?;
                match document_access::check(
                    conn,
                    principal,
                    id,
                    Permission::DocView,
                    Policy::default(),
                )
                .await?
                {
                    Decision::Allowed { .. } => {}
                    Decision::Refused(_) => return Err(ShareError::NotFound),
                }
                Ok((
                    Found {
                        subject,
                        kind: "document",
                        object: id,
                        org: row.org_id,
                        node: Some(row.node_id),
                        place_kind: None,
                        name: row.title,
                        people_chat: false,
                    },
                    None,
                ))
            }
            Subject::Chat(id) => {
                let acting = Acting {
                    principal,
                    now,
                    origin,
                };
                let loaded = chat_access::load(conn, &acting, id, Mode::Read).await?;
                let row = &loaded.row;
                let people = row.is_people_chat();
                let name = match (&row.name, row.node_id) {
                    (Some(name), _) => name.clone(),
                    (None, Some(node)) => threads::node_name(conn, node)
                        .await?
                        .unwrap_or_else(|| "Chat".to_owned()),
                    (None, None) => rows::member_names(conn, &[id])
                        .await?
                        .remove(&id)
                        .map_or_else(|| "Chat".to_owned(), |names| names.join(", ")),
                };
                let found = Found {
                    subject,
                    kind: if people { "chat" } else { "node" },
                    object: row.node_id.unwrap_or(row.id),
                    org: row.org_id,
                    node: row.node_id,
                    place_kind: None,
                    name,
                    people_chat: people,
                };
                Ok((found, Some(loaded)))
            }
        }
    }

    /// Whether the caller may see a place at all: it is not there for them otherwise.
    async fn may_see_node(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        node: Id,
    ) -> Result<(), ShareError> {
        match nodes::check_named(
            conn,
            principal,
            node,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(Decision::Allowed { .. }) => Ok(()),
            Ok(Decision::Refused(_)) | Err(orch_authz::Error::Archived { .. }) => {
                Err(ShareError::NotFound)
            }
            Err(other) => Err(other.into()),
        }
    }
}

fn node_found(place: &PlaceRow) -> Found {
    Found {
        subject: Subject::Node(place.id),
        kind: "node",
        object: place.id,
        org: place.org_id,
        node: Some(place.id),
        place_kind: Some(place.kind.clone()),
        name: place.name.clone(),
        people_chat: false,
    }
}

/// Whether a share of `found` is of a place by itself: the place is an org, a project or a topic, and the request
/// says it is not shared with what is in it.
fn place_alone(found: &Found, inside: bool) -> bool {
    !inside
        && matches!(found.subject, Subject::Node(_))
        && matches!(
            found.place_kind.as_deref(),
            Some("org" | "project" | "topic")
        )
}

/// The areas a request names for a thing, with what is left out of the request taken as the share's default. A
/// place shared by itself has no conversation of its own: that area is refused, and what is left out is its chat
/// and its documents, to read.
fn parts_asked(
    found: &Found,
    asked: Option<ShareParts>,
    mode: ShareMode,
    inside: bool,
) -> Result<ShareParts, ShareError> {
    let alone = place_alone(found, inside) && mode == ShareMode::Live;
    if alone && asked.is_some_and(|parts| parts.timeline.is_some()) {
        return Err(invalid(
            "parts.timeline",
            "a place has no conversation of its own: share it with what is in it, or leave the conversations out",
        ));
    }
    let parts = match (found.subject, asked) {
        (Subject::Node(_), None) if alone => ShareParts {
            chat: Some(ShareLevel::Read),
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        },
        (Subject::Node(_), None) => ShareParts {
            timeline: Some(ShareLevel::Read),
            ..ShareParts::default()
        },
        (Subject::Node(_), Some(parts)) => parts,
        (Subject::Chat(_), asked) => ShareParts {
            chat: Some(
                asked
                    .and_then(|parts| parts.chat)
                    .unwrap_or(ShareLevel::Read),
            ),
            ..ShareParts::default()
        },
        (Subject::Document(_), asked) => ShareParts {
            documents: Some(
                asked
                    .and_then(|parts| parts.documents)
                    .unwrap_or(ShareDocumentsLevel::Read),
            ),
            ..ShareParts::default()
        },
    };
    if parts == ShareParts::default() {
        return Err(ShareError::Unavailable {
            reason: "nothing_to_read",
            thread: None,
        });
    }
    if mode == ShareMode::Snapshot
        && (parts.timeline == Some(ShareLevel::Write)
            || parts.chat == Some(ShareLevel::Write)
            || matches!(
                parts.documents,
                Some(ShareDocumentsLevel::Edit | ShareDocumentsLevel::Manage)
            ))
    {
        return Err(invalid(
            "parts",
            "a copy holds no write and no manage: it is files that are written once",
        ));
    }
    Ok(parts)
}

/// A thread or a place whose agents a share is for.
#[derive(Debug, Clone)]
struct Destination {
    id: Id,
    place: PlaceRow,
}

impl Destination {
    fn is_thread(&self) -> bool {
        self.place.kind == "thread"
    }
}

impl Engine {
    /// The threads or places a request names, each checked: it is there, a side thread takes no share, and the
    /// caller may change what that agent will say (`thread.interact` on the thread, `node.interact` on a place).
    async fn destinations(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        to: &ShareTo,
        mode: ShareMode,
    ) -> Result<Vec<Destination>, ShareError> {
        let (ids, nodes_named): (Vec<Id>, bool) = match to {
            ShareTo::Threads(threads) => {
                (threads.iter().map(|each| each.thread_id).collect(), false)
            }
            ShareTo::Nodes(places) => (places.iter().map(|each| each.node_id).collect(), true),
        };
        if ids.is_empty() || ids.len() > DESTINATIONS_MAX {
            return Err(invalid(
                "to",
                format!("name 1 to {DESTINATIONS_MAX} threads or places"),
            ));
        }
        if nodes_named && mode == ShareMode::Snapshot {
            return Err(invalid("to", "a copy goes to threads"));
        }
        let mut unique: Vec<Id> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        let found = rows::places(conn, &unique).await?;
        let mut out = Vec::new();
        for id in unique {
            let place = found
                .iter()
                .find(|place| place.id == id)
                .cloned()
                .ok_or(ShareError::NotFound)?;
            if !nodes_named && place.kind != "thread" {
                return Err(invalid("to", format!("{id} is not a thread")));
            }
            if let Some(parent) = place.side_of {
                return Err(ShareError::Side { parent });
            }
            match nodes::check_named(
                conn,
                principal,
                id,
                PermissionName::OfNode(NodePermission::Interact),
                Act::Use,
                Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => {}
                Decision::Refused(refusal) => return Err(ShareError::Denied(Box::new(refusal))),
            }
            out.push(Destination { id, place });
        }
        Ok(out)
    }

    /// Why a thread cannot be given the tools of the shared tree, if it cannot.
    async fn tools_unavailable(
        &self,
        conn: &mut Conn,
        thread: Id,
    ) -> Result<Option<&'static str>, ShareError> {
        let Some(row) = threads::thread(conn, thread).await? else {
            return Err(ShareError::NotFound);
        };
        let settings = ToolSettings::load(conn, row.node_id).await?;
        if !offered(&settings.enabled, row.side_of.is_some()).contains(&OrchTool::ListShared) {
            return Ok(Some("tools_off"));
        }
        if rows::harness_takes_tools(conn, thread).await? == Some(false) {
            return Ok(Some("no_tools"));
        }
        Ok(None)
    }

    /// The two policies of sharing at a place: whether `sharing.allow_external` lets the owners of a place name
    /// somebody from outside its root org, and whether `sharing.allow_holders` lets somebody who holds what they
    /// share without owning it or managing its page share it. Both are true as shipped.
    async fn policy_at(&self, conn: &mut Conn, node: Id) -> Result<Policy, ShareError> {
        let allowed =
            orch_settings::get::<bool>(conn, Place::Node(node), keys::SHARING_ALLOW_EXTERNAL)
                .await
                .map_err(EngineError::from)?;
        let holders =
            orch_settings::get::<bool>(conn, Place::Node(node), keys::SHARING_ALLOW_HOLDERS)
                .await
                .map_err(EngineError::from)?;
        Ok(Policy {
            owners_name_outsiders: allowed.unwrap_or(true),
            holders_share: holders.unwrap_or(true),
        })
    }
}

// ---------------------------------------------------------------- sharing

impl Engine {
    /// Shares a thing with the agents of threads or places, as a copy or as a live share. All or nothing over
    /// the threads or places named; a repeat with the same `client_id` finds what it made.
    pub async fn share_with_agents(
        &self,
        caller: &Caller,
        origin: &Origin,
        request: &ShareWithAgents,
    ) -> Result<Vec<Record>, ShareError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        self.share_with_agents_guarded(tx, principal, origin, request)
            .await
    }

    /// Previews an agent share using the ordinary source/destination checks and live producer. The caller rolls back;
    /// this never commits, keeps a parcel, finishes a unit, wakes a thread or publishes a marker.
    pub async fn preview_agent_share_in(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        origin: &Origin,
        request: &ShareWithAgents,
    ) -> Result<ShareTrial, ShareError> {
        if request.client_id.trim().is_empty() {
            return Err(invalid(
                "client_id",
                "is the value that makes asking again safe",
            ));
        }
        let now = principal.now().get();
        let asked = match &request.what {
            ShareWhat::Node(node) => Subject::Node(node.node_id),
            ShareWhat::Chat(chat) => Subject::Chat(chat.chat_id),
            ShareWhat::Document(doc) => Subject::Document(doc.document_id),
        };
        let (found, loaded) = self.find_subject(tx, principal, origin, now, asked).await?;
        let inside = request.inside.unwrap_or(true);
        let parts = parts_asked(&found, request.parts, request.mode, inside)?;
        let destinations = self
            .destinations(tx, principal, &request.to, request.mode)
            .await?;
        if request.expires_at.is_some_and(|end| end.get() <= now) {
            return Err(invalid("expires_at", "is already past"));
        }
        if request.read_first == Some(true) && parts.documents.is_none() {
            return Err(invalid("read_first", "requires included document material"));
        }
        let before = self.live_records(tx, &found, now).await?;
        let mut copies = Vec::new();
        let mut copy_parts = BTreeMap::new();
        let mut live = match request.mode {
            ShareMode::Live => {
                if found.people_chat && loaded.as_ref().is_some_and(|chat| !chat.standing.in_chat) {
                    return Err(ShareError::NotFound);
                }
                for destination in destinations.iter().filter(|each| each.is_thread()) {
                    if let Some(reason) = self.tools_unavailable(tx, destination.id).await? {
                        return Err(ShareError::Unavailable {
                            reason,
                            thread: Some(destination.id),
                        });
                    }
                }
                self.create_live(
                    tx,
                    principal,
                    origin,
                    now,
                    &found,
                    &parts,
                    inside,
                    &destinations,
                    request.expires_at,
                    request.download.unwrap_or(true),
                )
                .await?
            }
            ShareMode::Snapshot => {
                if request
                    .comment
                    .as_ref()
                    .is_some_and(|text| text.chars().count() > COMMENT_MAX_CHARS)
                {
                    return Err(invalid("comment", "is longer than an input may be"));
                }
                if self.parcels().is_none() {
                    return Err(ShareError::Unsupported(
                        "nothing is connected that keeps the files of a copy until they are written into a workspace",
                    ));
                }
                let mut missing = Vec::new();
                for destination in &destinations {
                    let id = copy_id(&request.client_id, destination.id, found.kind, found.object);
                    if let Some(existing) = rows::copy(tx, id).await? {
                        copies.push((
                            destination.id,
                            i64::from(existing.file_count),
                            existing.total_bytes,
                            true,
                        ));
                        if let Some(record) = copy_record(existing) {
                            copy_parts.insert(destination.id, record.parts);
                        }
                    } else {
                        missing.push(destination);
                    }
                }
                if missing.is_empty() {
                    return Ok(ShareTrial {
                        before,
                        live: Vec::new(),
                        destinations: destinations
                            .iter()
                            .map(|to| (to.id, !to.is_thread()))
                            .collect(),
                        parts,
                        inside: false,
                        copies,
                        copy_parts,
                    });
                }
                let files = self
                    .files_of_copy(tx, principal, &found, &parts, principal.now())
                    .await?;
                let (limit, scope) = self.copy_limit(tx, &found).await?;
                let mut deliveries = Vec::with_capacity(missing.len());
                for destination in &missing {
                    let theirs = self.copy_for_reader(tx, destination.id, &files).await?;
                    if theirs.bytes() > limit {
                        return Err(ShareError::TooLarge {
                            bytes: theirs.bytes(),
                            limit,
                            scope,
                        });
                    }
                    deliveries.push((
                        destination.id,
                        i64::try_from(theirs.files.len()).unwrap_or(i64::MAX),
                        theirs.bytes(),
                    ));
                }
                // A comment must pass the same input gate as sending the real copy. No input survives the trial.
                if let Some(comment) = request
                    .comment
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
                {
                    let mut unit = Unit::new(tx, now);
                    for destination in &missing {
                        let id =
                            copy_id(&request.client_id, destination.id, found.kind, found.object);
                        self.submit_input_in(
                            &mut unit,
                            principal.caller(),
                            destination.id,
                            &InputRequest {
                                plan: false,
                                content: vec![ContentPart::Text {
                                    text: comment.to_owned(),
                                }],
                                delivery: Delivery::Queue,
                                client_id: comment_client(id),
                                bypass: None,
                            },
                        )
                        .await?;
                    }
                }
                for (destination, count, bytes) in deliveries {
                    copies.push((destination, count, bytes, false));
                }
                Vec::new()
            }
        };
        if let Some(read_first) = request.read_first {
            for record in &mut live {
                let ids: Vec<_> = record.grants.iter().map(|grant| grant.entry_id).collect();
                rows::read_first(tx, &ids, read_first).await?;
                record.read_first = read_first;
                for grant in &mut record.grants {
                    if grant.permission == "doc.view" && grant.read_first != read_first {
                        grant.read_first = read_first;
                        grant.revision += 1;
                    }
                }
            }
        }
        let source_inside = live.first().is_some_and(|record| record.inside);
        Ok(ShareTrial {
            before,
            live,
            destinations: destinations
                .iter()
                .map(|to| (to.id, !to.is_thread()))
                .collect(),
            parts,
            inside: source_inside,
            copies,
            copy_parts,
        })
    }

    /// Shares as [`Self::share_with_agents`] does, in a transaction begun by whoever asked the caller's
    /// credential and authority again once the request had come (the API's action guard): `principal` was
    /// loaded in `tx` by that check, its caller and its time are the ones the share is made with, so a session
    /// that ended or a token taken back while the body came makes nothing.
    #[allow(
        clippy::too_many_lines,
        reason = "one request, checked in the order the caller would be told: the thing, the places, the areas, then the mode"
    )]
    pub async fn share_with_agents_guarded(
        &self,
        mut tx: Tx,
        principal: Principal,
        origin: &Origin,
        request: &ShareWithAgents,
    ) -> Result<Vec<Record>, ShareError> {
        if request.client_id.trim().is_empty() {
            return Err(invalid(
                "client_id",
                "is the value that makes asking again safe",
            ));
        }
        let caller = principal.caller().clone();
        let caller = &caller;
        let now = principal.now().get();
        let asked = match &request.what {
            ShareWhat::Node(node) => Subject::Node(node.node_id),
            ShareWhat::Chat(chat) => Subject::Chat(chat.chat_id),
            ShareWhat::Document(document) => Subject::Document(document.document_id),
        };
        let (found, loaded) = self
            .find_subject(&mut tx, &principal, origin, now, asked)
            .await?;
        let inside = request.inside.unwrap_or(true);
        let parts = parts_asked(&found, request.parts, request.mode, inside)?;
        let destinations = self
            .destinations(&mut tx, &principal, &request.to, request.mode)
            .await?;
        if let Some(end) = request.expires_at
            && end.get() <= now
        {
            return Err(invalid("expires_at", "is already past"));
        }
        if request.read_first == Some(true) && parts.documents.is_none() {
            return Err(invalid("read_first", "requires included document material"));
        }
        let mut records = match request.mode {
            ShareMode::Live => {
                if let Some(chat) = &loaded
                    && found.people_chat
                    && !chat.standing.in_chat
                {
                    return Err(ShareError::NotFound);
                }
                for destination in destinations.iter().filter(|each| each.is_thread()) {
                    if let Some(reason) = self.tools_unavailable(&mut tx, destination.id).await? {
                        return Err(ShareError::Unavailable {
                            reason,
                            thread: Some(destination.id),
                        });
                    }
                }
                self.create_live(
                    &mut tx,
                    &principal,
                    origin,
                    now,
                    &found,
                    &parts,
                    inside,
                    &destinations,
                    request.expires_at,
                    request.download.unwrap_or(true),
                )
                .await?
            }
            ShareMode::Snapshot => {
                self.create_copies(
                    &mut tx,
                    &principal,
                    caller,
                    now,
                    &found,
                    &parts,
                    &destinations,
                    request,
                )
                .await?
            }
        };
        if let Some(read_first) = request.read_first {
            for record in &mut records {
                if record.mode != ShareMode::Live {
                    continue;
                }
                let ids: Vec<Id> = record.grants.iter().map(|entry| entry.entry_id).collect();
                rows::read_first(&mut tx, &ids, read_first).await?;
                record.read_first = read_first;
                for grant in &mut record.grants {
                    if grant.permission == "doc.view" && grant.read_first != read_first {
                        grant.read_first = read_first;
                        grant.revision += 1;
                        record.fresh = true;
                    }
                }
            }
        }
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            for record in records.iter().filter(|record| record.fresh) {
                if let Some(destination) = destinations
                    .iter()
                    .find(|each| each.id == record.to && each.is_thread())
                {
                    self.tell_thread(&mut unit, destination.id, record, &found, caller.user())
                        .await?;
                }
            }
            unit.finish().await.map_err(ShareError::from)?
        };
        self.commit(tx, finished).await?;
        Ok(records)
    }

    /// Names the agents of the destinations in the rows of the areas, on the pages that carry them.
    #[allow(
        clippy::too_many_arguments,
        reason = "who shares what, with whom, until when, in which transaction and for which audit row"
    )]
    async fn create_live(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        origin: &Origin,
        now: DateTime<Utc>,
        found: &Found,
        parts: &ShareParts,
        inside: bool,
        destinations: &[Destination],
        expires_at: Option<Time>,
        download: bool,
    ) -> Result<Vec<Record>, ShareError> {
        let mut wanted = rows_of(found, parts, inside);
        let share_inside = (matches!(found.subject, Subject::Node(_))
            && matches!(
                found.place_kind.as_deref(),
                Some("org" | "project" | "topic")
            ))
        .then_some(inside);
        let holders: Vec<Id> = destinations.iter().map(|each| each.id).collect();
        let before = self.live_records(tx, found, now).await?;
        if !found.people_chat {
            // Nobody gives more than they hold: the whole share is asked what it lacks before anything is
            // written, so that the refusal names everything and nothing is half made.
            wanted = self
                .rows_the_sharer_may_give(tx, principal, found, wanted)
                .await?;
        }
        // The export of a place: the agents may also copy what is shared where the sharer holds the Export row
        // of the area and leaves "They may download it" ticked. What they do not hold is left out, not refused:
        // the agents then read it and do not copy it.
        if download {
            let export = export_rows_of(found, parts, inside);
            let export = if found.people_chat {
                export
            } else {
                self.export_rows_the_sharer_may_give(tx, principal, found, export)
                    .await?
            };
            wanted.extend(export);
        }
        if found.people_chat {
            for holder in &holders {
                self.name_agents_in_chat(tx, principal, found, *holder, &wanted, expires_at, now)
                    .await?;
            }
        } else {
            let mut delta = Delta::default();
            for holder in &holders {
                for row in &wanted {
                    delta.add(*row, *holder, expires_at);
                }
            }
            self.apply_delta(tx, principal, origin, found, &delta, share_inside)
                .await?;
        }
        let mut shares = self.live_records(tx, found, now).await?;
        // A place shared both ways has two shares for the same place: the one that was asked for is the answer.
        let is_place = matches!(found.subject, Subject::Node(_))
            && matches!(
                found.place_kind.as_deref(),
                Some("org" | "project" | "topic")
            );
        // The exact requested reach is independently retained even when another reach covers its areas.
        let mut answer: Vec<Record> = Vec::new();
        for holder in &holders {
            let of_the_holder: Vec<&Record> = shares
                .iter()
                .filter(|record| record.to == *holder)
                .collect();
            if let Some(record) = of_the_holder
                .iter()
                .find(|record| !is_place || record.inside == inside)
            {
                answer.push((*record).clone());
            }
        }
        shares = answer;
        for record in &mut shares {
            record.fresh = before
                .iter()
                .find(|earlier| earlier.to == record.to && earlier.inside == record.inside)
                .is_none_or(|earlier| earlier.parts != record.parts);
        }
        Ok(shares)
    }

    /// Asks the pages that a share is written on whether its sharer may give every row of it, and refuses with
    /// all that they lack: holding what the agents are given, in each area, on the page of the thing and, for the
    /// conversations, chats and documents inside a place, on a thing that follows its defaults; and, if they neither
    /// own the thing nor manage its page, that the place lets holders share. The rows to write are those asked for,
    /// except that a row of a place's defaults is given in the forms of environment the sharer holds it in.
    async fn rows_the_sharer_may_give(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        found: &Found,
        wanted: Vec<Row>,
    ) -> Result<Vec<Row>, ShareError> {
        let policy = match found.node {
            Some(node) => self.policy_at(tx, node).await?,
            None => Policy::default(),
        };
        let mut refusal: Option<Box<Refusal>> = None;
        let mut kept: Vec<Row> = Vec::new();
        for defaults in [false, true] {
            let rows: Vec<(Permission, Profile)> = wanted
                .iter()
                .filter(|row| row.defaults == defaults)
                .map(|row| {
                    (
                        row.permission,
                        match row.placement {
                            None => Profile::Any,
                            Some(Placement::Managed) => Profile::Managed,
                            Some(Placement::Connected) => Profile::Connected,
                        },
                    )
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            let target = match (found.subject, defaults) {
                (Subject::Document(id), _) => Target::Resource(ObjectRef {
                    kind: ObjectKind::Document,
                    id,
                }),
                (_, true) => Target::Defaults(PlaceRef::Node(found.object)),
                (_, false) => Target::Live(found.object),
            };
            match pages::judge_share(tx, principal, target, &rows, policy).await? {
                pages::ShareJudgement::Allowed(allowed) => {
                    kept.extend(wanted.iter().copied().filter(|row| {
                        row.defaults == defaults
                            && allowed.contains(&(
                                row.permission,
                                match row.placement {
                                    None => Profile::Any,
                                    Some(Placement::Managed) => Profile::Managed,
                                    Some(Placement::Connected) => Profile::Connected,
                                },
                            ))
                    }));
                }
                pages::ShareJudgement::Refused(more) => {
                    refusal = Some(match refusal.take() {
                        None => more,
                        Some(first) => {
                            let mut missing = first.missing;
                            missing.extend(more.missing);
                            Box::new(Refusal::of(first.required, first.object, missing))
                        }
                    });
                }
            }
        }
        match refusal {
            Some(refusal) => Err(ShareError::Denied(refusal)),
            None => Ok(kept),
        }
    }

    /// Of the Export rows of a share, those its sharer may give: the others are left out and no refusal is made, for
    /// a sharer who may not take a thing away as a file gives agents its reading and no copy of it.
    async fn export_rows_the_sharer_may_give(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        found: &Found,
        wanted: Vec<Row>,
    ) -> Result<Vec<Row>, ShareError> {
        let mut groups: Vec<(bool, Permission)> = Vec::new();
        for row in &wanted {
            if !groups.contains(&(row.defaults, row.permission)) {
                groups.push((row.defaults, row.permission));
            }
        }
        let mut kept: Vec<Row> = Vec::new();
        for (defaults, permission) in groups {
            let group: Vec<Row> = wanted
                .iter()
                .copied()
                .filter(|row| row.defaults == defaults && row.permission == permission)
                .collect();
            match self
                .rows_the_sharer_may_give(tx, principal, found, group)
                .await
            {
                Ok(rows) => kept.extend(rows),
                Err(ShareError::Denied(_)) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(kept)
    }

    /// The live shares that stand on the page(s) of `found`, as they count at `now`.
    async fn live_records(
        &self,
        conn: &mut Conn,
        found: &Found,
        now: DateTime<Utc>,
    ) -> Result<Vec<Record>, ShareError> {
        let permissions: Vec<String> = SHARE_PERMISSIONS
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect();
        let grants = rows::grants_on(conn, found.kind, found.object, &permissions, now).await?;
        let mut records = records_of(grants);
        if matches!(found.subject, Subject::Chat(_)) && !found.people_chat {
            // A node's chat is the node's own chat rows on its own page; the rows for what lies inside are another matter.
            for record in &mut records {
                record.grants.retain(|grant| {
                    grant.page == "live"
                        && matches!(
                            grant.permission.as_str(),
                            "chat.view" | "chat.interact" | "chat.export"
                        )
                });
                let permissions: BTreeSet<String> = record
                    .grants
                    .iter()
                    .map(|grant| grant.permission.clone())
                    .collect();
                record.parts = parts_of(&permissions);
            }
            records.retain(|record| !record.grants.is_empty());
        }
        for record in &mut records {
            record.subject = found.subject;
        }
        Ok(records)
    }
}

/// A change of the entries of a share: what is added, changed and taken out, row by row.
#[derive(Debug, Default)]
struct Delta {
    add: BTreeMap<Row, Vec<(Id, Option<Time>)>>,
    update: BTreeMap<Row, Vec<(Id, Option<Time>)>>,
    remove: BTreeMap<Row, Vec<Id>>,
}

impl Delta {
    fn add(&mut self, row: Row, holder: Id, expires_at: Option<Time>) {
        self.add.entry(row).or_default().push((holder, expires_at));
    }

    fn update(&mut self, row: Row, entry: Id, expires_at: Option<Time>) {
        self.update
            .entry(row)
            .or_default()
            .push((entry, expires_at));
    }

    fn remove(&mut self, row: Row, entry: Id) {
        self.remove.entry(row).or_default().push(entry);
    }

    fn is_empty(&self) -> bool {
        self.add.is_empty() && self.update.is_empty() && self.remove.is_empty()
    }

    fn rows(&self) -> BTreeSet<Row> {
        self.add
            .keys()
            .chain(self.update.keys())
            .chain(self.remove.keys())
            .copied()
            .collect()
    }
}

impl Engine {
    /// Applies a change of entries to the pages that carry them, one change for each page, as the caller:
    /// whoever may not change a page is refused as for any change of it.
    async fn apply_delta(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        origin: &Origin,
        found: &Found,
        delta: &Delta,
        share_inside: Option<bool>,
    ) -> Result<(), ShareError> {
        if delta.is_empty() {
            return Ok(());
        }
        let policy = match found.node {
            Some(node) => self.policy_at(tx, node).await?,
            None => Policy::default(),
        };
        for defaults in [false, true] {
            let rows_here: Vec<Row> = delta
                .rows()
                .into_iter()
                .filter(|row| row.defaults == defaults)
                .collect();
            let target = match (found.subject, defaults) {
                (Subject::Document(id), _) => Target::Resource(ObjectRef {
                    kind: ObjectKind::Document,
                    id,
                }),
                (_, true) => Target::Defaults(PlaceRef::Node(found.object)),
                (_, false) => Target::Live(found.object),
            };
            // One row at a time: a change that names the same place in several rows reads as naming more places
            // than there are, and is taken for naming somebody from outside.
            for row in rows_here {
                let change = PageChange {
                    rows: Some(vec![RowChange {
                        permission: row.permission.as_str().to_owned(),
                        profile: row.placement,
                        inherit: None,
                        locked: None,
                        add: delta.add.get(&row).map(|adds| {
                            adds.iter()
                                .map(|(holder, expires_at)| EntryAdd {
                                    holder: Holder::Agents { id: *holder },
                                    options: None,
                                    expires_at: *expires_at,
                                    note: None,
                                })
                                .collect()
                        }),
                        update: delta.update.get(&row).map(|updates| {
                            updates
                                .iter()
                                .map(|(entry, expires_at)| EntryUpdate {
                                    entry_id: *entry,
                                    options: None,
                                    expires_at: Some(*expires_at),
                                    note: None,
                                })
                                .collect()
                        }),
                        remove: delta.remove.get(&row).cloned(),
                    }]),
                    ..PageChange::default()
                };
                let outcome = pages::change_by_share(
                    tx,
                    principal,
                    target,
                    &change,
                    policy,
                    share_inside,
                    origin,
                )
                .await?;
                if outcome.changed {
                    match found.subject {
                        Subject::Document(id) => {
                            document_rows::touch(tx, id).await?;
                        }
                        _ => {
                            hierarchy::stamp(tx, found.object).await?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Names agents in the rows of a direct or a group chat, which has no page of its own: the entries are all it
    /// keeps. The caller is in the chat.
    #[allow(
        clippy::too_many_arguments,
        reason = "a chat, a place, the rows, the end and the moment"
    )]
    async fn name_agents_in_chat(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        found: &Found,
        holder: Id,
        wanted: &[Row],
        expires_at: Option<Time>,
        now: DateTime<Utc>,
    ) -> Result<(), ShareError> {
        let page = PageKey::new("chat", found.object, "live");
        let mut changed = false;
        for row in wanted {
            let existing = access::find_entry(
                tx,
                &page,
                "any",
                row.permission.as_str(),
                "agents",
                Some(holder),
                None,
            )
            .await?;
            let end = expires_at.map(Time::get);
            if let Some(existing) = existing {
                if end.is_some() && existing.expires_at != end {
                    access::rewrite_entry(tx, existing.id, &existing.options, end, &existing.note)
                        .await?;
                    changed = true;
                }
                continue;
            }
            let id = orch_types::new_id();
            access::insert_entry(
                tx,
                &NewEntry {
                    id,
                    page: &page,
                    profile: "any",
                    permission: row.permission.as_str(),
                    holder_kind: "agents",
                    holder_id: Some(holder),
                    holder_role: None,
                    options: &json!({}),
                    expires_at: end,
                    note: "",
                    source: "page",
                    created_by: Some(principal.user()),
                    at: now,
                },
            )
            .await?;
            changed = true;
            let mut audit = AuditEntry::new(
                Actor::User(principal.user()),
                AuditAction::PermissionAdded.as_str(),
                AuditAction::PermissionAdded.object_kind().unwrap_or("chat"),
                Some(id),
                "A chat was shared with the agents of a place.",
            );
            audit.org_id = Some(found.org);
            audit.details = json!({
                "permission": row.permission.as_str(), "holder": { "kind": "agents", "id": holder },
                "object": { "kind": "chat", "id": found.object },
            });
            append(tx, &audit, &Origin::default(), now).await?;
        }
        if changed {
            epoch::bump(tx).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- copies

impl Engine {
    /// The limit on the size of a copy of what lies at `node`, and where it was set.
    async fn copy_limit(
        &self,
        conn: &mut Conn,
        found: &Found,
    ) -> Result<(i64, Option<Id>), ShareError> {
        let at = found.node.unwrap_or(found.org);
        let def = definition_of(keys::SHARING_SNAPSHOT_MAX_BYTES)?;
        let snapshot = orch_settings::Snapshot::load(
            conn,
            Place::Node(at),
            orch_settings::Wanted::Keys(&[def]),
        )
        .await
        .map_err(EngineError::from)?;
        let limit = snapshot
            .resolve(def)
            .value
            .and_then(|value| value.as_i64())
            .unwrap_or(i64::MAX);
        Ok((limit, Some(at)))
    }

    /// Makes a copy for each thread named: its files are made once, as the caller may read the thing, kept, and
    /// recorded as waiting for the thread's next turn.
    #[allow(
        clippy::too_many_arguments,
        reason = "who shares what, in which areas, with which threads, and the request that asked"
    )]
    async fn create_copies(
        &self,
        tx: &mut Tx,
        principal: &Principal,
        caller: &Caller,
        now: DateTime<Utc>,
        found: &Found,
        parts: &ShareParts,
        destinations: &[Destination],
        request: &ShareWithAgents,
    ) -> Result<Vec<Record>, ShareError> {
        if let Some(comment) = &request.comment
            && comment.chars().count() > COMMENT_MAX_CHARS
        {
            return Err(invalid("comment", "is longer than an input may be"));
        }
        let Some(parcels) = self.parcels() else {
            return Err(ShareError::Unsupported(
                "nothing is connected that keeps the files of a copy until they are written into a workspace",
            ));
        };
        let mut missing: Vec<(Id, &Destination)> = Vec::new();
        let mut out: Vec<Record> = Vec::new();
        for destination in destinations {
            let id = copy_id(&request.client_id, destination.id, found.kind, found.object);
            match rows::copy(tx, id).await? {
                Some(existing) => {
                    if let Some(mut record) = copy_record(existing) {
                        record.fresh = false;
                        record.input = self.comment_input(tx, &record, caller.user()).await?;
                        out.push(record);
                    }
                }
                None => missing.push((id, destination)),
            }
        }
        if missing.is_empty() {
            return Ok(out);
        }
        let files = self
            .files_of_copy(tx, principal, found, parts, Time::new(now))
            .await?;
        let (limit, scope) = self.copy_limit(tx, found).await?;
        let mut deliveries = Vec::with_capacity(missing.len());
        for (id, destination) in missing {
            let writings = self.copy_writings(tx, destination.id, &files).await?;
            let bytes = files.for_reader(&writings).bytes();
            if bytes > limit {
                return Err(ShareError::TooLarge {
                    bytes,
                    limit,
                    scope,
                });
            }
            deliveries.push((id, destination, writings));
        }
        let include = include_of(parts);
        let object_kind = match found.subject {
            Subject::Node(_) => "node",
            Subject::Chat(_) => "chat",
            Subject::Document(_) => "document",
        };
        let object = match found.subject {
            Subject::Node(id) | Subject::Chat(id) | Subject::Document(id) => id,
        };
        let finished: Finished = {
            let mut unit = Unit::new(tx, now);
            for (id, destination, writings) in &deliveries {
                let theirs = files.for_reader(writings);
                let path = format!("{COPY_ROOT}/{id}/");
                let manifest = parcels
                    .keep(&mut *unit.conn, *id, &path, &theirs)
                    .await
                    .map_err(|fault| {
                        ShareError::Internal(EngineError::unreadable(format!(
                            "the files of a copy could not be kept: {fault}"
                        )))
                    })?;
                let row = rows::insert_copy(
                    &mut *unit.conn,
                    &NewCopy {
                        read_first: request.read_first.unwrap_or(false),
                        id: *id,
                        thread: destination.id,
                        object_kind,
                        object_id: object,
                        include: &include,
                        path: &path,
                        manifest: manifest.as_deref(),
                        file_count: i32::try_from(theirs.files.len()).unwrap_or(i32::MAX),
                        total_bytes: theirs.bytes(),
                        as_of: now,
                        by: principal.user(),
                    },
                )
                .await?;
                let mut audit = AuditEntry::new(
                    Actor::User(principal.user()),
                    orch_db::audit::action!(AuditAction::ShareSnapshotSent).as_str(),
                    "thread",
                    Some(destination.id),
                    "A copy was shared with an agent.",
                );
                audit.history = AuditAction::ShareSnapshotSent.is_history_line();
                audit.node_id = Some(destination.id);
                audit.org_id = Some(found.org);
                audit.details = json!({
                    "copy": id, "what": { "kind": object_kind, "id": object },
                    "files": row.file_count, "bytes": row.total_bytes, "path": path,
                });
                append(&mut *unit.conn, &audit, &Origin::default(), now).await?;
                let mut record = copy_record(row).ok_or_else(|| {
                    EngineError::unreadable(
                        "a copy was made of something that is none of the three",
                    )
                })?;
                if let Some(comment) = request
                    .comment
                    .as_deref()
                    .filter(|comment| !comment.trim().is_empty())
                {
                    let taken = self
                        .submit_input_in(
                            &mut unit,
                            caller,
                            destination.id,
                            &InputRequest {
                                plan: false,
                                content: vec![ContentPart::Text {
                                    text: comment.to_owned(),
                                }],
                                delivery: Delivery::Queue,
                                client_id: comment_client(*id),
                                bypass: None,
                            },
                        )
                        .await?;
                    record.input = Some(taken.submitted.input.id);
                }
                out.push(record);
            }
            unit.finish().await.map_err(ShareError::from)?
        };
        for thread in &finished.wakes {
            self.wake(*thread);
        }
        out.sort_by_key(|record| {
            destinations
                .iter()
                .position(|each| each.id == record.to)
                .unwrap_or(usize::MAX)
        });
        Ok(out)
    }

    /// The input that carried the comment of a copy, if there was one.
    async fn comment_input(
        &self,
        conn: &mut Conn,
        record: &Record,
        by: Id,
    ) -> Result<Option<Id>, ShareError> {
        Ok(
            threads::input_by_client(conn, record.to, Some(by), &comment_client(record.id))
                .await?
                .map(|row| row.id),
        )
    }
}

// ---------------------------------------------------------------- telling the thread

impl Engine {
    /// Tells a thread that something was shared with it, or that a share ended: a line of its history and a notice
    /// for its next turn. Once for each share.
    async fn tell_thread(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        record: &Record,
        found: &Found,
        by: Id,
    ) -> Result<(), ShareError> {
        self.put_share_line(unit, thread, record, found, by, true)
            .await
            .map_err(ShareError::from)
    }

    /// The marker and the notice of a share, for `thread`: `begun` is whether it was made or ended.
    pub(crate) async fn put_share_line(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        record: &Record,
        found: &Found,
        by: Id,
        begun: bool,
    ) -> Result<(), EngineError> {
        let Some(row) = threads::thread(&mut *unit.conn, thread).await? else {
            return Ok(());
        };
        let who = crate::views::user_ref_json(&mut *unit.conn, by).await?;
        let display = who
            .get("display_name")
            .and_then(Json::as_str)
            .unwrap_or("Someone")
            .to_owned();
        let mode = match record.mode {
            ShareMode::Live => "live",
            ShareMode::Snapshot => "snapshot",
        };
        let path = record
            .copy
            .as_ref()
            .map(|copy| copy.path.clone())
            .map(Json::String)
            .unwrap_or(Json::Null);
        let what = match record.subject {
            Subject::Node(node) => {
                let kind = found
                    .place_kind
                    .clone()
                    .unwrap_or_else(|| "topic".to_owned());
                Some(
                    json!({ "node": { "id": node, "kind": kind, "name": found.name, "redacted": false } }),
                )
            }
            Subject::Chat(chat) => {
                let kind = if found.people_chat { "group" } else { "node" };
                Some(json!({ "chat": {
                    "id": chat, "kind": kind, "org_id": found.org, "node": null, "name": found.name,
                } }))
            }
            Subject::Document(document) => {
                Some(json!({ "document": { "id": document, "title": found.name } }))
            }
        };
        if let (Some(what), Some(segment)) = (what, row.current_segment_id) {
            let name = if begun { "shared" } else { "unshared" };
            let payload = json!({
                "marker": name,
                "data": { "what": what, "mode": mode, "path": path, "by": who,
                    "parts": record.parts, "read_first": record.read_first },
            });
            // Entry revisions distinguish a changed live share while retries of the same state
            // still produce one line. A removed and re-added row has a new entry identifier.
            let mut versions: Vec<_> = record
                .grants
                .iter()
                .map(|grant| (grant.entry_id, grant.revision))
                .collect();
            versions.sort_unstable();
            let key = format!("{name}:{}:{versions:?}", record.id);
            crate::ingest::add_marker(unit, thread, segment, None, &payload, &key).await?;
        }
        let body = notice_body(record, found, begun);
        let id = format!("{}:{}", if begun { "share" } else { "unshare" }, record.id);
        let notice = json!({
            "id": id, "kind": "share", "at": Time::new(unit.now()), "from": display,
            "from_thread": false, "title": if begun { "Shared with you" } else { "No longer shared with you" },
            "body": body,
        });
        if threads::notices::append(&mut *unit.conn, thread, &notice).await? {
            let seq = unit.stamp(thread).await?;
            threads::set_seq_state(&mut *unit.conn, thread, seq).await?;
        }
        Ok(())
    }
}

/// What the notice for a thread's next turn says.
fn notice_body(record: &Record, found: &Found, begun: bool) -> String {
    let what = match record.subject {
        Subject::Node(_) => format!(
            "the {} \"{}\"",
            found.place_kind.as_deref().unwrap_or("place"),
            found.name
        ),
        Subject::Chat(_) => format!("the chat \"{}\"", found.name),
        Subject::Document(_) => format!("the document \"{}\"", found.name),
    };
    match (&record.copy, begun) {
        (Some(copy), true) => format!(
            "A copy of {what} is in {} in your workspace, as it was when it was shared.",
            copy.path
        ),
        (Some(copy), false) => format!(
            "The copy of {what} in {} was taken back and is removed from your workspace.",
            copy.path
        ),
        (None, true) => {
            let manages = record.parts.documents == Some(ShareDocumentsLevel::Manage);
            let writes = manages
                || record.parts.timeline == Some(ShareLevel::Write)
                || record.parts.chat == Some(ShareLevel::Write)
                || record.parts.documents == Some(ShareDocumentsLevel::Edit);
            format!(
                "{what} is shared with you{}{}: list_shared shows it.",
                if writes { ", also for writing" } else { "" },
                if manages {
                    ", and its documents to manage (manage_shared)"
                } else {
                    ""
                }
            )
        }
        (None, false) => format!("{what} is not shared with you any more."),
    }
}

// ---------------------------------------------------------------- listing

impl Engine {
    /// The live shares and the copies of a thing, for a person who may see it.
    pub async fn shares_of(
        &self,
        caller: &Caller,
        origin: &Origin,
        subject: Subject,
    ) -> Result<Vec<Record>, ShareError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
        let (found, _) = self
            .find_subject(&mut conn, &principal, origin, now, subject)
            .await?;
        let mut records = self.live_records(&mut conn, &found, now).await?;
        let object_kind = match subject {
            Subject::Node(_) => "node",
            Subject::Chat(_) => "chat",
            Subject::Document(_) => "document",
        };
        let id = match subject {
            Subject::Node(id) | Subject::Chat(id) | Subject::Document(id) => id,
        };
        for row in rows::copies_of(&mut conn, object_kind, id).await? {
            if let Some(mut record) = copy_record(row) {
                record.input = self
                    .comment_input(&mut conn, &record, record.by.unwrap_or_default())
                    .await?;
                records.push(record);
            }
        }
        Ok(records)
    }

    /// What was shared with the agents of a place, or of a place above it, as far as the caller may see each
    /// thing.
    pub async fn shares_to_place(
        &self,
        caller: &Caller,
        origin: &Origin,
        node: Id,
    ) -> Result<Vec<Record>, ShareError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
        let place = rows::places(&mut conn, &[node])
            .await?
            .into_iter()
            .next()
            .ok_or(ShareError::NotFound)?;
        let mut holders = place.path.clone();
        if !holders.contains(&node) {
            holders.push(node);
        }
        let mut records = self.records_to(&mut conn, &holders, now).await?;
        records = self
            .visible_to(&mut conn, &principal, origin, now, records)
            .await?;
        Ok(records)
    }

    /// What this thread's agent may read beyond its own thread: the policy of its topic, the shares that name it
    /// or a place above it, and the copies it was given, as far as the caller may see each thing.
    pub async fn shares_of_thread(
        &self,
        caller: &Caller,
        origin: &Origin,
        thread: Id,
    ) -> Result<ThreadShares, ShareError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
        let row = threads::thread(&mut conn, thread)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(ShareError::NotFound)?;
        let place = rows::places(&mut conn, &[thread])
            .await?
            .into_iter()
            .next()
            .ok_or(ShareError::NotFound)?;
        let policy = orch_settings::get::<String>(
            &mut conn,
            Place::Node(row.topic_id),
            keys::SHARING_AGENT_REACH,
        )
        .await
        .map_err(EngineError::from)?
        .and_then(|word| AgentReach::parse(&word))
        .unwrap_or_default();
        let mut holders = place.path.clone();
        if !holders.contains(&thread) {
            holders.push(thread);
        }
        let mut shares = self.records_to(&mut conn, &holders, now).await?;
        shares = self
            .visible_to(&mut conn, &principal, origin, now, shares)
            .await?;
        for copy in rows::copies_to(&mut conn, &[thread]).await? {
            if let Some(record) = copy_record(copy) {
                shares.push(record);
            }
        }
        shares.sort_by_key(|record| (record.at, record.id));
        let material = self.topic_material(&mut conn, thread).await?;
        Ok(ThreadShares {
            policy,
            shares,
            material,
        })
    }

    /// The live shares that name any of the places `holders`.
    async fn records_to(
        &self,
        conn: &mut Conn,
        holders: &[Id],
        now: DateTime<Utc>,
    ) -> Result<Vec<Record>, ShareError> {
        let permissions: Vec<String> = SHARE_PERMISSIONS
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect();
        let grants = rows::grants_to(conn, holders, &permissions, now).await?;
        Ok(records_of(grants))
    }

    /// Of these shares, those whose thing the caller may see.
    async fn visible_to(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        origin: &Origin,
        now: DateTime<Utc>,
        records: Vec<Record>,
    ) -> Result<Vec<Record>, ShareError> {
        let mut out = Vec::new();
        for record in records {
            let seen = self
                .find_subject(conn, principal, origin, now, record.subject)
                .await;
            match seen {
                Ok(_) => out.push(record),
                Err(ShareError::NotFound | ShareError::Archived { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- changing and taking back

/// A share looked for by its identifier: a live one, or a copy.
#[derive(Debug, Clone)]
enum Located {
    Live(Record),
    Copy(Record),
}

impl Engine {
    /// The share with this identifier, if it stands.
    async fn locate_share(
        &self,
        conn: &mut Conn,
        id: Id,
        now: DateTime<Utc>,
    ) -> Result<Located, ShareError> {
        if let Some(row) = rows::copy(conn, id).await? {
            return match copy_record(row) {
                Some(record)
                    if record
                        .copy
                        .as_ref()
                        .is_some_and(|copy| copy.state != "revoked") =>
                {
                    Ok(Located::Copy(record))
                }
                _ => Err(ShareError::NotFound),
            };
        }
        let permissions: Vec<String> = SHARE_PERMISSIONS
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect();
        let grants = rows::all_grants(conn, &permissions, now).await?;
        records_of(grants)
            .into_iter()
            .find(|record| record.id == id)
            .map(Located::Live)
            .ok_or(ShareError::NotFound)
    }

    /// Changes the areas or the end of a live share; what the request leaves out stays.
    pub async fn update_share(
        &self,
        caller: &Caller,
        origin: &Origin,
        share: Id,
        change: &orch_api_contract::share::ShareAgentUpdateBody,
    ) -> Result<Record, ShareError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        self.update_share_guarded(tx, principal, origin, share, change)
            .await
    }

    /// Changes a live share as [`Self::update_share`] does, in a transaction whose caller was asked again once
    /// the request had come (see [`Self::share_with_agents_guarded`]).
    pub async fn update_share_guarded(
        &self,
        mut tx: Tx,
        principal: Principal,
        origin: &Origin,
        share: Id,
        change: &orch_api_contract::share::ShareAgentUpdateBody,
    ) -> Result<Record, ShareError> {
        let now = principal.now().get();
        let record = match self.locate_share(&mut tx, share, now).await? {
            Located::Live(record) => record,
            Located::Copy(_) => {
                return Err(invalid("share", "a copy has no areas or end to change"));
            }
        };
        let (found, loaded) = self
            .find_subject(&mut tx, &principal, origin, now, record.subject)
            .await?;
        if found.people_chat && !loaded.is_some_and(|chat| chat.standing.in_chat) {
            return Err(ShareError::NotFound);
        }
        if let Some(end) = change.expires_at
            && end.get() <= now
        {
            return Err(invalid("expires_at", "is already past"));
        }
        let parts = match change.parts {
            Some(asked) => parts_asked(&found, Some(asked), ShareMode::Live, record.inside)?,
            None => record.parts,
        };
        if change.read_first == Some(true) && parts.documents.is_none() {
            return Err(invalid("read_first", "requires included document material"));
        }
        let mut wanted: BTreeSet<Row> =
            rows_of(&found, &parts, record.inside).into_iter().collect();
        // The export of a place: a share that let the agents copy what it shares goes on doing so for the areas
        // it has now, as far as its sharer holds the Export rows.
        let downloads = record.grants.iter().any(|grant| {
            grant
                .permission
                .parse::<Permission>()
                .is_ok_and(Permission::is_export)
        });
        if downloads {
            wanted.extend(export_rows_of(&found, &parts, record.inside));
        }
        let mut standing: BTreeMap<Row, Id> = BTreeMap::new();
        for grant in &record.grants {
            if let Some(row) = row_of(grant) {
                standing.insert(row, grant.entry_id);
            }
        }
        let mut delta = Delta::default();
        let end = change.expires_at;
        // What is added is asked of the sharer's own holding, as when it was made; what stands stays.
        let to_add: Vec<Row> = wanted
            .iter()
            .filter(|row| !standing.contains_key(row))
            .copied()
            .collect();
        let (soft_add, strict_add): (Vec<Row>, Vec<Row>) = to_add
            .into_iter()
            .partition(|row| row.permission.is_export());
        let mut may_add: Vec<Row> = if found.people_chat || strict_add.is_empty() {
            strict_add
        } else {
            self.rows_the_sharer_may_give(&mut tx, &principal, &found, strict_add)
                .await?
        };
        may_add.extend(if found.people_chat || soft_add.is_empty() {
            soft_add
        } else {
            self.export_rows_the_sharer_may_give(&mut tx, &principal, &found, soft_add)
                .await?
        });
        for row in &wanted {
            match standing.get(row) {
                None if may_add.contains(row) => delta.add(*row, record.to, end),
                None => {}
                Some(entry) if end.is_some() => delta.update(*row, *entry, end),
                Some(_) => {}
            }
        }
        for (row, entry) in &standing {
            if !wanted.contains(row) {
                delta.remove(*row, *entry);
            }
        }
        if found.people_chat {
            let missing: Vec<Row> = wanted
                .iter()
                .filter(|row| !standing.contains_key(row))
                .copied()
                .collect();
            self.name_agents_in_chat(&mut tx, &principal, &found, record.to, &missing, end, now)
                .await?;
            for (row, entry) in &standing {
                if !wanted.contains(row) {
                    self.take_entry_out(&mut tx, principal.user(), &found, *entry, now)
                        .await?;
                } else if let Some(end) = end
                    && let Some(existing) = access::find_entry(
                        &mut tx,
                        &PageKey::new("chat", found.object, "live"),
                        "any",
                        row.permission.as_str(),
                        "agents",
                        Some(record.to),
                        None,
                    )
                    .await?
                {
                    access::rewrite_entry(
                        &mut tx,
                        existing.id,
                        &existing.options,
                        Some(end.get()),
                        &existing.note,
                    )
                    .await?;
                }
            }
        } else {
            let reach = (matches!(found.subject, Subject::Node(_))
                && matches!(
                    found.place_kind.as_deref(),
                    Some("org" | "project" | "topic")
                ))
            .then_some(record.inside);
            self.apply_delta(&mut tx, &principal, origin, &found, &delta, reach)
                .await?;
        }
        if let Some(read_first) = change.read_first {
            let current = self.live_records(&mut tx, &found, now).await?;
            if let Some(current) = current.iter().find(|entry| entry.id == share) {
                rows::read_first(
                    &mut tx,
                    &current
                        .grants
                        .iter()
                        .map(|entry| entry.entry_id)
                        .collect::<Vec<_>>(),
                    read_first,
                )
                .await?;
            }
        }
        let mut shares = self.live_records(&mut tx, &found, now).await?;
        let updated = shares
            .drain(..)
            .find(|each| each.id == share)
            .ok_or(ShareError::NotFound)?;
        let destination_thread = threads::thread(&mut tx, updated.to).await?.is_some();
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            if destination_thread {
                self.put_share_line(
                    &mut unit,
                    updated.to,
                    &updated,
                    &found,
                    principal.user(),
                    true,
                )
                .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(updated)
    }

    /// Takes a share back. A live share ends at the agent's next call; a copy that was not delivered yet is
    /// cancelled, and one that was is taken out of the workspace.
    pub async fn revoke_share(
        &self,
        caller: &Caller,
        origin: &Origin,
        share: Id,
    ) -> Result<(), ShareError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        self.revoke_share_guarded(tx, principal, origin, share)
            .await
    }

    /// Takes a share back as [`Self::revoke_share`] does, in a transaction whose caller was asked again once
    /// the request had come (see [`Self::share_with_agents_guarded`]).
    pub async fn revoke_share_guarded(
        &self,
        mut tx: Tx,
        principal: Principal,
        origin: &Origin,
        share: Id,
    ) -> Result<(), ShareError> {
        let now = principal.now().get();
        let located = self.locate_share(&mut tx, share, now).await?;
        let (record, found, destination_thread) = match &located {
            Located::Live(record) => {
                let (found, loaded) = self
                    .find_subject(&mut tx, &principal, origin, now, record.subject)
                    .await?;
                if found.people_chat && !loaded.is_some_and(|chat| chat.standing.in_chat) {
                    // Whoever shared it may take it back even from a chat they left.
                    if record.by != Some(principal.user()) {
                        return Err(ShareError::NotFound);
                    }
                }
                (
                    record.clone(),
                    found,
                    rows::places(&mut tx, &[record.to])
                        .await?
                        .into_iter()
                        .next(),
                )
            }
            Located::Copy(record) => {
                let found = self
                    .found_for_copy(&mut tx, &principal, origin, now, record)
                    .await?;
                (
                    record.clone(),
                    found,
                    rows::places(&mut tx, &[record.to])
                        .await?
                        .into_iter()
                        .next(),
                )
            }
        };
        match &located {
            Located::Live(_) => {
                let mut delta = Delta::default();
                for grant in &record.grants {
                    if let Some(row) = row_of(grant) {
                        delta.remove(row, grant.entry_id);
                    }
                }
                if found.people_chat {
                    for grant in &record.grants {
                        self.take_entry_out(&mut tx, principal.user(), &found, grant.entry_id, now)
                            .await?;
                    }
                } else {
                    // Whoever manages the page takes back any share; whoever shared it, the entries they made,
                    // for as long as they could give them again (the page's own rule).
                    let reach = (matches!(found.subject, Subject::Node(_))
                        && matches!(
                            found.place_kind.as_deref(),
                            Some("org" | "project" | "topic")
                        ))
                    .then_some(record.inside);
                    self.apply_delta(&mut tx, &principal, origin, &found, &delta, reach)
                        .await?;
                }
            }
            Located::Copy(_) => {
                let copy = record.copy.clone().ok_or(ShareError::NotFound)?;
                if copy.created_by != principal.user() {
                    // Whoever could share the same thing now: the thing is readable by them and the thread theirs to drive.
                    match nodes::check_named(
                        &mut tx,
                        &principal,
                        copy.thread_id,
                        PermissionName::OfNode(NodePermission::Interact),
                        Act::Use,
                        Policy::default(),
                    )
                    .await?
                    {
                        Decision::Allowed { .. } => {}
                        Decision::Refused(refusal) => {
                            return Err(ShareError::Denied(Box::new(refusal)));
                        }
                    }
                }
                if let Some(parcels) = self.parcels() {
                    parcels.withdraw(&mut tx, &copy).await.map_err(|fault| {
                        ShareError::Internal(EngineError::unreadable(format!(
                            "a copy could not be taken out of the workspace: {fault}"
                        )))
                    })?;
                }
                rows::revoke_copy(&mut tx, copy.id, principal.user(), now).await?;
                let mut audit = AuditEntry::new(
                    Actor::User(principal.user()),
                    orch_db::audit::action!(AuditAction::ShareSnapshotRevoked).as_str(),
                    "thread",
                    Some(copy.thread_id),
                    "A copy that was shared with an agent was taken back.",
                );
                audit.history = AuditAction::ShareSnapshotRevoked.is_history_line();
                audit.node_id = Some(copy.thread_id);
                audit.org_id = Some(found.org);
                audit.details = json!({ "copy": copy.id, "was": copy.state, "path": copy.path });
                append(&mut tx, &audit, &Origin::default(), now).await?;
            }
        }
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            if destination_thread
                .as_ref()
                .is_some_and(|place| place.kind == "thread")
            {
                self.put_share_line(
                    &mut unit,
                    record.to,
                    &record,
                    &found,
                    principal.user(),
                    false,
                )
                .await?;
            }
            unit.finish().await.map_err(ShareError::from)?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// What a copy is of, found again for a person who takes it back: the thing may be gone, and then it is
    /// described by what the copy remembers of it.
    async fn found_for_copy(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        origin: &Origin,
        now: DateTime<Utc>,
        record: &Record,
    ) -> Result<Found, ShareError> {
        match self
            .find_subject(conn, principal, origin, now, record.subject)
            .await
        {
            Ok((found, _)) => Ok(found),
            Err(ShareError::NotFound) => {
                let copy = record.copy.as_ref().ok_or(ShareError::NotFound)?;
                let place = rows::places(conn, &[copy.thread_id])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or(ShareError::NotFound)?;
                Ok(Found {
                    subject: record.subject,
                    kind: "node",
                    object: copy.object_id,
                    org: place.org_id,
                    node: None,
                    place_kind: None,
                    name: String::new(),
                    people_chat: false,
                })
            }
            Err(other) => Err(other),
        }
    }

    /// Takes one entry out without the page's check: for the person who made it, and for an entry of a chat.
    async fn take_entry_out(
        &self,
        conn: &mut Conn,
        by: Id,
        found: &Found,
        entry: Id,
        now: DateTime<Utc>,
    ) -> Result<(), ShareError> {
        if !access::remove_entry(conn, entry, by, now).await? {
            return Ok(());
        }
        let mut audit = AuditEntry::new(
            Actor::User(by),
            AuditAction::PermissionRemoved.as_str(),
            AuditAction::PermissionRemoved
                .object_kind()
                .unwrap_or(found.kind),
            Some(entry),
            "Agents were taken out of a row of a page.",
        );
        audit.node_id = found.node;
        audit.org_id = Some(found.org);
        audit.details = json!({ "object": { "kind": found.kind, "id": found.object } });
        append(conn, &audit, &Origin::default(), now).await?;
        epoch::bump(conn).await?;
        Ok(())
    }
}

/// The row of a page that an entry stands in.
fn row_of(grant: &GrantRow) -> Option<Row> {
    let permission: Permission = grant.permission.parse().ok()?;
    let placement = match grant.profile.as_str() {
        "managed" => Some(Placement::Managed),
        "connected" => Some(Placement::Connected),
        _ => None,
    };
    Some(Row {
        defaults: grant.page == "defaults",
        permission,
        placement,
    })
}

// ---------------------------------------------------------------- forwarding to an agent's thread

/// An input that a forward queued for a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardedInput {
    /// The thread.
    pub thread: Id,
    /// The input, by the caller.
    pub input: orch_api_contract::threads::ThreadInput,
    /// Whether this request queued it, and did not find it queued by the same request before.
    pub created: bool,
}

/// Inputs queued for threads inside the transaction of a forward, and what is left for after its commit.
#[derive(Debug, Clone)]
pub struct ForwardedToThreads {
    /// The inputs, in ascending order of their threads.
    pub inputs: Vec<ForwardedInput>,
    after: Finished,
}

impl Engine {
    /// Queues one input by the caller for each of the threads, with the same parts and `delivery: queue`: checked,
    /// counted against usage caps and attributed as any input of theirs. It is written in the caller's transaction,
    /// together with whatever else the caller writes there, and nothing is written where one thread refuses: the
    /// error names that thread. After the transaction has committed, [`Engine::after_forward`] takes up the work.
    pub async fn forward_to_threads(
        &self,
        tx: &mut Tx,
        caller: &Caller,
        to: &[Id],
        content: &[ContentPart],
        client_id: &str,
    ) -> Result<ForwardedToThreads, (Option<Id>, InputError)> {
        let mut threads_sorted: Vec<Id> = to.to_vec();
        threads_sorted.sort_unstable();
        threads_sorted.dedup();
        let now = self.inner.clock.now();
        let mut unit = Unit::new(tx, now);
        let mut inputs = Vec::with_capacity(threads_sorted.len());
        for thread in threads_sorted {
            let taken = self
                .submit_input_in(
                    &mut unit,
                    caller,
                    thread,
                    &InputRequest {
                        plan: false,
                        content: content.to_vec(),
                        delivery: Delivery::Queue,
                        client_id: client_id.to_owned(),
                        bypass: None,
                    },
                )
                .await
                .map_err(|error| (Some(thread), error))?;
            inputs.push(ForwardedInput {
                thread,
                input: taken.submitted.input,
                created: taken.submitted.created,
            });
        }
        let after = unit
            .finish()
            .await
            .map_err(|error| (None, InputError::from(error)))?;
        Ok(ForwardedToThreads { inputs, after })
    }

    /// Takes up the work that queued inputs ask for, once the transaction they were queued in has committed.
    pub fn after_forward(&self, forwarded: &ForwardedToThreads) {
        for thread in &forwarded.after.wakes {
            self.wake(*thread);
        }
    }
}
