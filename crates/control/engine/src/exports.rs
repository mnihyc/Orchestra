//! The export of a place: what a person may take away of the conversations, chats and documents in an org, a project or folder, a topic or a thread, planned before anything is made, asked for as a job, and kept as an archive for them alone to fetch for a day.
//!
//! **What is in an archive is decided when it is written, by its person's access then.** [`Engine::plan_export`]
//! says beforehand what the archive would hold as far as the caller may read it, what they may read and not take
//! away (their Export row of a thing: `thread.export`, `chat.export`, `doc.export`), and what stands in its way;
//! nothing is made. [`Engine::make_export`] makes the row and the job that writes it (`jobs::export`), which asks
//! the access engine again for each thing as it writes it. What the caller may not read is neither written nor
//! named, in the plan or in what is left out.
//!
//! A single export of a thread, a chat or a document is the same text as the file of that thing in an archive.
//! What only the API can write (a document with its names, a thread as the API shows it) is given by the
//! program the engine lives in, through [`Forms`], the way the files of a copy are kept through
//! [`crate::sharing::Parcels`].

use crate::engine::Engine;
use crate::error::EngineError;
use crate::export_layout::{self, Layout};
use crate::jobs::export as the_job;
use crate::jobs::runner::initial_progress;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::exports::{
    EXPORT_CHOSEN_MAX, Export, ExportCounts, ExportLeftOut, ExportLeftOutWhat, ExportLimits,
    ExportPlan, ExportRequest, ExportState, ExportWhat,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::threads::{ExportDetail, ExportFormat, Thread};
use orch_authz::{Act, Caller, Policy, Principal, documents as document_access, nodes};
use orch_blob_contract::store::BlobRead;
use orch_db::audit::{Actor, Entry as AuditEntry, Origin, append};
use orch_db::queries::exports::{self as rows, ExportRow, FileFacts, Span};
use orch_db::queries::jobs::{self as job_rows, JobRow, NewJob, kind};
use orch_db::queries::shares::{self as places, PlaceRow};
use orch_db::{Conn, DbError};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, PoisonError};

pub mod download;
pub mod operation;
mod scope;
pub use scope::ScopedExportPlan;

/// The most places and threads one archive looks through. A place with more says so, and is exported as far as
/// this reaches: past it the walk would be longer than a request may wait for.
pub const NODES_MAX: i64 = 20_000;

/// How many things have their access asked for again together, just before they are written: few enough for the
/// decision to be as fresh as the writing, many enough for it to take a handful of statements for all.
pub const BATCH: usize = 100;

/// What an entry of a zip costs beside what it holds: its header, and its line in the list at the end.
pub const ENTRY_OVERHEAD_BYTES: i64 = 200;

/// What the index says of each thing: a line of its path and what it is.
pub const INDEX_LINE_BYTES: i64 = 160;

/// What the head of a conversation or of a document comes to beside what is stored of it.
pub const HEAD_BYTES: i64 = 128;

/// What an object of a package's manifest comes to: its kind, identifier, parent, two paths, name, times, creator
/// and two hashes, written out; about six hundred characters, taken up to seven hundred for the longer names.
pub const PACKAGE_OBJECT_BYTES: i64 = 700;

/// The largest archive when the setting is not there: two gibibytes.
const DEFAULT_MAX_BYTES: i64 = 2_147_483_648;

/// The most things in one archive when the setting is not there.
const DEFAULT_MAX_ITEMS: i64 = 10_000;

/// How long an archive is kept when the setting is not there, in hours.
const DEFAULT_KEEP_HOURS: i64 = 24;

// ---------------------------------------------------------------- what only the API can write

/// Why a form could not be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FormsFault(pub String);

/// How a document is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentHow {
    /// Its blocks as records, Markdown, or an HTML reading copy.
    pub format: orch_api_contract::documents::DocumentExport,
    /// Whether its files are carried beside its reading copies.
    pub files: bool,
    /// Its discussions with it.
    pub comments: bool,
    /// The directory its Markdown links its files in: the one beside the file the text is kept in
    /// ([`crate::written::files_beside`]), so that a link leads to the file where the text and its files lie
    /// together.
    pub beside: String,
}

/// A document as its single export writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentForm {
    /// What the single export answers with.
    pub bytes: Vec<u8>,
    /// What Markdown could not keep of it, as a sentence; empty where it kept all.
    pub losses: String,
}

/// The native form whose existing live references are selected before formatting.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Form {
    /// The native thread record header.
    Thread { id: Id },
    /// A document's actual requested form; JSON includes its full metadata.
    Document { id: Id, json: bool, comments: bool },
}
/// A native protected reference, not an identifier discovered in a returned DTO.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FormReference {
    /// A node name, document title, folder name or other current protected reference.
    Object {
        object: orch_access_contract::permissions::ObjectKind,
        id: Id,
    },
    /// A chat name or one of its carried file names.
    Chat { id: Id },
    /// A binding/environment reference the native readable Thread publishes.
    Published {
        object: orch_access_contract::permissions::ObjectKind,
        id: Id,
    },
}
/// Genuine form inputs before serialization; absence of this handoff is never an empty proof.
pub struct FormSources {
    /// Only the actual existing references this native form materializes.
    pub references: Vec<FormReference>,
}

/// What writes the forms of a single export that need the views of the API: a document with the names a reader
/// is given, and a thread as the API shows it. The program the engine lives in attaches it; a single export and
/// an archive both ask it, so that a thing in an archive is the very bytes of its single export.
#[async_trait]
pub trait Forms: Send + Sync {
    /// Existing native factual selectors, before the Thread/Document DTO or bytes are built.
    /// An unreported source remains unsupported for scoped forms; old in-process forms stay ordinary.
    async fn sources(
        &self,
        _conn: &mut Conn,
        _asker: &Principal,
        _form: Form,
    ) -> Result<Option<FormSources>, FormsFault> {
        Ok(None)
    }
    /// The thread as `asker` sees it, which is the `thread` of its records.
    async fn thread(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        thread: Id,
        now: DateTime<Utc>,
    ) -> Result<Thread, FormsFault>;

    /// The document as `asker` exports it.
    async fn document(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        document: Id,
        how: DocumentHow,
        now: DateTime<Utc>,
    ) -> Result<DocumentForm, FormsFault>;
}

impl Engine {
    /// Attaches what writes the forms that need the API's views. What was attached before is replaced.
    pub fn attach_forms(&self, forms: Arc<dyn Forms>) {
        *self
            .inner
            .forms
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(forms);
    }

    /// What writes the forms that need the API's views, if it was attached.
    #[must_use]
    pub fn forms(&self) -> Option<Arc<dyn Forms>> {
        self.inner
            .forms
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

// ---------------------------------------------------------------- why a request is not carried out

/// Why an archive cannot be made as it is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// The caller may take nothing of what was asked for away.
    Nothing,
    /// The archive would hold more things than the place allows.
    TooMany,
    /// The archive would be larger than the place allows.
    TooLarge,
}

impl Blocked {
    /// The code of the refusal it comes to.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Nothing => "export.nothing",
            Self::TooMany => "export.too_many",
            Self::TooLarge => "export.too_large",
        }
    }
}

/// Why an export could not be planned, asked for, read or removed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// There is no such place or archive, or the caller may not know of it.
    #[error("there is no such thing")]
    NotFound,
    /// The request does not fit.
    #[error("{field}: {message}")]
    Invalid {
        /// Where in the request.
        field: String,
        /// What is wrong.
        message: String,
    },
    /// The archive would pass what the place allows, or there is nothing to take away.
    #[error("the archive cannot be made as asked: {message}")]
    Blocked {
        /// Which.
        by: Blocked,
        /// The limit that was passed, where one was.
        limit: i64,
        /// What was asked.
        asked: i64,
        /// In a sentence.
        message: String,
    },
    /// The person has as many archives being written as they may.
    #[error("as many archives are being written as one person may have: {limit}")]
    TooManyAtOnce {
        /// How many.
        limit: u32,
    },
    /// The archive was removed, or its time ran out.
    #[error("the archive is gone")]
    Gone,
    /// The caller is suspended where this is.
    #[error("the caller is suspended")]
    Suspended {
        /// The org; none for altogether.
        org: Option<Id>,
    },
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for ExportError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for ExportError {
    fn from(error: orch_authz::Error) -> Self {
        match error {
            orch_authz::Error::NotFound(_) => Self::NotFound,
            orch_authz::Error::Suspended { org } => Self::Suspended { org },
            other => Self::Internal(other.into()),
        }
    }
}

impl From<orch_settings::Error> for ExportError {
    fn from(error: orch_settings::Error) -> Self {
        Self::Internal(error.into())
    }
}

fn invalid(field: &str, message: impl Into<String>) -> ExportError {
    ExportError::Invalid {
        field: field.to_owned(),
        message: message.into(),
    }
}

// ---------------------------------------------------------------- what a request asks, read

/// A request, read: what is left out of it is filled in, and the days are moments.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    /// Everything in the place that the caller may read.
    pub everything: bool,
    /// Places and threads chosen, each with what is in it.
    pub nodes: Vec<Id>,
    /// Documents chosen.
    pub documents: Vec<Id>,
    /// Chats chosen.
    pub chats: Vec<Id>,
    /// Whether conversations are in it.
    pub conversations_on: bool,
    /// Whether chats are in it.
    pub chats_on: bool,
    /// Whether documents are in it.
    pub documents_on: bool,
    /// The forms the things are written in.
    pub format: ExportFormat,
    /// How much of a conversation.
    pub detail: ExportDetail,
    /// The files that inputs, messages and documents carry.
    pub files: bool,
    /// The earlier versions of documents.
    pub versions: bool,
    /// The discussions of documents.
    pub comments: bool,
    /// Whether the archive is an Orch package: each thing as its record in JSON and as its readable form, and a
    /// manifest at the root.
    pub package: bool,
    /// The turns and the messages of these days.
    pub span: Span,
}

fn day(field: &str, text: Option<&str>) -> Result<Option<NaiveDate>, ExportError> {
    let Some(text) = text else {
        return Ok(None);
    };
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| invalid(field, "is a day written YYYY-MM-DD"))
}

impl Wanted {
    /// Reads a request.
    pub fn read(request: &ExportRequest) -> Result<Self, ExportError> {
        let (everything, chosen) = match &request.what {
            ExportWhat::Everything(_) => {
                (true, orch_api_contract::exports::ExportChosen::default())
            }
            ExportWhat::Chosen(chosen) => (false, chosen.clone()),
        };
        let limit = EXPORT_CHOSEN_MAX as usize;
        for (field, list) in [
            ("what.nodes", &chosen.nodes),
            ("what.documents", &chosen.documents),
            ("what.chats", &chosen.chats),
        ] {
            if list.as_ref().is_some_and(|list| list.len() > limit) {
                return Err(invalid(
                    field,
                    format!("names more than {EXPORT_CHOSEN_MAX}"),
                ));
            }
        }
        let parts = request.parts.unwrap_or_default();
        let from = day("from", request.from.as_deref())?;
        let to = day("to", request.to.as_deref())?;
        if let (Some(from), Some(to)) = (from, to)
            && to < from
        {
            return Err(invalid("to", "is before the first day"));
        }
        let moment = |date: NaiveDate| {
            date.and_hms_opt(0, 0, 0)
                .map(|time| time.and_utc())
                .unwrap_or_default()
        };
        // A package is written to be read again: the records are JSON, and what a thing carries and what was said of
        // it come with it unless it is asked that they do not.
        let package = request.package.unwrap_or(false);
        Ok(Self {
            everything,
            nodes: chosen.nodes.unwrap_or_default(),
            documents: chosen.documents.unwrap_or_default(),
            chats: chosen.chats.unwrap_or_default(),
            conversations_on: parts.conversations.unwrap_or(true),
            chats_on: parts.chats.unwrap_or(true),
            documents_on: parts.documents.unwrap_or(true),
            format: if package {
                ExportFormat::Json
            } else {
                request.format.unwrap_or(ExportFormat::Markdown)
            },
            detail: request.detail.unwrap_or_default(),
            files: request.files.unwrap_or(package),
            versions: request.versions.unwrap_or(false),
            comments: request.comments.unwrap_or(package),
            package,
            span: Span {
                from: from.map(moment),
                until: to.and_then(|date| date.succ_opt()).map(moment),
            },
        })
    }

    /// The extension of the files of conversations, chats and documents.
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self.format {
            ExportFormat::Markdown => ".md",
            ExportFormat::Json => ".json",
        }
    }
}

// ---------------------------------------------------------------- what the caller may take away

/// One thing an archive holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Thing {
    /// The thread, the chat or the document.
    pub id: Id,
    /// The node it belongs to: the thread, the chat's node, the node a document is written beside.
    pub node: Id,
    /// The node whose directory holds it, and where it is counted when it is left out: the topic of a thread,
    /// the node of a chat or a document.
    pub home: Id,
    /// Its name or its title.
    pub title: String,
    /// What it comes to, as an estimate from what is stored.
    pub bytes: i64,
    /// The files it carries, where files were asked for.
    pub files: FileFacts,
    /// When it was made.
    pub made: DateTime<Utc>,
}

/// What a person holds on the things of an archive: which they read, and which they may take away.
#[derive(Debug, Clone, Default)]
pub struct Access {
    /// Threads they read.
    pub reads_conversations: HashSet<Id>,
    /// Threads whose conversation they may take away.
    pub exports_conversations: HashSet<Id>,
    /// Nodes whose chat they read.
    pub reads_chats: HashSet<Id>,
    /// Nodes whose chat they may take away.
    pub exports_chats: HashSet<Id>,
    /// Documents they read.
    pub reads_documents: HashSet<Id>,
    /// Documents they may take away.
    pub exports_documents: HashSet<Id>,
}

async fn holding(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    name: PermissionName,
) -> Result<HashSet<Id>, ExportError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(
        nodes::holding(conn, principal, ids, name, Act::Use, Policy::default())
            .await?
            .into_iter()
            .map(|listed| listed.id)
            .collect(),
    )
}

impl Access {
    /// What `principal` holds on these threads, nodes (for their chats) and documents, asked together.
    pub async fn of(
        conn: &mut Conn,
        principal: &Principal,
        threads: &[Id],
        chat_nodes: &[Id],
        documents: &[Id],
    ) -> Result<Self, ExportError> {
        let view = PermissionName::OfNode(NodePermission::View);
        let mut access = Self {
            reads_conversations: holding(conn, principal, threads, view).await?,
            exports_conversations: holding(
                conn,
                principal,
                threads,
                PermissionName::Exact(Permission::ThreadExport),
            )
            .await?,
            reads_chats: holding(
                conn,
                principal,
                chat_nodes,
                PermissionName::Exact(Permission::ChatView),
            )
            .await?,
            exports_chats: holding(
                conn,
                principal,
                chat_nodes,
                PermissionName::Exact(Permission::ChatExport),
            )
            .await?,
            ..Self::default()
        };
        if !documents.is_empty() {
            for listed in
                document_access::listed(conn, principal, documents, Policy::default()).await?
            {
                if listed.document.deleted {
                    continue;
                }
                if listed.holds(Permission::DocView) {
                    access.reads_documents.insert(listed.document.id);
                    if listed.holds(Permission::DocExport) {
                        access.exports_documents.insert(listed.document.id);
                    }
                }
            }
        }
        Ok(access)
    }
}

/// Native database candidates and ordinary access, before aggregate construction or API rendering.
#[derive(Debug, Clone)]
pub struct ExportSelection {
    /// Actual requested place.
    pub place: PlaceRow,
    /// Native bounded traversal, including routing ancestors.
    pub scope: Vec<PlaceRow>,
    /// Native request-selected node ids.
    pub included: Vec<Id>,
    /// Actual requested conversation candidates.
    pub threads: Vec<Id>,
    /// Actual nonempty native node chats.
    pub chats: Vec<rows::ChatFacts>,
    /// Existing document candidates, including native size facts.
    pub documents: Vec<rows::DocumentFacts>,
    /// The original per-item ordinary reading/export questions.
    pub access: Access,
    /// Native conversation measurements, not public aggregates.
    pub conversation_facts: HashMap<Id, rows::ConversationFacts>,
    /// Names admitted by the native node reader.
    pub viewable: HashMap<Id, String>,
    /// The existing traversal bound was reached.
    pub cut_short: bool,
}

/// Trusted additional selection bounds; these add no native visibility or Export authority.
#[derive(Debug, Clone, Default)]
pub struct ExportBounds {
    /// Conversations admitted by the consuming operation.
    pub threads: HashSet<Id>,
    /// Native chats admitted by the consuming operation.
    pub chats: HashSet<Id>,
    /// Native documents admitted by the consuming operation.
    pub documents: HashSet<Id>,
    /// Native node records/labels admitted by the consuming operation.
    pub nodes: HashSet<Id>,
}
impl ExportSelection {
    /// Restrict raw candidates before counts, file accounting, blockers, layout or DTOs.
    pub fn restrict(&mut self, bounds: &ExportBounds, package: bool) {
        self.threads.retain(|id| bounds.threads.contains(id));
        self.chats.retain(|row| bounds.chats.contains(&row.chat_id));
        self.documents
            .retain(|row| bounds.documents.contains(&row.id));
        self.access
            .reads_conversations
            .retain(|id| bounds.threads.contains(id));
        self.access
            .exports_conversations
            .retain(|id| bounds.threads.contains(id));
        let chat_nodes = self
            .chats
            .iter()
            .map(|chat| chat.node_id)
            .collect::<HashSet<_>>();
        self.access.reads_chats.retain(|id| chat_nodes.contains(id));
        self.access
            .exports_chats
            .retain(|id| chat_nodes.contains(id));
        self.access
            .reads_documents
            .retain(|id| bounds.documents.contains(id));
        self.access
            .exports_documents
            .retain(|id| bounds.documents.contains(id));
        let mut needed = HashSet::from([self.place.id]);
        needed.extend(self.threads.iter().copied());
        needed.extend(self.chats.iter().map(|row| row.node_id));
        needed.extend(self.documents.iter().map(|row| row.node_id));
        if package {
            needed.extend(
                self.included
                    .iter()
                    .copied()
                    .filter(|id| bounds.nodes.contains(id)),
            );
        }
        let anchors = self
            .scope
            .iter()
            .filter(|node| needed.contains(&node.id))
            .flat_map(|node| node.path.iter().copied())
            .collect::<Vec<_>>();
        needed.extend(anchors);
        self.scope.retain(|node| needed.contains(&node.id));
        self.included.retain(|id| needed.contains(id));
        self.viewable
            .retain(|id, _| needed.contains(id) && bounds.nodes.contains(id));
    }
}

/// Prepared native plan operands, reused by ordinary requests and scoped source factories.
#[derive(Debug, Clone)]
pub struct PreparedExportPlan {
    /// Raw native selection; any extra bounds are applied here before aggregation.
    pub selection: ExportSelection,
    /// The native resolved request, never an operation label supplied by a client.
    pub wanted: Wanted,
    /// Current native place limits.
    pub limits: ExportLimits,
}

/// What was found for an archive: the things the caller may take away, and what they may not.
#[derive(Debug, Clone)]
pub struct Selected {
    /// The place the archive is of.
    pub place: PlaceRow,
    /// The nodes in scope, the place first, in the order they were made.
    pub scope: Vec<PlaceRow>,
    /// The places in scope that the caller may view, by name.
    pub viewable: HashMap<Id, String>,
    /// The places in scope that the archive is asked to hold, each with what is in it.
    pub included: HashSet<Id>,
    /// The conversations to write.
    pub conversations: Vec<Thing>,
    /// The chats to write.
    pub chats: Vec<Thing>,
    /// The documents to write.
    pub documents: Vec<Thing>,
    /// What the caller may read and not take away, and the files past a size, by the place they lie in.
    pub left_out: BTreeMap<(u8, Id), i64>,
    /// Exact native readable contributors to the not-exportable counts, selected before aggregation.
    pub count_sources: Vec<operation::Subject>,
    /// An estimate of the size of the archive.
    pub bytes: i64,
    /// The files, counted.
    pub files: i64,
    /// Whether the place holds more than a walk reaches.
    pub cut_short: bool,
}

/// The key of `Selected::left_out` for things that may be read and not taken away.
pub const NOT_EXPORTABLE: u8 = 0;
/// The key of `Selected::left_out` for files that are too large.
pub const TOO_LARGE: u8 = 1;

impl Selected {
    /// How many conversations, chats and documents there are to write.
    #[must_use]
    pub fn items(&self) -> i64 {
        i64::try_from(self.conversations.len() + self.chats.len() + self.documents.len())
            .unwrap_or(i64::MAX)
    }

    /// Where each thing is to be placed, in the forms asked for.
    #[must_use]
    pub fn layout(&self, wanted: &Wanted) -> Layout {
        let nodes: Vec<export_layout::Node> = self
            .scope
            .iter()
            .map(|node| export_layout::Node {
                id: node.id,
                parent: node.parent_id.filter(|_| node.id != self.place.id),
                name: self.viewable.get(&node.id).map(|name| {
                    if node.kind == "org" {
                        node.slug.clone().unwrap_or_else(|| name.clone())
                    } else {
                        name.clone()
                    }
                }),
            })
            .collect();
        let wanting = export_layout::Wanting {
            conversations: self
                .conversations
                .iter()
                .map(|thing| (thing.id, thing.home, thing.title.clone()))
                .collect(),
            chats: self
                .chats
                .iter()
                .map(|thing| (thing.id, thing.home))
                .collect(),
            documents: self
                .documents
                .iter()
                .map(|thing| (thing.id, thing.home, thing.title.clone()))
                .collect(),
        };
        export_layout::lay_out(self.place.id, &nodes, &wanting, wanted.extension())
    }

    /// The numbers an archive would hold.
    #[must_use]
    pub fn counts(&self, layout: &Layout) -> ExportCounts {
        ExportCounts {
            places: layout.places(),
            conversations: i64::try_from(self.conversations.len()).unwrap_or(i64::MAX),
            chats: i64::try_from(self.chats.len()).unwrap_or(i64::MAX),
            documents: i64::try_from(self.documents.len()).unwrap_or(i64::MAX),
            files: self.files,
        }
    }

    /// What was left out when the things were read, as the API tells it.
    #[must_use]
    pub fn left_out_view(&self) -> Vec<ExportLeftOut> {
        self.left_out_told(&self.left_out)
    }

    /// `left_out`, as the API tells it: by the place they lie in, with the name only where the caller may see
    /// the place.
    #[must_use]
    pub fn left_out_told(&self, left_out: &BTreeMap<(u8, Id), i64>) -> Vec<ExportLeftOut> {
        let kinds: HashMap<Id, &str> = self
            .scope
            .iter()
            .map(|node| (node.id, node.kind.as_str()))
            .collect();
        left_out
            .iter()
            .map(|((what, place), count)| ExportLeftOut {
                what: if *what == TOO_LARGE {
                    ExportLeftOutWhat::TooLarge
                } else {
                    ExportLeftOutWhat::NotExportable
                },
                count: *count,
                place: Some(NodeRef {
                    thread_display: None,
                    id: *place,
                    kind: node_kind(kinds.get(place).copied().unwrap_or("org")),
                    name: self.viewable.get(place).cloned(),
                    redacted: !self.viewable.contains_key(place),
                }),
            })
            .collect()
    }
}

/// The kind of node a word names.
#[must_use]
pub fn node_kind(word: &str) -> NodeKind {
    match word {
        "project" => NodeKind::Project,
        "topic" => NodeKind::Topic,
        "thread" => NodeKind::Thread,
        _ => NodeKind::Org,
    }
}

impl Engine {
    /// Finds what `principal` may take away of what lies in the place `place`, as `wanted` says, at this moment.
    pub async fn select_export(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        place: Id,
        wanted: &Wanted,
    ) -> Result<Selected, ExportError> {
        let selection = self
            .prepare_export_selection(conn, principal, place, wanted)
            .await?;
        self.finish_export_selection(conn, &selection, wanted).await
    }

    /// Select actual native rows and ordinary authority, without computing the plan's aggregates.
    pub async fn prepare_export_selection(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        place: Id,
        wanted: &Wanted,
    ) -> Result<ExportSelection, ExportError> {
        let root = places::places(conn, &[place])
            .await?
            .into_iter()
            .next()
            .ok_or(ExportError::NotFound)?;
        let below = places::below(conn, place, NODES_MAX).await?;
        let cut_short = i64::try_from(below.len()).unwrap_or(i64::MAX) >= NODES_MAX;
        let mut scope = vec![root.clone()];
        scope.extend(below);
        let in_scope: HashSet<Id> = scope.iter().map(|node| node.id).collect();
        // The nodes that are in it, each with what is in it.
        let chosen: HashSet<Id> = wanted
            .nodes
            .iter()
            .copied()
            .filter(|node| in_scope.contains(node))
            .collect();
        let included: Vec<&PlaceRow> = scope
            .iter()
            .filter(|node| {
                wanted.everything || node.path.iter().any(|ancestor| chosen.contains(ancestor))
            })
            .collect();
        let included_ids: Vec<Id> = included.iter().map(|node| node.id).collect();
        // What exists: the threads, the chats that say something, the documents.
        let thread_ids: Vec<Id> = if wanted.conversations_on {
            included
                .iter()
                .filter(|node| node.kind == "thread")
                .map(|node| node.id)
                .collect()
        } else {
            Vec::new()
        };
        let mut chat_facts = Vec::new();
        if wanted.chats_on {
            chat_facts.extend(rows::chat_facts(conn, &included_ids, wanted.span).await?);
            let chosen_chats: Vec<Id> = wanted
                .chats
                .iter()
                .copied()
                .filter(|chat| !chat_facts.iter().any(|facts| facts.chat_id == *chat))
                .collect();
            if !chosen_chats.is_empty() {
                chat_facts.extend(
                    rows::chat_facts_of(conn, &chosen_chats, wanted.span)
                        .await?
                        .into_iter()
                        .filter(|facts| in_scope.contains(&facts.node_id)),
                );
            }
        }
        chat_facts.retain(|facts| facts.messages > 0);
        let mut document_ids: Vec<Id> = if wanted.documents_on {
            places::documents_beside(conn, &included_ids).await?
        } else {
            Vec::new()
        };
        if wanted.documents_on {
            for document in &wanted.documents {
                if !document_ids.contains(document) {
                    document_ids.push(*document);
                }
            }
        }
        let document_facts: Vec<_> = rows::document_facts(conn, &document_ids)
            .await?
            .into_iter()
            .filter(|facts| in_scope.contains(&facts.node_id))
            .collect();
        let document_ids: Vec<Id> = document_facts.iter().map(|facts| facts.id).collect();

        // What the caller holds on them.
        let chat_nodes: Vec<Id> = chat_facts.iter().map(|facts| facts.node_id).collect();
        let access = Access::of(conn, principal, &thread_ids, &chat_nodes, &document_ids).await?;
        let conversation_facts = rows::conversation_facts(conn, &thread_ids, wanted.span).await?;

        // The places the caller may name.
        let scope_ids: Vec<Id> = scope.iter().map(|node| node.id).collect();
        let viewable: HashMap<Id, String> = nodes::holding(
            conn,
            principal,
            &scope_ids,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await?
        .into_iter()
        .map(|listed| (listed.id, listed.name))
        .collect();
        Ok(ExportSelection {
            place: root,
            scope,
            included: included_ids,
            threads: thread_ids,
            chats: chat_facts,
            documents: document_facts,
            access,
            conversation_facts,
            viewable,
            cut_short,
        })
    }

    /// Aggregate and lay out the same native operands, after any trusted source restriction.
    pub async fn finish_export_selection(
        &self,
        conn: &mut Conn,
        selection: &ExportSelection,
        wanted: &Wanted,
    ) -> Result<Selected, ExportError> {
        let root = selection.place.clone();
        let place = root.id;
        let scope = selection.scope.clone();
        let included_ids = selection.included.clone();
        let included: Vec<&PlaceRow> = scope
            .iter()
            .filter(|node| included_ids.contains(&node.id))
            .collect();
        let name_of_node: HashMap<Id, &str> = scope
            .iter()
            .map(|node| (node.id, node.name.as_str()))
            .collect();
        let thread_ids = &selection.threads;
        let chat_facts = &selection.chats;
        let document_facts = &selection.documents;
        let access = &selection.access;
        let conversation_facts = &selection.conversation_facts;
        let viewable = selection.viewable.clone();
        let cut_short = selection.cut_short;
        let mut left_out: BTreeMap<(u8, Id), i64> = BTreeMap::new();
        let mut count_sources = Vec::new();
        let mut conversations: Vec<Thing> = Vec::new();
        for node in included.iter().filter(|node| node.kind == "thread") {
            if !thread_ids.contains(&node.id) || !access.reads_conversations.contains(&node.id) {
                continue;
            }
            let home = node
                .parent_id
                .filter(|_| node.id != place)
                .unwrap_or(node.id);
            if !access.exports_conversations.contains(&node.id) {
                *left_out.entry((NOT_EXPORTABLE, home)).or_default() += 1;
                count_sources.push(operation::Subject::ThreadCount(node.id));
                continue;
            }
            conversations.push(Thing {
                id: node.id,
                node: node.id,
                home,
                title: node.name.clone(),
                bytes: conversation_facts
                    .get(&node.id)
                    .map_or(0, |facts| facts.bytes)
                    + HEAD_BYTES
                    + ENTRY_OVERHEAD_BYTES,
                files: FileFacts::default(),
                made: node.created_at,
            });
        }
        let mut chats: Vec<Thing> = Vec::new();
        for facts in chat_facts {
            if !access.reads_chats.contains(&facts.node_id) {
                continue;
            }
            if !access.exports_chats.contains(&facts.node_id) {
                *left_out.entry((NOT_EXPORTABLE, facts.node_id)).or_default() += 1;
                count_sources.push(operation::Subject::ChatCount(facts.chat_id));
                continue;
            }
            chats.push(Thing {
                id: facts.chat_id,
                node: facts.node_id,
                home: facts.node_id,
                title: name_of_node
                    .get(&facts.node_id)
                    .map_or_else(String::new, |name| (*name).to_owned()),
                bytes: facts.bytes + HEAD_BYTES + ENTRY_OVERHEAD_BYTES,
                files: FileFacts::default(),
                made: DateTime::UNIX_EPOCH,
            });
        }
        let mut documents: Vec<Thing> = Vec::new();
        for facts in document_facts {
            if !access.reads_documents.contains(&facts.id) {
                continue;
            }
            if !access.exports_documents.contains(&facts.id) {
                *left_out.entry((NOT_EXPORTABLE, facts.node_id)).or_default() += 1;
                count_sources.push(operation::Subject::DocumentCount(facts.id));
                continue;
            }
            let mut bytes = facts.bytes + HEAD_BYTES + ENTRY_OVERHEAD_BYTES;
            if wanted.versions {
                bytes += facts.version_bytes + facts.versions * ENTRY_OVERHEAD_BYTES;
            }
            if wanted.comments {
                bytes += facts.comment_bytes;
            }
            documents.push(Thing {
                id: facts.id,
                node: facts.node_id,
                home: facts.node_id,
                title: facts.title.clone(),
                bytes,
                files: FileFacts::default(),
                made: facts.created_at,
            });
        }
        // The files they carry.
        let most = self.inner.config.export_file_max_bytes;
        let mut files = 0_i64;
        if wanted.files {
            let threads: Vec<Id> = conversations.iter().map(|thing| thing.id).collect();
            let found = rows::input_files(conn, &threads, most, wanted.span).await?;
            for thing in &mut conversations {
                thing.files = found.get(&thing.id).copied().unwrap_or_default();
            }
            let ids: Vec<Id> = chats.iter().map(|thing| thing.id).collect();
            let found = rows::chat_files(conn, &ids, most).await?;
            for thing in &mut chats {
                thing.files = found.get(&thing.id).copied().unwrap_or_default();
            }
            let ids: Vec<Id> = documents.iter().map(|thing| thing.id).collect();
            let found = rows::document_file_facts(conn, &ids, most).await?;
            for thing in &mut documents {
                thing.files = found.get(&thing.id).copied().unwrap_or_default();
            }
            for thing in conversations
                .iter_mut()
                .chain(chats.iter_mut())
                .chain(documents.iter_mut())
            {
                files += thing.files.files;
                thing.bytes += thing.files.bytes + thing.files.files * ENTRY_OVERHEAD_BYTES;
                if thing.files.too_large > 0 {
                    *left_out.entry((TOO_LARGE, thing.home)).or_default() += thing.files.too_large;
                }
            }
        }
        let items =
            i64::try_from(conversations.len() + chats.len() + documents.len()).unwrap_or(i64::MAX);
        let mut bytes = conversations
            .iter()
            .chain(&chats)
            .chain(&documents)
            .map(|thing| thing.bytes)
            .sum::<i64>()
            + items * INDEX_LINE_BYTES;
        if wanted.package {
            // A package adds a Markdown reading copy and a manifest; documents also have an HTML page.
            // Carried files are shared by both reading copies, rather than counted or embedded twice.
            let text: i64 = conversations
                .iter()
                .chain(&chats)
                .chain(&documents)
                .map(|thing| (thing.bytes - thing.files.bytes).max(0))
                .sum();
            let objects = items + i64::try_from(scope.len()).unwrap_or(i64::MAX);
            let html: i64 = documents
                .iter()
                .map(|thing| {
                    (thing.bytes - thing.files.bytes).max(0)
                        + i64::try_from(orch_documents::html::PAGE_OVERHEAD_BYTES)
                            .unwrap_or(i64::MAX)
                        + ENTRY_OVERHEAD_BYTES
                })
                .sum();
            bytes += text + html + objects * PACKAGE_OBJECT_BYTES;
        }

        let included: HashSet<Id> = included_ids.iter().copied().collect();
        Ok(Selected {
            place: root,
            scope,
            viewable,
            included,
            conversations,
            chats,
            documents,
            left_out,
            count_sources,
            bytes,
            files,
            cut_short,
        })
    }

    /// What the place allows of one archive made of it: the largest, the most things, and how long it is kept.
    pub async fn export_limits(
        &self,
        conn: &mut Conn,
        place: Id,
    ) -> Result<ExportLimits, ExportError> {
        let max_bytes = orch_settings::get::<i64>(conn, Place::Node(place), keys::EXPORT_MAX_BYTES)
            .await?
            .unwrap_or(DEFAULT_MAX_BYTES);
        let max_items = orch_settings::get::<i64>(conn, Place::Node(place), keys::EXPORT_MAX_ITEMS)
            .await?
            .unwrap_or(DEFAULT_MAX_ITEMS);
        let keep_hours = orch_settings::get::<i64>(conn, Place::System, keys::EXPORT_KEEP_HOURS)
            .await?
            .unwrap_or(DEFAULT_KEEP_HOURS);
        Ok(ExportLimits {
            max_bytes,
            max_items,
            keep_hours,
        })
    }
}

// ---------------------------------------------------------------- the plan

/// What stands in the way of an archive as it is asked for, in the order the refusal names it.
#[must_use]
pub fn blockers_of(selected: &Selected, limits: &ExportLimits) -> Vec<(Blocked, i64, i64, String)> {
    let mut out = Vec::new();
    let items = selected.items();
    if items == 0 {
        out.push((
            Blocked::Nothing,
            0,
            0,
            "there is nothing in it that you may take away as a file".to_owned(),
        ));
        return out;
    }
    if items > limits.max_items || selected.cut_short {
        out.push((
            Blocked::TooMany,
            limits.max_items,
            items,
            format!(
                "it would hold {items} things, and the place allows {}",
                limits.max_items
            ),
        ));
    }
    if selected.bytes > limits.max_bytes {
        out.push((
            Blocked::TooLarge,
            limits.max_bytes,
            selected.bytes,
            format!(
                "it would come to about {} bytes, and the place allows {}",
                selected.bytes, limits.max_bytes
            ),
        ));
    }
    out
}

/// A reference to the place an archive is of, with its name: its person sees it, or could not have asked.
fn node_ref(selected: &Selected) -> NodeRef {
    NodeRef {
        thread_display: None,
        id: selected.place.id,
        kind: node_kind(&selected.place.kind),
        name: Some(selected.place.name.clone()),
        redacted: false,
    }
}

impl Engine {
    /// Whether the caller may view the place an archive is asked of; it is not there for them otherwise.
    pub async fn export_place_seen(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        place: Id,
    ) -> Result<(), ExportError> {
        match nodes::check_named(
            conn,
            principal,
            place,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => Ok(()),
            Ok(orch_access_contract::decisions::Decision::Refused(_)) => Err(ExportError::NotFound),
            Err(orch_authz::Error::Archived { .. }) => Ok(()),
            Err(other) => Err(other.into()),
        }
    }

    /// What an archive of `place` would hold as far as `caller` may read it, and what stands in its way. Nothing
    /// is made.
    pub async fn plan_export(
        &self,
        caller: &Caller,
        place: Id,
        request: &ExportRequest,
    ) -> Result<ExportPlan, ExportError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
        let prepared = self
            .prepare_export_plan(&mut conn, &principal, place, request)
            .await?;
        self.render_export_plan(&mut conn, &prepared).await
    }

    /// Prepare on the caller's guarded connection using the same native source and ordinary gate.
    pub async fn prepare_export_plan(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        place: Id,
        request: &ExportRequest,
    ) -> Result<PreparedExportPlan, ExportError> {
        let wanted = Wanted::read(request)?;
        self.export_place_seen(conn, principal, place).await?;
        let selection = self
            .prepare_export_selection(conn, principal, place, &wanted)
            .await?;
        let limits = self.export_limits(conn, place).await?;
        Ok(PreparedExportPlan {
            selection,
            wanted,
            limits,
        })
    }

    /// The existing plan formatter; no source selection or external IO is repeated here.
    pub async fn render_export_plan(
        &self,
        conn: &mut Conn,
        prepared: &PreparedExportPlan,
    ) -> Result<ExportPlan, ExportError> {
        let selected = self
            .finish_export_selection(conn, &prepared.selection, &prepared.wanted)
            .await?;
        Ok(plan_of(&selected, &prepared.wanted, prepared.limits))
    }
}

fn plan_of(selected: &Selected, wanted: &Wanted, limits: ExportLimits) -> ExportPlan {
    let layout = selected.layout(wanted);
    ExportPlan {
        node: node_ref(selected),
        counts: selected.counts(&layout),
        bytes: selected.bytes,
        limits,
        left_out: selected.left_out_view(),
        blockers: blockers_of(selected, &limits)
            .into_iter()
            .map(
                |(by, _, _, message)| orch_api_contract::exports::ExportBlocker {
                    code: by.code().to_owned(),
                    message,
                },
            )
            .collect(),
    }
}

// ---------------------------------------------------------------- making one

/// An archive asked for, with the job that writes it.
#[derive(Debug, Clone)]
pub struct Made {
    /// The archive.
    pub export: ExportRow,
    /// The job.
    pub job: JobRow,
}

/// Immutable native creation operands selected on the writer's actual guarded connection.
#[derive(Debug, Clone)]
pub struct PreparedExportCreation {
    selected: Selected,
    request: ExportRequest,
    user: Id,
}

/// What a caller was reading with, kept with the job so that it reads the same way when it writes: a browser
/// session reads as its person; a token reads within its scopes and the subtree it is kept to.
fn credential_of(caller: &Caller) -> Json {
    match caller.grant() {
        None => Json::Null,
        Some(grant) => {
            json!({ "scopes": grant.scopes().to_strings(), "subtree": grant.subtree(), "operation_scoped":grant.operation_scope().is_some() })
        }
    }
}

impl Engine {
    /// Asks for an archive of `place` for the caller: the row, and the job that writes it. Refused when the
    /// caller may take nothing away of what they asked for, or the archive would pass what the place allows, or
    /// they have as many being written as they may.
    pub async fn make_export(
        &self,
        caller: &Caller,
        origin: &Origin,
        place: Id,
        request: &ExportRequest,
    ) -> Result<Made, ExportError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        rows::lock_person(&mut tx, principal.user()).await?;
        let plan = self
            .prepare_export_plan(&mut tx, &principal, place, request)
            .await?;
        let prepared = self
            .prepare_export_creation(&mut tx, &principal, &plan, request)
            .await?;
        let made = self
            .apply_export_creation(&mut tx, &principal, caller, origin, &prepared, || {
                Ok::<(), EngineError>(())
            })
            .await?;
        if caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
        {
            operation::Request::retain(
                &mut tx,
                &made,
                caller,
                crate::operation_scope::request_origin::Credential::Native,
                caller.grant().and_then(orch_authz::TokenGrant::expires_at),
                Time::new(now),
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        self.kick();
        Ok(made)
    }

    /// Validate the native request's source-bounded selection and the existing per-person queue limit.
    /// The caller holds lock_person on this same transaction before selecting these operands.
    pub async fn prepare_export_creation(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        plan: &PreparedExportPlan,
        request: &ExportRequest,
    ) -> Result<PreparedExportCreation, ExportError> {
        self.export_place_seen(conn, principal, plan.selection.place.id)
            .await?;
        let selected = self
            .finish_export_selection(conn, &plan.selection, &plan.wanted)
            .await?;
        if let Some((by, limit, asked, message)) =
            blockers_of(&selected, &plan.limits).into_iter().next()
        {
            return Err(ExportError::Blocked {
                by,
                limit,
                asked,
                message,
            });
        }
        let writing = rows::writing(conn, principal.user()).await?;
        if writing >= i64::from(self.inner.config.exports_per_user) {
            return Err(ExportError::TooManyAtOnce {
                limit: self.inner.config.exports_per_user,
            });
        }
        Ok(PreparedExportCreation {
            selected,
            request: request.clone(),
            user: principal.user(),
        })
    }

    /// The original native archive/job writer, with an opaque check at its actual first insert.
    pub async fn apply_export_creation<E: Into<EngineError>>(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        caller: &Caller,
        origin: &Origin,
        prepared: &PreparedExportCreation,
        before: impl FnOnce() -> Result<(), E>,
    ) -> Result<Made, ExportError> {
        if principal.user() != prepared.user || caller.user() != prepared.user {
            return Err(ExportError::NotFound);
        }
        let user = prepared.user;
        let selected = &prepared.selected;
        let place = selected.place.id;
        let request = &prepared.request;
        let now = self.inner.clock.now();
        let id = new_id();
        let stored = serde_json::to_value(request).map_err(EngineError::from)?;
        before().map_err(|error| ExportError::Internal(error.into()))?;
        let export = rows::insert(conn, id, user, place, &stored, now).await?;
        let progress =
            serde_json::to_value(initial_progress(&the_job::PHASES)).map_err(EngineError::from)?;
        let plan = json!({
            "export": id,
            "user": user,
            "node": place,
            "credential": credential_of(caller),
            // Whether the people a package names are named with their address: only for a system administrator
            // in the mode that is theirs to use, as the admin screens are, and for the person who asks, of themself.
            "emails": principal.is_system_admin(),
        });
        let job = job_rows::insert(
            &mut *conn,
            &NewJob {
                id: new_id(),
                kind: kind::EXPORT,
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &plan,
                progress: &progress,
                created_by: Some(user),
            },
            now,
        )
        .await?;
        rows::attach_job(&mut *conn, id, job.id).await?;
        let export = ExportRow {
            job_id: Some(job.id),
            ..export
        };
        let mut audit = AuditEntry::new(
            Actor::User(user),
            orch_db::audit::action!(AuditAction::ExportCreated).as_str(),
            "export",
            Some(id),
            "An archive of what a person may read in a place was asked for.",
        );
        audit.node_id = Some(place);
        audit.org_id = Some(selected.place.org_id);
        audit.details = json!({
            "export_id": id,
            "place": { "id": place, "name": selected.place.name },
            "options": stored,
            "job_id": job.id,
        });
        append(&mut *conn, &audit, origin, now).await?;
        Ok(Made { export, job })
    }
}

// ---------------------------------------------------------------- reading them

fn counts_of(raw: &Json) -> Option<ExportCounts> {
    serde_json::from_value(raw.clone()).ok()
}

fn left_out_of(raw: &Json) -> Vec<ExportLeftOut> {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

fn state_of(word: &str) -> ExportState {
    match word {
        rows_state::READY => ExportState::Ready,
        rows_state::FAILED => ExportState::Failed,
        rows_state::EXPIRED => ExportState::Expired,
        _ => ExportState::Pending,
    }
}

use orch_db::queries::exports::state as rows_state;

/// Opaque native archive metadata and the exact current name sources used by its formatter.
pub struct PreparedExportViews {
    rows: Vec<ExportRow>,
    kinds: HashMap<Id, String>,
    viewable: HashMap<Id, String>,
    inspections: HashMap<Id, orch_api_contract::access::AdminAccess>,
    manifests: HashMap<Id, Json>,
}
impl PreparedExportViews {
    /// Actual stored archives selected by this producer.
    pub fn rows(&self) -> &[ExportRow] {
        &self.rows
    }
    /// Only the native place labels this preparation will disclose.
    pub fn named_places(&self) -> BTreeSet<Id> {
        self.viewable.keys().copied().collect()
    }
    /// Restrict the original native formatter to actual admitted name sources.
    pub fn bound_names(&mut self, allowed: &BTreeSet<Id>) {
        self.viewable.retain(|id, _| allowed.contains(id));
    }
    /// The unchanged native archive wire shape, from these prepared stored rows.
    pub fn render(&self) -> Result<Vec<Export>, ExportError> {
        let mut out = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let request: ExportRequest =
                serde_json::from_value(row.request.clone()).map_err(EngineError::from)?;
            let ready = row.state == rows_state::READY;
            out.push(Export {
                admin_access: self.inspections.get(&row.id).cloned(),
                manifest: self.manifests.get(&row.id).cloned(),
                id: row.id,
                node: row.node_id.map(|node| NodeRef {
                    thread_display: None,
                    id: node,
                    kind: node_kind(self.kinds.get(&node).map_or("org", String::as_str)),
                    name: self.viewable.get(&node).cloned(),
                    redacted: !self.viewable.contains_key(&node),
                }),
                request,
                state: state_of(&row.state),
                job_id: row.job_id,
                bytes: row.bytes,
                counts: if ready { counts_of(&row.counts) } else { None },
                left_out: left_out_of(&row.left_out),
                created_at: Time::new(row.created_at),
                ready_at: row.ready_at.map(Time::new),
                expires_at: row.expires_at.map(Time::new),
                downloaded_at: row.downloaded_at.map(Time::new),
            });
        }
        Ok(out)
    }
}

impl Engine {
    /// The same native rows and current place-name eligibility, before formatting.
    pub async fn prepare_export_views(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        list: Vec<ExportRow>,
    ) -> Result<PreparedExportViews, ExportError> {
        let ids: Vec<Id> = list.iter().filter_map(|row| row.node_id).collect();
        let found = if ids.is_empty() {
            Vec::new()
        } else {
            places::places(conn, &ids).await?
        };
        let viewable: HashMap<Id, String> = if ids.is_empty() {
            HashMap::new()
        } else {
            nodes::holding(
                conn,
                principal,
                &ids,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await?
            .into_iter()
            .map(|listed| (listed.id, listed.name))
            .collect()
        };
        let kinds: HashMap<Id, String> =
            found.into_iter().map(|node| (node.id, node.kind)).collect();
        Ok(PreparedExportViews {
            manifests: rows::manifests(conn, &list.iter().map(|row| row.id).collect::<Vec<_>>())
                .await?
                .into_iter()
                .collect(),
            inspections: list
                .iter()
                .filter_map(|row| {
                    crate::archive::custody_inspection(principal, row.user_id)
                        .map(|reach| (row.id, reach))
                })
                .collect(),
            rows: list,
            kinds,
            viewable,
        })
    }

    /// Archives keep their original full native view for ordinary callers.
    pub async fn export_views(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        list: &[ExportRow],
    ) -> Result<Vec<Export>, ExportError> {
        self.prepare_export_views(conn, principal, list.to_vec())
            .await?
            .render()
    }

    /// An archive of `user`, if it is theirs: another person's is none for them.
    pub async fn export_of(
        &self,
        conn: &mut Conn,
        user: Id,
        id: Id,
    ) -> Result<ExportRow, ExportError> {
        rows::get(conn, id)
            .await?
            .filter(|row| row.user_id == Some(user))
            .ok_or(ExportError::NotFound)
    }

    /// The actual creator's custody, or a current administrative browser inspection.
    pub async fn read_export(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        id: Id,
    ) -> Result<ExportRow, ExportError> {
        rows::get(conn, id)
            .await?
            .filter(|row| crate::archive::may_read_custody(principal, row.user_id))
            .ok_or(ExportError::NotFound)
    }

    /// The archives of `user`, newest first: a page after the cursor of the last one shown.
    pub async fn exports_of(
        &self,
        conn: &mut Conn,
        user: Id,
        after: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<ExportRow>, Option<String>), ExportError> {
        let after = match after {
            None => None,
            Some(cursor) => {
                Some(read_cursor(cursor).ok_or_else(|| invalid("cursor", "is not one"))?)
            }
        };
        let mut list = rows::of_user(conn, user, after, limit + 1).await?;
        let more = i64::try_from(list.len()).unwrap_or(i64::MAX) > limit;
        if more {
            list.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        let next = if more {
            list.last().map(metadata_cursor)
        } else {
            None
        };
        Ok((list, next))
    }
}

/// The existing opaque native metadata cursor for a selected archive row.
#[must_use]
pub fn metadata_cursor(row: &ExportRow) -> String {
    write_cursor(row.created_at, row.id)
}

fn write_cursor(at: DateTime<Utc>, id: Id) -> String {
    format!("{}:{}", at.timestamp_micros(), id)
}

fn read_cursor(text: &str) -> Option<(DateTime<Utc>, Id)> {
    let (micros, id) = text.split_once(':')?;
    Some((
        DateTime::from_timestamp_micros(micros.parse().ok()?)?,
        id.parse().ok()?,
    ))
}

// ---------------------------------------------------------------- removing one, and fetching one

/// The native locked archive and its complete fixed cancellation/pin operands.
pub struct PreparedExportRemoval {
    row: ExportRow,
}
impl PreparedExportRemoval {
    /// The actual owner-checked stored artifact, not a path label or wire projection.
    pub fn row(&self) -> &ExportRow {
        &self.row
    }
}

/// Exact physical cleanup accepted by the committed native archive removal.
pub struct ExportRemoval {
    forgotten: Option<orch_types::Hash>,
}

/// Native archive-open operands, held on their caller's real transaction.
#[derive(Clone)]
pub struct PreparedExportOpen {
    row: ExportRow,
    hash: orch_types::Hash,
}
impl PreparedExportOpen {
    /// Actual creator-owned ready artifact, before its native store is opened.
    pub fn row(&self) -> &ExportRow {
        &self.row
    }
    /// The exact content-addressed archive selected from that stored row.
    pub fn hash(&self) -> orch_types::Hash {
        self.hash
    }

    /// The same immutable native archive, ignoring its once-only access bookkeeping.
    pub fn preserves(&self, fresh: &Self) -> bool {
        let old = &self.row;
        let now = &fresh.row;
        self.hash == fresh.hash
            && old.id == now.id
            && old.user_id == now.user_id
            && old.node_id == now.node_id
            && old.request == now.request
            && old.job_id == now.job_id
            && old.created_at == now.created_at
            && old.ready_at == now.ready_at
            && old.bytes == now.bytes
    }
}

/// The native ready-state/custody/hash predicate, from an actual selected stored row.
pub fn prepare_export_open_row(
    row: Option<ExportRow>,
    user: Id,
) -> Result<PreparedExportOpen, ExportError> {
    let row = row
        .filter(|row| row.user_id == Some(user))
        .ok_or(ExportError::NotFound)?;
    ready_export(row)
}

/// The same ready artifact, checked against the actual reader without impersonating its maker.
pub fn prepare_export_open_for_row(
    row: Option<ExportRow>,
    principal: &Principal,
) -> Result<PreparedExportOpen, ExportError> {
    let row = row
        .filter(|row| crate::archive::may_read_custody(principal, row.user_id))
        .ok_or(ExportError::NotFound)?;
    ready_export(row)
}

fn ready_export(row: ExportRow) -> Result<PreparedExportOpen, ExportError> {
    match row.state.as_str() {
        rows_state::READY => {}
        rows_state::PENDING => return Err(ExportError::NotFound),
        _ => return Err(ExportError::Gone),
    }
    let hash = row
        .blob
        .as_deref()
        .and_then(|text| text.parse::<orch_types::Hash>().ok())
        .ok_or(ExportError::Gone)?;
    Ok(PreparedExportOpen { row, hash })
}

/// The native archive filename from its selected optional place name and stored ready day.
pub fn archive_file_name(row: &ExportRow, name: Option<&str>) -> String {
    let name = name.unwrap_or("export");
    let day = row.ready_at.unwrap_or(row.created_at).format("%Y-%m-%d");
    format!("{}-{day}.zip", export_layout::clean(name))
}

impl Engine {
    /// Removes an archive of `user` now: a job still writing it is asked to stop, the blob it was kept in is let
    /// go and removed from the store when nothing else holds it. An archive removed already is none.
    pub async fn remove_export(&self, user: Id, id: Id) -> Result<(), ExportError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let prepared = self.prepare_export_removal(&mut tx, user, id).await?;
        let removed = self.apply_export_removal(&mut tx, &prepared, now).await?;
        tx.commit().await.map_err(DbError::from)?;
        self.finish_export_removal(removed).await;
        Ok(())
    }

    /// Effect-free native removal preparation on the actual action transaction.
    pub async fn prepare_export_removal(
        &self,
        conn: &mut Conn,
        user: Id,
        id: Id,
    ) -> Result<PreparedExportRemoval, ExportError> {
        let row = rows::lock(conn, id)
            .await?
            .filter(|row| row.user_id == Some(user))
            .ok_or(ExportError::NotFound)?;
        if row.state == rows_state::EXPIRED {
            return Err(ExportError::NotFound);
        }
        if let Some(job) = row.job_id {
            job_rows::lock(conn, job).await?;
        }
        if let Some(hash) = row.blob.as_ref().and_then(|value| value.parse().ok()) {
            orch_db::queries::storage::prepare_unpin_blobs(conn, &[hash]).await?;
        }
        Ok(PreparedExportRemoval { row })
    }

    /// First SQL is the native expiry mutation over the same transaction's locked operands.
    /// The original cancellation and pin release stay one rollbackable native act.
    pub async fn apply_export_removal(
        &self,
        conn: &mut Conn,
        prepared: &PreparedExportRemoval,
        now: DateTime<Utc>,
    ) -> Result<ExportRemoval, ExportError> {
        let row = &prepared.row;
        let job = row.job_id;
        let user = row.user_id.ok_or(ExportError::NotFound)?;
        rows::expire_locked(conn, row.id).await?;
        if let Some(job) = job {
            job_rows::request_cancel(conn, job, user, now).await?;
        }
        let hash = row
            .blob
            .as_ref()
            .and_then(|text| text.parse::<orch_types::Hash>().ok());
        let mut forgotten = None;
        if let Some(hash) = hash {
            orch_db::queries::storage::unpin_blobs(conn, &[hash], now).await?;
            if orch_db::queries::storage::forget_blob(conn, &hash, now + chrono::Duration::days(1))
                .await?
            {
                forgotten = Some(hash);
            }
        }
        Ok(ExportRemoval { forgotten })
    }

    /// Finish only this committed removal's exact native file cleanup and wakeup.
    pub async fn finish_export_removal(&self, removed: ExportRemoval) {
        if let (Some(hash), Some(store)) = (removed.forgotten, self.blobs())
            && let Err(error) = store.delete(&hash).await
        {
            tracing::warn!(%error, "an archive that was removed could not be taken out of the store");
        }
        self.kick();
    }

    /// Opens an archive for its person to fetch: records the first time it is, and says so in the audit trail.
    pub async fn open_export(
        &self,
        origin: &Origin,
        user: Id,
        id: Id,
    ) -> Result<(ExportRow, BlobRead), ExportError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let prepared = self.prepare_export_open(&mut tx, user, id).await?;
        let read = self.read_prepared_export(&prepared).await?;
        self.record_export_open(&mut tx, &prepared, origin, now)
            .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok((prepared.row, read))
    }

    /// Native effect-free open preparation, retaining the original row serialization and custody.
    pub async fn prepare_export_open(
        &self,
        conn: &mut Conn,
        user: Id,
        id: Id,
    ) -> Result<PreparedExportOpen, ExportError> {
        prepare_export_open_row(rows::lock(conn, id).await?, user)
    }

    /// Retain the actual administrative reader and the same locked native archive.
    pub async fn prepare_export_open_for(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        id: Id,
    ) -> Result<PreparedExportOpen, ExportError> {
        prepare_export_open_for_row(rows::lock(conn, id).await?, principal)
    }

    /// Open exactly the selected native blob once, outside any retryable source preparation.
    pub async fn read_prepared_export(
        &self,
        prepared: &PreparedExportOpen,
    ) -> Result<BlobRead, ExportError> {
        let Some(store) = self.blobs() else {
            return Err(ExportError::Internal(EngineError::unreadable(
                "no store of content is attached",
            )));
        };
        let read = match store.get(&prepared.hash, None).await {
            Ok(read) => read,
            Err(orch_blob_contract::store::BlobError::NotFound(_)) => {
                return Err(ExportError::Gone);
            }
            Err(error) => return Err(ExportError::Internal(EngineError::unreadable(error))),
        };
        Ok(read)
    }

    /// Record the original once-only first-download marker and audit on the open transaction.
    /// The first SQL is the actual native marker update; no store IO happens here.
    pub async fn record_export_open(
        &self,
        conn: &mut Conn,
        prepared: &PreparedExportOpen,
        origin: &Origin,
        now: DateTime<Utc>,
    ) -> Result<bool, ExportError> {
        let user = prepared.row.user_id.ok_or(ExportError::NotFound)?;
        self.record_export_open_by(conn, prepared, origin, now, user)
            .await
    }

    /// Download bookkeeping names the actual admitted reader, including an administrative opening.
    pub async fn record_export_open_for(
        &self,
        conn: &mut Conn,
        prepared: &PreparedExportOpen,
        principal: &Principal,
        origin: &Origin,
        now: DateTime<Utc>,
    ) -> Result<bool, ExportError> {
        if !crate::archive::may_read_custody(principal, prepared.row.user_id) {
            return Err(ExportError::NotFound);
        }
        self.record_export_open_by(conn, prepared, origin, now, principal.user())
            .await
    }

    async fn record_export_open_by(
        &self,
        conn: &mut Conn,
        prepared: &PreparedExportOpen,
        origin: &Origin,
        now: DateTime<Utc>,
        user: Id,
    ) -> Result<bool, ExportError> {
        let row = &prepared.row;
        let changed = rows::first_download(conn, row.id, now).await?;
        if changed {
            let mut audit = AuditEntry::new(
                Actor::User(user),
                orch_db::audit::action!(AuditAction::ExportDownloaded).as_str(),
                "export",
                Some(row.id),
                "An archive was fetched for the first time.",
            );
            audit.node_id = row.node_id;
            audit.details = json!({ "export_id": row.id, "bytes": row.bytes });
            append(conn, &audit, origin, now).await?;
        }
        Ok(changed)
    }

    /// The name an archive is fetched under: for the place and the day.
    pub async fn export_file_name(
        &self,
        conn: &mut Conn,
        row: &ExportRow,
    ) -> Result<String, ExportError> {
        let named = match row.node_id {
            Some(node) => orch_db::queries::threads::node_name(conn, node).await?,
            None => None,
        };
        Ok(archive_file_name(row, named.as_deref()))
    }
}
