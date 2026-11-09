//! A document and its parts as a reader is shown them: names put in the place of identifiers, and what the reader may not see left without a name.
//!
//! The database keeps identifiers: the person who changed a block, the thread
//! of the agent that did, the person a mention names, the node or the document
//! a chip points at. A reader is given names, but only where they may know
//! them: a thread they cannot see is named `null` and marked redacted, and so
//! is a chip to a document they cannot read. This is looked up once for a
//! whole answer, in a few statements however many blocks it holds.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::middleware::authorization::refusals::user_refs;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::documents::limits::DocSettings;
use crate::routes::nodes::view;
use chrono::{DateTime, Utc};
use orch_access_contract::pages::PageState as StoredPageState;
use orch_access_contract::permissions::Permission;
use orch_api_contract::documents::{
    Author, AuthorAgent, BlockContent, BlockType, DOCUMENT_DESCRIPTION_MAX_CHARS,
    DOCUMENT_LINE_MAX_CHARS, DiscussionResolved, Document, DocumentAccess, DocumentBlock,
    DocumentComment, DocumentDiscussion, DocumentFile, DocumentRef, Inline, InlineRefKind,
    TableCell, TableRow,
};
use orch_api_contract::nodes::{NodeRef, PageState};
use orch_api_contract::users::UserRef;
use orch_authz::documents::Listed;
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::documents::discussions::{CommentRow, Full};
use orch_db::queries::documents::{self as rows, BlockRow, DocumentRow, FileRow};
use orch_documents::blocks;
use orch_documents::comments;
use orch_documents::describe;
use orch_documents::position::Position;
use orch_types::{Id, Time};

/// The resource whose already-readable label is copied into a newly written reference.
/// Attached files retain their actual document/chat owner from the ordinary Cast producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WrittenLabelSource {
    /// A node's current name.
    Node(Id),
    /// A document's title or an attached file's name.
    Document(Id),
    /// A chat's current name or a file carried by one of its messages.
    Chat(Id),
}

type WrittenReference = (InlineRefKind, Id, Option<String>);

fn missing_label_references<T: serde::Serialize>(content: &T) -> ApiResult<Vec<Inline>> {
    fn missing(value: &serde_json::Value, out: &mut Vec<Inline>) {
        match value {
            serde_json::Value::Object(object)
                if object.get("type").and_then(serde_json::Value::as_str) == Some("ref") =>
            {
                if !object
                    .get("label")
                    .is_some_and(serde_json::Value::is_string)
                    && let Ok(reference) = serde_json::from_value(value.clone())
                {
                    out.push(reference);
                }
            }
            serde_json::Value::Object(object) => {
                for value in object.values() {
                    missing(value, out);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    missing(value, out);
                }
            }
            _ => {}
        }
    }
    let value = serde_json::to_value(content).map_err(ApiError::internal)?;
    let mut refs = Vec::new();
    missing(&value, &mut refs);
    Ok(refs)
}

async fn reference_candidates(
    conn: &mut Conn,
    refs: &[Inline],
) -> ApiResult<Vec<WrittenLabelSource>> {
    let mut sources = BTreeSet::new();
    let mut files = Vec::new();
    let mut chats = Vec::new();
    for reference in refs {
        let Inline::Ref { kind, id, .. } = reference else {
            continue;
        };
        match kind {
            InlineRefKind::Node => {
                sources.insert(WrittenLabelSource::Node(*id));
            }
            InlineRefKind::Document => {
                sources.insert(WrittenLabelSource::Document(*id));
            }
            InlineRefKind::Chat => {
                sources.insert(WrittenLabelSource::Chat(*id));
                chats.push(*id);
            }
            InlineRefKind::File => files.push(*id),
            InlineRefKind::EnvironmentFile => {}
        }
    }
    if !chats.is_empty() {
        for chat in orch_db::queries::chats::chats(conn, &chats).await? {
            if chat.name.is_none() {
                sources.extend(chat.node_id.map(WrittenLabelSource::Node));
            }
        }
    }
    if !files.is_empty() {
        sources.extend(
            ReferenceFileRows::sources(conn, &files)
                .await?
                .into_values()
                .flatten(),
        );
    }
    Ok(sources.into_iter().collect())
}

async fn scoped_references(
    conn: &mut Conn,
    mut refs: Vec<Inline>,
    allowed: &BTreeSet<WrittenLabelSource>,
) -> ApiResult<Vec<Inline>> {
    let files: Vec<Id> = refs
        .iter()
        .filter_map(|reference| match reference {
            Inline::Ref {
                kind: InlineRefKind::File,
                id,
                ..
            } => Some(*id),
            _ => None,
        })
        .collect();
    let file_sources = ReferenceFileRows::sources(conn, &files).await?;
    refs.retain(|reference| match reference {
        Inline::Ref { kind, id, .. } => match kind {
            InlineRefKind::Node => allowed.contains(&WrittenLabelSource::Node(*id)),
            InlineRefKind::Document => allowed.contains(&WrittenLabelSource::Document(*id)),
            InlineRefKind::Chat => allowed.contains(&WrittenLabelSource::Chat(*id)),
            InlineRefKind::File => file_sources
                .get(id)
                .is_some_and(|sources| sources.iter().any(|source| allowed.contains(source))),
            InlineRefKind::EnvironmentFile => true,
        },
        _ => false,
    });
    Ok(refs)
}

fn visit_inline(content: &BlockContent, mut visit: impl FnMut(&Inline)) {
    match content {
        BlockContent::Inline(runs) => runs.iter().for_each(&mut visit),
        BlockContent::Rows(rows) => {
            for cell in rows.iter().flat_map(|row| &row.cells) {
                cell.content.iter().for_each(&mut visit);
            }
        }
    }
}

/// The exact labels the existing visibility producer would materialize for this input.
/// Keep this value with the operation's checked sources so applying it cannot discover a new source.
#[derive(Debug, Default)]
pub(crate) struct WrittenLabels {
    labels: HashMap<WrittenReference, String>,
    sources: Vec<WrittenLabelSource>,
}

impl WrittenLabels {
    /// Candidate governing identities before name/visibility evaluation. This uses the same stored
    /// file relations as Cast; it supplies no names and decides no key or account permission.
    pub(crate) async fn candidates<T: serde::Serialize>(
        conn: &mut Conn,
        content: &T,
    ) -> ApiResult<Vec<WrittenLabelSource>> {
        reference_candidates(conn, &missing_label_references(content)?).await
    }

    /// `allowed` is the shared operation planner's admitted candidate set, never request data.
    /// Optional labels outside it remain absent; a newly discovered file owner cannot enter it.
    pub(crate) async fn capture_allowed<T: serde::Serialize>(
        conn: &mut Conn,
        principal: &Principal,
        content: &T,
        allowed: &[WrittenLabelSource],
    ) -> ApiResult<Self> {
        Self::capture_for_sources(conn, principal, content, Some(allowed)).await
    }

    async fn capture_for_sources<T: serde::Serialize>(
        conn: &mut Conn,
        principal: &Principal,
        content: &T,
        allowed: Option<&[WrittenLabelSource]>,
    ) -> ApiResult<Self> {
        let mut refs = missing_label_references(content)?;
        let allowed = allowed.map(|sources| sources.iter().copied().collect::<BTreeSet<_>>());
        if let Some(allowed) = &allowed {
            refs = scoped_references(conn, refs, allowed).await?;
        }
        if refs.is_empty() {
            return Ok(Self::default());
        }
        let mut wanted = Wanted::default();
        wanted.runs(&refs);
        let cast =
            Cast::load_for_labels(conn, principal, wanted, Policy::default(), allowed.as_ref())
                .await?;
        let mut labels = HashMap::new();
        let mut sources = std::collections::BTreeSet::new();
        for inline in cast.expand(refs) {
            let Inline::Ref {
                kind,
                id,
                path,
                name: Some(Some(name)),
                ..
            } = inline
            else {
                continue;
            };
            let source = cast.materialized_source(kind, id)?;
            if source.iter().any(|source| {
                allowed
                    .as_ref()
                    .is_some_and(|allowed| !allowed.contains(source))
            }) {
                continue;
            }
            sources.extend(source);
            labels.insert((kind, id, path), name);
        }
        Ok(Self {
            labels,
            sources: sources.into_iter().collect(),
        })
    }

    /// Actual eligible sources, for the shared operation resolver's target binding.
    pub(crate) fn sources(&self) -> &[WrittenLabelSource] {
        &self.sources
    }

    /// Applies only the captured values, without another name or permission lookup. Supplied
    /// labels remain untouched, including an explicitly empty one.
    pub(crate) fn apply<T: serde::Serialize + serde::de::DeserializeOwned>(
        &self,
        content: T,
    ) -> ApiResult<T> {
        fn fill(value: &mut serde_json::Value, labels: &HashMap<WrittenReference, String>) {
            match value {
                serde_json::Value::Object(object)
                    if object.get("type").and_then(serde_json::Value::as_str) == Some("ref") =>
                {
                    if !object
                        .get("label")
                        .is_some_and(serde_json::Value::is_string)
                        && let Ok(Inline::Ref { kind, id, path, .. }) =
                            serde_json::from_value(serde_json::Value::Object(object.clone()))
                        && let Some(label) = labels.get(&(kind, id, path))
                    {
                        object.insert("label".to_owned(), serde_json::Value::String(label.clone()));
                    }
                }
                serde_json::Value::Object(object) => {
                    for value in object.values_mut() {
                        fill(value, labels);
                    }
                }
                serde_json::Value::Array(items) => {
                    for value in items {
                        fill(value, labels);
                    }
                }
                _ => {}
            }
        }
        let mut value = serde_json::to_value(content).map_err(ApiError::internal)?;
        fill(&mut value, &self.labels);
        serde_json::from_value(value).map_err(ApiError::internal)
    }
}

/// What a reader's answer is going to name.
#[derive(Debug, Default)]
pub struct Wanted {
    users: Vec<Id>,
    nodes: Vec<Id>,
    documents: Vec<Id>,
    chats: Vec<Id>,
    files: Vec<Id>,
    environments: Vec<Id>,
}

impl Wanted {
    /// A person.
    pub fn user(&mut self, id: Id) {
        self.users.push(id);
    }

    /// A node, an agent's thread among them.
    pub fn node(&mut self, id: Id) {
        self.nodes.push(id);
    }

    /// A document.
    pub fn document(&mut self, id: Id) {
        self.documents.push(id);
    }

    /// A chat.
    pub fn chat(&mut self, id: Id) {
        self.chats.push(id);
    }

    /// A file attached to a document or a chat message.
    pub fn file(&mut self, id: Id) {
        self.files.push(id);
    }

    /// An environment whose files a chip may name.
    pub fn environment(&mut self, id: Id) {
        self.environments.push(id);
    }

    /// The people and things the inline content names.
    pub fn runs(&mut self, runs: &[Inline]) {
        for run in runs {
            match run {
                Inline::Mention { user_id, .. } => self.users.push(*user_id),
                Inline::Ref {
                    kind: InlineRefKind::Node,
                    id,
                    ..
                } => self.nodes.push(*id),
                Inline::Ref {
                    kind: InlineRefKind::Document,
                    id,
                    ..
                } => self.documents.push(*id),
                Inline::Ref {
                    kind: InlineRefKind::Chat,
                    id,
                    ..
                } => self.chats.push(*id),
                Inline::Ref {
                    kind: InlineRefKind::File,
                    id,
                    ..
                } => self.files.push(*id),
                Inline::Ref {
                    kind: InlineRefKind::EnvironmentFile,
                    id,
                    ..
                } => self.environments.push(*id),
                _ => {}
            }
        }
    }

    /// The people and things a block's content names, a table's cells among them.
    pub fn content(&mut self, content: &BlockContent) {
        match content {
            BlockContent::Inline(runs) => self.runs(runs),
            BlockContent::Rows(rows) => {
                for cell in rows.iter().flat_map(|row| row.cells.iter()) {
                    self.runs(&cell.content);
                }
            }
        }
    }

    /// Who changed a block, and what its content names.
    pub fn block(&mut self, row: &BlockRow) {
        self.users.extend(row.updated_by);
        self.nodes.extend(row.updated_by_thread);
        self.content(&blocks::content_from_json(&row.content));
    }

    fn comment_people(&mut self, row: &CommentRow) {
        self.users.push(row.author_id);
        for each in row.reaction_list() {
            self.users.extend(each.users);
        }
    }

    /// Who wrote a comment, who reacted to it, and what it names.
    pub fn comment(&mut self, row: &CommentRow) {
        self.comment_people(row);
        if let Ok(runs) = serde_json::from_value::<Vec<Inline>>(row.content.clone()) {
            self.runs(&runs);
        }
    }
}

/// The existing attached/carried file label producer's exact stored associations.
struct ReferenceFileRows {
    attached: Vec<(Id, Id, String)>,
    carried: Vec<(Id, Id, i32, orch_types::Json)>,
}

impl ReferenceFileRows {
    /// Governing identities only: no filename or carried message content is read before scope filtering.
    async fn sources(
        conn: &mut Conn,
        files: &[Id],
    ) -> ApiResult<HashMap<Id, Vec<WrittenLabelSource>>> {
        if files.is_empty() {
            return Ok(HashMap::new());
        }
        let attached: Vec<(Id, Id)> =
            sqlx::query_as("SELECT id,document_id FROM document_files WHERE id=ANY($1)")
                .bind(files)
                .fetch_all(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        let carried: Vec<(Id, Id)> = sqlx::query_as("SELECT f.id,f.chat_id FROM chat_file_references f JOIN chat_messages m ON m.id=f.message_id WHERE f.id=ANY($1) AND m.deleted_at IS NULL")
            .bind(files).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let mut sources: HashMap<Id, Vec<WrittenLabelSource>> = HashMap::new();
        for (file, document) in attached {
            sources
                .entry(file)
                .or_default()
                .push(WrittenLabelSource::Document(document));
        }
        for (file, chat) in carried {
            sources
                .entry(file)
                .or_default()
                .push(WrittenLabelSource::Chat(chat));
        }
        Ok(sources)
    }

    async fn load(
        conn: &mut Conn,
        files: &[Id],
        allowed: Option<&BTreeSet<WrittenLabelSource>>,
    ) -> ApiResult<Self> {
        let documents = allowed.map(|sources| {
            sources
                .iter()
                .filter_map(|source| match source {
                    WrittenLabelSource::Document(id) => Some(*id),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        let chats = allowed.map(|sources| {
            sources
                .iter()
                .filter_map(|source| match source {
                    WrittenLabelSource::Chat(id) => Some(*id),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        let attached = sqlx::query_as("SELECT id,document_id,name FROM document_files WHERE id=ANY($1) AND ($2::uuid[] IS NULL OR document_id=ANY($2))")
            .bind(files).bind(documents.as_deref()).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let carried = sqlx::query_as("SELECT f.id,f.chat_id,f.carried,m.content FROM chat_file_references f JOIN chat_messages m ON m.id=f.message_id WHERE f.id=ANY($1) AND m.deleted_at IS NULL AND ($2::uuid[] IS NULL OR f.chat_id=ANY($2))")
            .bind(files).bind(chats.as_deref()).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        Ok(Self { attached, carried })
    }
}

/// The names a reader is given.
#[derive(Debug, Default)]
pub struct Cast {
    users: HashMap<Id, UserRef>,
    nodes: HashMap<Id, NodeRef>,
    titles: HashMap<Id, Option<String>>,
    /// The name of each chat a chip names, where the reader may read the chat: its own, or its place's.
    chats: HashMap<Id, Option<String>>,
    chat_name_nodes: HashMap<Id, Id>,
    files: HashMap<Id, String>,
    file_sources: HashMap<Id, WrittenLabelSource>,
    environments: HashMap<Id, String>,
    reference_sources: Option<BTreeSet<WrittenLabelSource>>,
}

impl Cast {
    /// Existing live labels selected by this native Cast, before any block or comment DTO is built.
    pub(crate) fn live_sources<'a>(
        &self,
        runs: impl IntoIterator<Item = &'a Inline>,
    ) -> ApiResult<Vec<WrittenLabelSource>> {
        let mut sources = BTreeSet::new();
        for run in runs {
            if let Inline::Ref { kind, id, path, .. } = run
                && self.reference_name(*kind, *id, path.as_deref()).is_some()
            {
                sources.extend(self.materialized_source(*kind, *id)?);
            }
        }
        Ok(sources.into_iter().collect())
    }
    /// One block source walk, with any stored version authors supplied by its caller.
    /// Ordinary DTO construction remains `block`; only external live names are bounded.
    pub(super) async fn for_blocks(
        conn: &mut Conn,
        principal: &Principal,
        mut wanted: Wanted,
        rows: &[BlockRow],
        policy: Policy,
        allowed: Option<&[WrittenLabelSource]>,
    ) -> ApiResult<Self> {
        let Some(allowed) = allowed else {
            for row in rows {
                wanted.block(row);
            }
            return Self::load(conn, principal, wanted, policy).await;
        };
        let allowed: BTreeSet<_> = allowed.iter().copied().collect();
        let mut refs = Vec::new();
        for row in rows {
            wanted.users.extend(row.updated_by);
            wanted.nodes.extend(row.updated_by_thread);
            visit_inline(&blocks::content_from_json(&row.content), |run| match run {
                Inline::Mention { user_id, .. } => wanted.user(*user_id),
                Inline::Ref { .. } => refs.push(run.clone()),
                _ => {}
            });
        }
        wanted.runs(&scoped_references(conn, refs, &allowed).await?);
        Self::load_for_labels(conn, principal, wanted, policy, Some(&allowed)).await
    }

    /// One comment source walk for standalone windows and discussions. Ordinary
    /// DTO construction remains `comment`; only live external label lookup is bounded.
    async fn for_comments(
        conn: &mut Conn,
        principal: &Principal,
        mut wanted: Wanted,
        comments: &[&CommentRow],
        policy: Policy,
        allowed: Option<&[WrittenLabelSource]>,
    ) -> ApiResult<Self> {
        let Some(allowed) = allowed else {
            for row in comments {
                wanted.comment(row);
            }
            return Self::load(conn, principal, wanted, policy).await;
        };
        let allowed: BTreeSet<_> = allowed.iter().copied().collect();
        let mut refs = Vec::new();
        for row in comments {
            wanted.comment_people(row);
            for run in
                serde_json::from_value::<Vec<Inline>>(row.content.clone()).unwrap_or_default()
            {
                match run {
                    Inline::Mention { user_id, .. } => wanted.user(user_id),
                    Inline::Ref { .. } => refs.push(run),
                    _ => {}
                }
            }
        }
        wanted.runs(&scoped_references(conn, refs, &allowed).await?);
        Self::load_for_labels(conn, principal, wanted, policy, Some(&allowed)).await
    }

    /// Looks up everything `wanted` names, for `principal`.
    pub async fn load(
        conn: &mut Conn,
        principal: &Principal,
        wanted: Wanted,
        policy: Policy,
    ) -> ApiResult<Self> {
        Self::load_for_labels(conn, principal, wanted, policy, None).await
    }

    pub(crate) async fn load_for_labels(
        conn: &mut Conn,
        principal: &Principal,
        mut wanted: Wanted,
        policy: Policy,
        allowed: Option<&BTreeSet<WrittenLabelSource>>,
    ) -> ApiResult<Self> {
        let ReferenceFileRows { attached, carried } =
            ReferenceFileRows::load(conn, &wanted.files, allowed).await?;
        wanted
            .documents
            .extend(attached.iter().map(|(_, owner, _)| *owner));
        wanted
            .chats
            .extend(carried.iter().map(|(_, owner, _, _)| *owner));
        for list in [
            &mut wanted.users,
            &mut wanted.nodes,
            &mut wanted.documents,
            &mut wanted.chats,
            &mut wanted.environments,
        ] {
            list.sort_unstable();
            list.dedup();
        }
        let users = user_refs(conn, &wanted.users).await?;
        let nodes = match allowed {
            None => view::refs(conn, principal, &wanted.nodes, policy).await?,
            Some(allowed) => {
                let nodes: BTreeSet<Id> = allowed
                    .iter()
                    .filter_map(|source| match source {
                        WrittenLabelSource::Node(id) => Some(*id),
                        _ => None,
                    })
                    .collect();
                view::refs_with_name_sources(conn, principal, &wanted.nodes, policy, &nodes).await?
            }
        };
        let mut titles: HashMap<Id, Option<String>> = HashMap::new();
        if !wanted.documents.is_empty() {
            let named: HashMap<Id, String> = rows::get_many(conn, &wanted.documents)
                .await?
                .into_iter()
                .map(|row| (row.id, row.title))
                .collect();
            let listed = orch_authz::documents::listed(conn, principal, &wanted.documents, policy)
                .await
                .map_err(ApiError::internal)?;
            for each in listed {
                let readable = each.holds(Permission::DocView)
                    && (!each.document.deleted
                        || orch_db::queries::trace::retained_read(each.document.id));
                titles.insert(
                    each.document.id,
                    named.get(&each.document.id).filter(|_| readable).cloned(),
                );
            }
        }
        let mut chats: HashMap<Id, Option<String>> = HashMap::new();
        let mut chat_name_nodes = HashMap::new();
        if !wanted.chats.is_empty() {
            let rows = orch_db::queries::chats::chats(conn, &wanted.chats).await?;
            let readable = orch_authz::chats::listed_access(conn, principal, &rows)
                .await
                .map_err(ApiError::internal)?;
            let places: Vec<Id> = rows.iter().filter_map(|row| row.node_id).collect();
            let named = view::refs(conn, principal, &places, policy).await?;
            for row in rows {
                if row.name.is_none()
                    && let Some(node) = row.node_id
                {
                    chat_name_nodes.insert(row.id, node);
                }
                let name = row.name.clone().or_else(|| {
                    row.node_id
                        .and_then(|node| named.get(&node))
                        .and_then(|node| node.name.clone())
                });
                chats.insert(row.id, name.filter(|_| readable.contains_key(&row.id)));
            }
        }
        let mut files = HashMap::new();
        let mut file_sources = HashMap::new();
        for (id, owner, name) in attached {
            if titles.get(&owner).is_some_and(Option::is_some) {
                files.insert(id, name);
                file_sources.insert(id, WrittenLabelSource::Document(owner));
            }
        }
        for (id, owner, slot, content) in carried {
            if chats.get(&owner).is_some_and(Option::is_some)
                && let Ok(parts) =
                    serde_json::from_value::<Vec<orch_api_contract::chats::ChatPart>>(content)
                && let Some((_, name, _, _)) = usize::try_from(slot)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|at| super::references::chat_files(parts).into_iter().nth(at))
            {
                files.insert(id, name);
                file_sources.insert(id, WrittenLabelSource::Chat(owner));
            }
        }
        let mut environments = HashMap::new();
        for id in wanted.environments {
            if orch_authz::resources::check(
                conn,
                principal,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id,
                },
                Permission::FilesView,
                policy,
            )
            .await
            .is_ok_and(|decision| decision.is_allowed())
                && let Some(env) =
                    orch_db::queries::environments::workspace::environment(conn, id).await?
            {
                environments.insert(id, env.name);
            }
        }
        Ok(Self {
            users,
            nodes,
            titles,
            chats,
            chat_name_nodes,
            files,
            file_sources,
            environments,
            reference_sources: allowed.cloned(),
        })
    }

    /// A person.
    #[must_use]
    pub fn person(&self, id: Id) -> Option<UserRef> {
        self.users.get(&id).cloned()
    }

    /// A node.
    #[must_use]
    pub fn node(&self, id: Id) -> Option<NodeRef> {
        self.nodes.get(&id).cloned()
    }

    /// The name of a chat, where the reader may read it.
    #[must_use]
    pub fn chat_of(&self, id: Id) -> Option<String> {
        self.chats.get(&id).cloned().flatten()
    }

    /// The attachment's name, where its owner is readable.
    pub fn file_of(&self, id: Id) -> Option<String> {
        self.files.get(&id).cloned()
    }

    /// The name of a workspace file where the reader may read its environment's files: the last part of its path.
    #[must_use]
    pub fn environment_file_of(&self, id: Id, path: &str) -> Option<String> {
        self.environments
            .get(&id)
            .map(|_| path.rsplit('/').next().unwrap_or(path).to_owned())
    }

    /// The title of a document, where the reader may know it.
    #[must_use]
    pub fn title_of(&self, id: Id) -> Option<String> {
        self.titles.get(&id).cloned().flatten()
    }

    /// Who changed something: a person, or an agent by the thread it worked in.
    #[must_use]
    pub fn author(
        &self,
        user: Option<Id>,
        thread: Option<Id>,
        agent: Option<&str>,
    ) -> Option<Author> {
        match (user, thread, agent) {
            (Some(user), _, _) => self.person(user).map(Author::User),
            (None, Some(thread), Some(agent)) => self.node(thread).map(|thread| {
                Author::Agent(AuthorAgent {
                    agent: agent.to_owned(),
                    thread,
                })
            }),
            _ => None,
        }
    }

    fn materialized_source(
        &self,
        kind: InlineRefKind,
        id: Id,
    ) -> ApiResult<Vec<WrittenLabelSource>> {
        let mut sources: Vec<_> = match kind {
            InlineRefKind::Node => Some(WrittenLabelSource::Node(id)),
            InlineRefKind::Document => Some(WrittenLabelSource::Document(id)),
            InlineRefKind::Chat => Some(WrittenLabelSource::Chat(id)),
            InlineRefKind::File => Some(*self.file_sources.get(&id).ok_or_else(|| {
                ApiError::internal("a materialized file name has no governing source")
            })?),
            // A workspace basename comes from the submitted/stored path, not an external name.
            InlineRefKind::EnvironmentFile => None,
        }
        .into_iter()
        .collect();
        if kind == InlineRefKind::Chat
            && let Some(node) = self.chat_name_nodes.get(&id)
        {
            sources.push(WrittenLabelSource::Node(*node));
        }
        Ok(sources)
    }

    fn reference_name(&self, kind: InlineRefKind, id: Id, path: Option<&str>) -> Option<String> {
        if let Some(allowed) = &self.reference_sources {
            if kind == InlineRefKind::Chat
                && self
                    .chat_name_nodes
                    .get(&id)
                    .is_some_and(|node| !allowed.contains(&WrittenLabelSource::Node(*node)))
            {
                return None;
            }
            let source = match kind {
                InlineRefKind::Node => Some(WrittenLabelSource::Node(id)),
                InlineRefKind::Document => Some(WrittenLabelSource::Document(id)),
                InlineRefKind::Chat => Some(WrittenLabelSource::Chat(id)),
                InlineRefKind::File => self.file_sources.get(&id).copied(),
                InlineRefKind::EnvironmentFile => None,
            };
            if source.is_some_and(|source| !allowed.contains(&source))
                || (kind == InlineRefKind::File && source.is_none())
            {
                return None;
            }
        }
        match kind {
            InlineRefKind::Node => self.node(id).and_then(|node| node.name),
            InlineRefKind::Document => self.titles.get(&id).cloned().flatten(),
            InlineRefKind::Chat => self.chats.get(&id).cloned().flatten(),
            InlineRefKind::File => self.files.get(&id).cloned(),
            InlineRefKind::EnvironmentFile => self
                .environments
                .get(&id)
                .and(path)
                .map(|path| path.rsplit('/').next().unwrap_or(path).to_owned()),
        }
    }

    /// Inline content with what a reader is given besides it: the person a
    /// mention names, and the name of a chip where they may know it.
    #[must_use]
    pub fn expand(&self, runs: Vec<Inline>) -> Vec<Inline> {
        runs.into_iter()
            .map(|run| match run {
                Inline::Mention { user_id, .. } => Inline::Mention {
                    user_id,
                    user: self.person(user_id),
                },
                Inline::Ref {
                    display,
                    kind,
                    id,
                    path,
                    part,
                    label,
                    ..
                } => {
                    let name = self.reference_name(kind, id, path.as_deref());
                    Inline::Ref {
                        display,
                        kind,
                        id,
                        // Keep the pointer through editing even when its looked-up name is private.
                        path,
                        part,
                        label,
                        name: Some(name),
                    }
                }
                other => other,
            })
            .collect()
    }

    fn content(&self, content: BlockContent) -> BlockContent {
        match content {
            BlockContent::Inline(runs) => BlockContent::Inline(self.expand(runs)),
            BlockContent::Rows(rows) => BlockContent::Rows(
                rows.into_iter()
                    .map(|row| TableRow {
                        cells: row
                            .cells
                            .into_iter()
                            .map(|cell| TableCell {
                                content: self.expand(cell.content),
                                ..cell
                            })
                            .collect(),
                    })
                    .collect(),
            ),
        }
    }

    /// A block as a reader is shown it.
    #[must_use]
    pub fn block(&self, row: &BlockRow) -> DocumentBlock {
        let position = Position::parse(&row.position)
            .map_or_else(|| row.position.clone(), |position| position.to_string());
        DocumentBlock {
            id: row.id,
            document_id: row.document_id,
            parent_id: row.parent_id,
            position,
            block_type: block_type_of(&row.block_type),
            props: blocks::props_from_json(&row.props),
            content: self.content(blocks::content_from_json(&row.content)),
            removed: row.removed_at.is_some(),
            seq_created: row.seq_created,
            seq_updated: row.seq_updated,
            updated_by: self.author(
                row.updated_by,
                row.updated_by_thread,
                row.updated_by_agent.as_deref(),
            ),
            updated_at: Time::new(row.updated_at),
        }
    }

    /// A comment as a reader is shown it; none where its author is gone.
    #[must_use]
    pub fn comment(&self, row: &CommentRow) -> Option<DocumentComment> {
        let content =
            serde_json::from_value::<Vec<Inline>>(row.content.clone()).unwrap_or_default();
        Some(DocumentComment {
            id: row.id,
            discussion_id: row.discussion_id,
            author: self.person(row.author_id)?,
            content: self.expand(content),
            reactions: row.reaction_list(),
            edited_at: row.edited_at.map(Time::new),
            deleted: row.deleted_at.is_some(),
            created_at: Time::new(row.created_at),
            seq_updated: row.seq_updated,
        })
    }

    /// A discussion as a reader is shown it. `about` holds the blocks of the
    /// document that discussions are about, by identifier.
    #[must_use]
    pub fn discussion(
        &self,
        full: &Full,
        about: &HashMap<Id, BlockRow>,
    ) -> Option<DocumentDiscussion> {
        let discussion = &full.discussion;
        let block = discussion
            .block_id
            .and_then(|id| about.get(&id))
            .filter(|block| block.removed_at.is_none());
        let resolved = match (discussion.resolved_at, discussion.resolved_by) {
            (Some(at), Some(by)) => Some(DiscussionResolved {
                at: Time::new(at),
                by: self.person(by)?,
            }),
            _ => None,
        };
        Some(DocumentDiscussion {
            id: discussion.id,
            document_id: discussion.document_id,
            block_id: block.map(|block| block.id),
            quote: discussion.quote.clone(),
            anchored: comments::anchored(&discussion.quote, block.map(|block| block.text.as_str())),
            resolved,
            created_by: self.person(discussion.created_by)?,
            created_at: Time::new(discussion.created_at),
            comments: full
                .comments
                .iter()
                .filter_map(|comment| self.comment(comment))
                .collect(),
            seq_updated: discussion.seq_updated,
        })
    }
}

/// The type of a stored block; a word that is none of the twenty reads as a
/// paragraph, which nothing stored here is.
#[must_use]
pub fn block_type_of(word: &str) -> BlockType {
    serde_json::from_value(serde_json::Value::String(word.to_owned()))
        .unwrap_or(BlockType::Paragraph)
}

/// The blocks that discussions are about, by identifier.
pub async fn blocks_about(
    conn: &mut Conn,
    document: Id,
    discussions: &[Full],
) -> ApiResult<HashMap<Id, BlockRow>> {
    let ids: Vec<Id> = discussions
        .iter()
        .filter_map(|full| full.discussion.block_id)
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(rows::blocks_by_ids(conn, document, &ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect())
}

/// A frozen full-block projection and the external reference names it actually materialized.
/// Stored labels, snapshot text, props and all ordinary block fields are preserved.
#[derive(Debug)]
pub(crate) struct ProjectedBlocks {
    pub(crate) blocks: Vec<DocumentBlock>,
    sources: Vec<WrittenLabelSource>,
}

impl ProjectedBlocks {
    /// All references in the stored touched rows, including already-labeled references and rows
    /// touched only by a move/props change. No input-only candidate list can replace this.
    pub(crate) async fn candidates(
        conn: &mut Conn,
        list: &[BlockRow],
    ) -> ApiResult<Vec<WrittenLabelSource>> {
        let mut refs = Vec::new();
        for row in list {
            visit_inline(&blocks::content_from_json(&row.content), |run| {
                if matches!(run, Inline::Ref { .. }) {
                    refs.push(run.clone());
                }
            });
        }
        let mut sources: BTreeSet<_> = reference_candidates(conn, &refs)
            .await?
            .into_iter()
            .collect();
        sources.extend(
            list.iter()
                .filter(|row| row.updated_by.is_none())
                .filter_map(|row| row.updated_by_thread)
                .map(WrittenLabelSource::Node),
        );
        Ok(sources.into_iter().collect())
    }

    /// Reuses Cast and its ordinary visibility rules, with trusted same-operation source facts
    /// restricting dynamic reference and agent-attribution node names. Stored agent names and
    /// public user identity semantics stay unchanged.
    pub(crate) async fn capture_allowed(
        conn: &mut Conn,
        principal: &Principal,
        list: &[BlockRow],
        policy: Policy,
        allowed: &[WrittenLabelSource],
    ) -> ApiResult<Self> {
        let cast = Cast::for_blocks(
            conn,
            principal,
            Wanted::default(),
            list,
            policy,
            Some(allowed),
        )
        .await?;
        let blocks: Vec<_> = list.iter().map(|row| cast.block(row)).collect();
        let mut materialized = Vec::new();
        for block in &blocks {
            visit_inline(&block.content, |run| {
                if let Inline::Ref {
                    kind,
                    id,
                    name: Some(Some(_)),
                    ..
                } = run
                {
                    materialized.push((*kind, *id));
                }
            });
        }
        let mut sources = BTreeSet::new();
        for (kind, id) in materialized {
            sources.extend(cast.materialized_source(kind, id)?);
        }
        for block in &blocks {
            if let Some(Author::Agent(agent)) = &block.updated_by {
                named_node_source(&agent.thread, &mut sources);
            }
        }
        Ok(Self {
            blocks,
            sources: sources.into_iter().collect(),
        })
    }

    /// Only names actually returned require response-stage source binding.
    pub(crate) fn sources(&self) -> &[WrittenLabelSource] {
        &self.sources
    }
}

/// Blocks as a reader is shown them, with the names looked up once.
pub async fn blocks_for(
    conn: &mut Conn,
    principal: &Principal,
    list: &[BlockRow],
    policy: Policy,
) -> ApiResult<Vec<DocumentBlock>> {
    let cast = Cast::for_blocks(conn, principal, Wanted::default(), list, policy, None).await?;
    Ok(list.iter().map(|row| cast.block(row)).collect())
}

/// Discussions as a reader is shown them.
pub async fn discussions_for(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    fulls: &[Full],
    policy: Policy,
) -> ApiResult<Vec<DocumentDiscussion>> {
    discussions_bounded(conn, principal, document, fulls, policy, None).await
}

/// Exact reference candidates in existing comments; quoted/stored text creates no source read.
pub(crate) async fn discussion_sources(
    conn: &mut Conn,
    fulls: &[Full],
) -> ApiResult<Vec<WrittenLabelSource>> {
    let refs = comment_references(fulls.iter().flat_map(|full| &full.comments));
    reference_candidates(conn, &refs).await
}

pub(crate) async fn comment_sources(
    conn: &mut Conn,
    comments: &[CommentRow],
) -> ApiResult<Vec<WrittenLabelSource>> {
    let refs = comment_references(comments.iter());
    reference_candidates(conn, &refs).await
}

fn comment_references<'a>(comments: impl Iterator<Item = &'a CommentRow>) -> Vec<Inline> {
    comments
        .flat_map(|row| {
            serde_json::from_value::<Vec<Inline>>(row.content.clone()).unwrap_or_default()
        })
        .filter(|run| matches!(run, Inline::Ref { .. }))
        .collect()
}

/// The same discussion producer with only live external labels restricted by operation facts.
pub(crate) async fn discussions_bounded(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    fulls: &[Full],
    policy: Policy,
    allowed: Option<&[WrittenLabelSource]>,
) -> ApiResult<Vec<DocumentDiscussion>> {
    Ok(
        discussions_projection(conn, principal, document, fulls, policy, allowed)
            .await?
            .0,
    )
}

/// The same native discussion projection, with an optional witness of actual live names.
pub(crate) async fn discussions_projection(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    fulls: &[Full],
    policy: Policy,
    allowed: Option<&[WrittenLabelSource]>,
) -> ApiResult<(Vec<DocumentDiscussion>, Vec<WrittenLabelSource>)> {
    let mut wanted = Wanted::default();
    for full in fulls {
        wanted.user(full.discussion.created_by);
        wanted.users.extend(full.discussion.resolved_by);
    }
    let comments = fulls
        .iter()
        .flat_map(|full| &full.comments)
        .collect::<Vec<_>>();
    let cast = Cast::for_comments(conn, principal, wanted, &comments, policy, allowed).await?;
    let about = blocks_about(conn, document, fulls).await?;
    let discussions: Vec<_> = fulls
        .iter()
        .filter_map(|full| cast.discussion(full, &about))
        .collect();
    let sources = if allowed.is_some() {
        comment_projection_sources(&cast, discussions.iter().flat_map(|full| &full.comments))?
    } else {
        Vec::new()
    };
    Ok((discussions, sources))
}

/// A standalone comment window uses that same ordinary/bounded Cast, without
/// fetching the other comments or metadata of its document.
pub(crate) async fn comments_bounded(
    conn: &mut Conn,
    principal: &Principal,
    comments: &[CommentRow],
    policy: Policy,
    allowed: Option<&[WrittenLabelSource]>,
) -> ApiResult<Vec<DocumentComment>> {
    Ok(
        comments_projection(conn, principal, comments, policy, allowed)
            .await?
            .0,
    )
}

/// The same native standalone comment producer, exposing only the names it materialized.
pub(crate) async fn comments_projection(
    conn: &mut Conn,
    principal: &Principal,
    comments: &[CommentRow],
    policy: Policy,
    allowed: Option<&[WrittenLabelSource]>,
) -> ApiResult<(Vec<DocumentComment>, Vec<WrittenLabelSource>)> {
    let rows = comments.iter().collect::<Vec<_>>();
    let cast =
        Cast::for_comments(conn, principal, Wanted::default(), &rows, policy, allowed).await?;
    let comments: Vec<_> = comments
        .iter()
        .filter_map(|row| cast.comment(row))
        .collect();
    let sources = if allowed.is_some() {
        comment_projection_sources(&cast, &comments)?
    } else {
        Vec::new()
    };
    Ok((comments, sources))
}

fn comment_projection_sources<'a>(
    cast: &Cast,
    comments: impl IntoIterator<Item = &'a DocumentComment>,
) -> ApiResult<Vec<WrittenLabelSource>> {
    let mut sources = BTreeSet::new();
    for run in comments.into_iter().flat_map(|comment| &comment.content) {
        if let Inline::Ref {
            kind,
            id,
            name: Some(Some(_)),
            ..
        } = run
        {
            sources.extend(cast.materialized_source(*kind, *id)?);
        }
    }
    Ok(sources.into_iter().collect())
}

/// Protected live identities used by the complete document metadata, before any name projection.
pub(crate) async fn document_name_sources(
    conn: &mut Conn,
    row: &DocumentRow,
    listed: &Listed,
) -> ApiResult<crate::routes::access::page::NameSources> {
    let mut names = crate::routes::access::page::NameSources::default();
    for source in ProjectedDocumentRef::candidates(conn, row, listed).await? {
        if let WrittenLabelSource::Node(id) = source {
            names.nodes.insert(id);
        }
    }
    for file in rows::files(conn, row.id).await? {
        if file.added_by.is_none() {
            names.nodes.extend(file.added_by_thread);
        }
    }
    let graph =
        orch_authz::engine::Graph::load(conn, &names.nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    names.nodes.extend(graph.nodes().map(|node| node.id));
    names.containers.extend(
        rows::containers::ancestors_many(conn, &[row.id])
            .await?
            .into_iter()
            .map(|row| row.id),
    );
    Ok(names)
}

/// The page state of the engine as the API words it.
#[must_use]
pub fn page_state(state: StoredPageState) -> PageState {
    match state {
        StoredPageState::Follows => PageState::Follows,
        StoredPageState::Custom => PageState::Custom,
        StoredPageState::Restricted => PageState::Restricted,
    }
}

/// What the caller holds on one document, as the engine says.
pub async fn standing(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    policy: Policy,
) -> ApiResult<Listed> {
    orch_authz::documents::listed(conn, principal, &[id], policy)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}

/// What a list shows beside each document's title: the first line of its description, or while it has none the
/// first line of its first paragraph, by the one rule. The first paragraphs are read, in one statement, only for
/// the documents that have no description.
pub async fn lines_for<'a>(
    conn: &mut Conn,
    documents: impl IntoIterator<Item = &'a DocumentRow>,
) -> ApiResult<HashMap<Id, String>> {
    let documents: Vec<&DocumentRow> = documents.into_iter().collect();
    let undescribed: Vec<Id> = documents
        .iter()
        .filter(|row| describe::first_line(&row.description).is_empty())
        .map(|row| row.id)
        .collect();
    // As much of a paragraph is read as a description may hold: its first line is found within it.
    let read = i32::try_from(DOCUMENT_DESCRIPTION_MAX_CHARS).unwrap_or(i32::MAX);
    // On the heap: a list is built inside many requests' futures, which a statement's own would make larger.
    let paragraphs = Box::pin(rows::first_paragraphs(conn, &undescribed, read)).await?;
    let chars = usize::try_from(DOCUMENT_LINE_MAX_CHARS).unwrap_or(usize::MAX);
    Ok(documents
        .iter()
        .map(|row| {
            let first = paragraphs.get(&row.id).map(String::as_str);
            (row.id, describe::line(&row.description, first, chars))
        })
        .collect())
}

/// Documents as a list shows them, without their blocks.
pub async fn refs(
    conn: &mut Conn,
    principal: &Principal,
    entries: &[(DocumentRow, Listed)],
    now: DateTime<Utc>,
    policy: Policy,
) -> ApiResult<Vec<DocumentRef>> {
    Ok(
        refs_with_name_sources(conn, principal, entries, now, policy, None)
            .await?
            .0,
    )
}

async fn refs_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    entries: &[(DocumentRow, Listed)],
    now: DateTime<Utc>,
    policy: Policy,
    allowed: Option<&BTreeSet<WrittenLabelSource>>,
) -> ApiResult<(Vec<DocumentRef>, Vec<WrittenLabelSource>)> {
    if entries.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let ids: Vec<Id> = entries.iter().map(|(row, _)| row.id).collect();
    let owners = rows::owner_counts(conn, &ids, now).await?;
    let open = rows::open_discussion_counts(conn, &ids).await?;
    let seen = rows::seen_seqs(conn, principal.user(), &ids).await?;
    let levels = rows::people::levels(conn, principal.user(), &ids).await?;
    let by_agent: Vec<Id> = entries
        .iter()
        .filter(|(row, _)| row.updated_by.is_none() && row.doc_seq > 0)
        .map(|(row, _)| row.id)
        .collect();
    let agents = rows::last_agents(conn, &by_agent).await?;
    let lines = Box::pin(lines_for(conn, entries.iter().map(|(row, _)| row))).await?;
    let mut wanted = Wanted::default();
    for (row, listed) in entries {
        if let Some(id) = listed.admin_access.and_then(|reach| reach.place.node()) {
            wanted.node(id);
        }
        wanted.user(row.owner_id);
        wanted.users.extend(row.created_by);
        wanted.users.extend(row.updated_by);
        wanted.node(row.node_id);
        wanted.nodes.extend(row.created_by_thread);
    }
    for (thread, _) in agents.values() {
        wanted.node(*thread);
    }
    let marker_places: HashMap<_, _> = entries
        .iter()
        .filter_map(|(row, listed)| listed.admin_access.map(|place| (row.id, place)))
        .collect();
    let marker_ids: Vec<Id> = marker_places
        .values()
        .filter_map(|reach| reach.place.node())
        .collect();
    let marker_graph = if !marker_ids.is_empty() {
        let graph = orch_authz::engine::Graph::load(conn, &marker_ids)
            .await
            .map_err(ApiError::internal)?;
        for id in &marker_ids {
            if let Some(node) = graph.get(*id) {
                wanted.nodes.extend(node.path.iter().copied());
            }
        }
        Some(graph)
    } else {
        None
    };
    let cast = Cast::load_for_labels(conn, principal, wanted, policy, allowed).await?;
    let admin_markers = match allowed {
        None => view::named_admin_access(conn, principal, &marker_places).await?,
        Some(_) => {
            let mut qualified = HashMap::new();
            for id in &marker_ids {
                if let Some(node) = cast.node(*id) {
                    let named = if node.name.is_some() {
                        marker_graph
                            .as_ref()
                            .and_then(|graph| view::qualified_from(graph, &cast.nodes, *id))
                            .unwrap_or(node)
                    } else {
                        node
                    };
                    qualified.insert(*id, named);
                }
            }
            marker_places
                .iter()
                .filter_map(|(id, reach)| {
                    view::admin_access(*reach, &qualified).map(|marker| (*id, marker))
                })
                .collect()
        }
    };
    let documents = entries
        .iter()
        .map(|(row, listed)| {
            let owner = cast
                .person(row.owner_id)
                .ok_or_else(|| ApiError::internal("the owner of a document is not there"))?;
            let node = cast
                .node(row.node_id)
                .ok_or_else(|| ApiError::internal("the node of a document is not there"))?;
            let made_by_agent =
                cast.author(None, row.created_by_thread, row.created_by_agent.as_deref());
            let created_by = match made_by_agent {
                Some(author) => author,
                None => Author::User(
                    cast.person(row.created_by.unwrap_or(row.owner_id))
                        .ok_or_else(|| {
                            ApiError::internal("the maker of a document is not there")
                        })?,
                ),
            };
            let updated_by = match row.updated_by {
                Some(user) => cast.author(Some(user), None, None),
                None => agents
                    .get(&row.id)
                    .and_then(|(thread, agent)| cast.author(None, Some(*thread), Some(agent))),
            };
            Ok(DocumentRef {
                admin_access: admin_markers.get(&row.id).cloned(),
                id: row.id,
                title: row.title.clone(),
                description: row.description.clone(),
                description_line: lines.get(&row.id).cloned().unwrap_or_default(),
                node,
                owner: owner.clone(),
                owners: owners.get(&row.id).copied().unwrap_or(0),
                created_by,
                page: page_state(listed.state),
                seq: row.doc_seq,
                block_count: i64::from(row.block_count),
                bytes: row.bytes,
                files_bytes: row.files_bytes,
                version: i64::from(row.version),
                open_discussions: open.get(&row.id).copied().unwrap_or(0),
                updated_at: Time::new(row.updated_at),
                updated_by,
                discussion_level: if levels.get(&row.id).is_some_and(|level| level == "all") {
                    orch_api_contract::documents::DocumentDiscussionLevel::All
                } else {
                    orch_api_contract::documents::DocumentDiscussionLevel::Involved
                },
                seen_seq: seen.get(&row.id).map(|(seq, _)| *seq),
                archived: listed.archived_node.is_some(),
                deleted: row.deleted_at.is_some(),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let mut sources = BTreeSet::new();
    for document in &documents {
        named_node_source(&document.node, &mut sources);
        for author in [Some(&document.created_by), document.updated_by.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Author::Agent(agent) = author {
                named_node_source(&agent.thread, &mut sources);
            }
        }
        if let Some(marker) = &document.admin_access
            && let orch_api_contract::nodes::NodeOrSystem::Node(node) = &marker.place
            && node.name.is_some()
            && let Some(graph) = &marker_graph
            && let Some(place) = graph.get(node.id)
            && !place
                .path
                .iter()
                .any(|id| graph.get(*id).is_some_and(|node| node.deleted))
        {
            for id in &place.path {
                if let Some(node) = cast.node(*id) {
                    named_node_source(&node, &mut sources);
                }
            }
        }
    }
    Ok((documents, sources.into_iter().collect()))
}

fn named_node_source(node: &NodeRef, sources: &mut BTreeSet<WrittenLabelSource>) {
    if node.name.is_some() || node.thread_display.is_some() {
        sources.insert(WrittenLabelSource::Node(node.id));
    }
}

/// The existing document-reference projection with its actual protected external node names.
#[derive(Debug)]
pub(crate) struct ProjectedDocumentRef {
    pub(crate) document: DocumentRef,
    sources: Vec<WrittenLabelSource>,
}

impl ProjectedDocumentRef {
    /// Parent and stored agent-attribution identities, plus qualified provenance path identities.
    pub(crate) async fn candidates(
        conn: &mut Conn,
        row: &DocumentRow,
        listed: &Listed,
    ) -> ApiResult<Vec<WrittenLabelSource>> {
        Ok(
            Self::candidates_many(conn, &[(row.clone(), listed.clone())])
                .await?
                .remove(&row.id)
                .unwrap_or_default(),
        )
    }

    /// The same raw parent, last-agent and inspection provenance in one native batch.
    pub(crate) async fn candidates_many(
        conn: &mut Conn,
        entries: &[(DocumentRow, Listed)],
    ) -> ApiResult<BTreeMap<Id, Vec<WrittenLabelSource>>> {
        if entries.is_empty() {
            return Ok(BTreeMap::new());
        }
        let attributed: Vec<_> = entries
            .iter()
            .filter(|(row, _)| row.updated_by.is_none() && row.doc_seq > 0)
            .map(|(row, _)| row.id)
            .collect();
        let agents = if attributed.is_empty() {
            HashMap::new()
        } else {
            rows::last_agents(conn, &attributed).await?
        };
        let marker_ids: Vec<_> = entries
            .iter()
            .filter_map(|(_, listed)| listed.admin_access.and_then(|reach| reach.place.node()))
            .collect();
        let graph = orch_authz::engine::Graph::load(conn, &marker_ids)
            .await
            .map_err(ApiError::internal)?;
        Ok(entries
            .iter()
            .map(|(row, listed)| {
                let mut nodes = BTreeSet::from([row.node_id]);
                if row.created_by_agent.is_some() {
                    nodes.extend(row.created_by_thread);
                }
                if row.updated_by.is_none() && row.doc_seq > 0 {
                    nodes.extend(agents.get(&row.id).map(|(thread, _)| *thread));
                }
                if let Some(id) = listed.admin_access.and_then(|reach| reach.place.node())
                    && let Some(node) = graph.get(id)
                {
                    nodes.extend(node.path.iter().copied());
                }
                (
                    row.id,
                    nodes.into_iter().map(WrittenLabelSource::Node).collect(),
                )
            })
            .collect())
    }

    /// Public UserRef identities and stored document/agent text retain their existing meaning.
    /// Disallowed protected node names become typed identity-only refs before construction.
    pub(crate) async fn capture_allowed(
        conn: &mut Conn,
        principal: &Principal,
        row: &DocumentRow,
        listed: &Listed,
        now: DateTime<Utc>,
        policy: Policy,
        allowed: &[WrittenLabelSource],
    ) -> ApiResult<Self> {
        let (mut documents, sources) = Self::capture_many_allowed(
            conn,
            principal,
            &[(row.clone(), listed.clone())],
            now,
            policy,
            allowed,
        )
        .await?;
        let document = documents
            .pop()
            .ok_or_else(|| ApiError::internal("a document could not be shown"))?;
        Ok(Self { document, sources })
    }

    /// The ordinary native batch projection with its actual protected source identities.
    pub(crate) async fn capture_many(
        conn: &mut Conn,
        principal: &Principal,
        entries: &[(DocumentRow, Listed)],
        now: DateTime<Utc>,
        policy: Policy,
    ) -> ApiResult<(Vec<DocumentRef>, Vec<WrittenLabelSource>)> {
        refs_with_name_sources(conn, principal, entries, now, policy, None).await
    }

    /// One native batch projection and its actual named sources, under the same trusted
    /// source set. Lists must not rebuild the owner/count/author/Cast queries per document.
    pub(crate) async fn capture_many_allowed(
        conn: &mut Conn,
        principal: &Principal,
        entries: &[(DocumentRow, Listed)],
        now: DateTime<Utc>,
        policy: Policy,
        allowed: &[WrittenLabelSource],
    ) -> ApiResult<(Vec<DocumentRef>, Vec<WrittenLabelSource>)> {
        let allowed = allowed.iter().copied().collect();
        refs_with_name_sources(conn, principal, entries, now, policy, Some(&allowed)).await
    }

    pub(crate) fn sources(&self) -> &[WrittenLabelSource] {
        &self.sources
    }
}

/// A document without its blocks, as the caller reads it.
pub async fn reference(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
    listed: &Listed,
    now: DateTime<Utc>,
    policy: Policy,
) -> ApiResult<DocumentRef> {
    refs(
        conn,
        principal,
        &[(row.clone(), listed.clone())],
        now,
        policy,
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| ApiError::internal("a document could not be shown"))
}

/// The files of a document as a reader is shown them.
fn files_for(files: &[FileRow], cast: &Cast) -> Vec<DocumentFile> {
    files
        .iter()
        .map(|file| DocumentFile {
            id: file.id,
            name: file.name.clone(),
            media_type: file.media_type.clone(),
            size: file.size,
            is_text: file.is_text,
            // Files retain the actual writer thread without a profile-name snapshot.
            // Keep that known agent identity; today's bound profile could be another writer.
            added_by: cast.author(file.added_by, file.added_by_thread, Some("Agent")),
            added_at: Time::new(file.added_at),
        })
        .collect()
}

/// Current protected label bounds for one full document projection.
#[derive(Default)]
pub(crate) struct DocumentLabels<'a> {
    pub(crate) names: Option<&'a crate::routes::access::page::NameBounds>,
    pub(crate) sources: Option<&'a [WrittenLabelSource]>,
}

/// A document, as its reader is shown it: its node's way down, its files, what
/// it may hold, and what the caller holds on it.
pub async fn document(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
    listed: &Listed,
    now: DateTime<Utc>,
    policy: Policy,
) -> ApiResult<Document> {
    document_bounded(
        conn,
        principal,
        row,
        listed,
        now,
        policy,
        DocumentLabels::default(),
    )
    .await
}

/// The same full document producer with existing typed name bounds; stored content is unchanged.
pub(crate) async fn document_bounded(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
    listed: &Listed,
    now: DateTime<Utc>,
    policy: Policy,
    labels: DocumentLabels<'_>,
) -> ApiResult<Document> {
    let DocumentLabels { names, sources } = labels;
    let names = names.filter(|names| names.limited);
    let base = match sources.filter(|_| names.is_some()) {
        Some(sources) => {
            ProjectedDocumentRef::capture_allowed(
                conn, principal, row, listed, now, policy, sources,
            )
            .await?
            .document
        }
        None => reference(conn, principal, row, listed, now, policy).await?,
    };
    let admin_access = base.admin_access.clone();
    let files = rows::files(conn, row.id).await?;
    let path = match names {
        Some(names) => {
            view::ways_with_name_sources(conn, principal, &[row.node_id], policy, &names.nodes)
                .await?
        }
        None => view::ways(conn, principal, &[row.node_id], policy).await?,
    }
    .remove(&row.node_id)
    .unwrap_or_default();
    let mut wanted = Wanted::default();
    for file in &files {
        wanted.users.extend(file.added_by);
        wanted.nodes.extend(file.added_by_thread);
    }
    if let Some(id) = listed
        .admin_place
        .and_then(orch_access_contract::roles::PlaceRef::node)
    {
        wanted.node(id);
    }
    let allowed = sources
        .filter(|_| names.is_some())
        .map(|sources| sources.iter().copied().collect());
    let cast = Cast::load_for_labels(conn, principal, wanted, policy, allowed.as_ref()).await?;
    let act_places: Vec<Id> = listed
        .admin_place
        .and_then(orch_access_contract::roles::PlaceRef::node)
        .into_iter()
        .collect();
    let act_places = match names {
        Some(names) => names.qualified_refs(conn, principal, &act_places).await?,
        None => view::qualified_refs(conn, principal, &act_places, policy).await?,
    };
    let settings = DocSettings::at(conn, row.node_id).await?;
    let location = match names {
        Some(names) => {
            crate::routes::document_tree::metadata::location_with_name_sources(
                conn,
                principal,
                row,
                Time::new(now),
                &names.containers,
            )
            .await?
        }
        None => {
            crate::routes::document_tree::metadata::location(conn, principal, row, Time::new(now))
                .await?
        }
    };
    let permissions: Vec<String> = listed
        .held
        .iter()
        .map(|held| held.permission.as_str().to_owned())
        .filter(|name| name.starts_with("doc.") || name.starts_with("access."))
        .collect();
    Ok(Document {
        // A failed independent reader remains available through its ordinary endpoint;
        // it must not prevent reading the admitted document and its blocks.
        link_settings: if row.deleted_at.is_none() {
            crate::routes::links::tabs(conn, principal, row).await.ok()
        } else {
            None
        },
        inspection: None,
        base,
        path,
        location,
        files: files_for(&files, &cast),
        limits: settings.shown(),
        created_at: Time::new(row.created_at),
        access: DocumentAccess {
            admin_place: (!listed.admin_only.is_empty())
                .then_some(listed.admin_place)
                .flatten()
                .and_then(|place| view::admin_place(place, &act_places)),
            admin_only: listed
                .admin_only
                .iter()
                .map(|permission| permission.as_str().to_owned())
                .collect(),
            admin_access,
            participating: listed.participating,
            permissions,
            administers: listed.administers,
            owns: listed.owner,
        },
    })
}

/// Recheck only the existing native visibility of the exact materialized label sources.
/// This produces no replacement blocks/comments and grants no operation permission.
pub(crate) async fn label_sources_visible(
    ctx: &crate::registry::Ctx,
    conn: &mut Conn,
    principal: &Principal,
    sources: &[WrittenLabelSource],
) -> ApiResult<bool> {
    let nodes = sources
        .iter()
        .filter_map(|source| match source {
            WrittenLabelSource::Node(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let visible = view::reference_visibility(conn, principal, &nodes, Policy::default()).await?;
    if !nodes.iter().all(|id| visible.contains(id)) {
        return Ok(false);
    }
    for source in sources {
        match source {
            WrittenLabelSource::Node(_) => {}
            WrittenLabelSource::Document(id) => {
                match crate::middleware::authorization::require_document(
                    conn,
                    principal,
                    *id,
                    Permission::DocView,
                    Policy::default(),
                )
                .await
                {
                    Ok(()) => {}
                    Err(error) if error.problem.status < 500 => return Ok(false),
                    Err(error) => return Err(error),
                }
            }
            WrittenLabelSource::Chat(id) => {
                let origin = crate::sign_in::origin(ctx);
                let actor = orch_chat::Acting {
                    principal,
                    now: principal.now().get(),
                    origin: &origin,
                };
                match orch_chat::chats::load(conn, &actor, *id, orch_chat::chats::Mode::Read).await
                {
                    Ok(_) => {}
                    Err(orch_chat::Error::Db(error)) => return Err(error.into()),
                    Err(_) => return Ok(false),
                }
            }
        }
    }
    Ok(true)
}
