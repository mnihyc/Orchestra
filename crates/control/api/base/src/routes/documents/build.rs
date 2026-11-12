//! A document and its parts as a reader is shown them: names put in the place of identifiers, and what the reader may not see left without a name.
//!
//! The database keeps identifiers: the person who changed a block, the thread
//! of the agent that did, the person a mention names, the node or the document
//! a chip points at. A reader is given names, but only where they may know
//! them: a thread they cannot see is named `null` and marked redacted, and so
//! is a chip to a document they cannot read. This is looked up once for a
//! whole answer, in a few statements however many blocks it holds.

use std::collections::BTreeMap;

pub use std::collections::BTreeSet;

pub use std::collections::HashMap;

use crate::middleware::authorization::refusals::user_refs;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

use crate::routes::documents::limits::DocSettings;

pub use crate::routes::nodes::view;

pub use chrono::DateTime;

pub use chrono::Utc;

pub use orch_access_contract::permissions::Permission;

pub use orch_api_contract::documents::Author;

use orch_api_contract::documents::AuthorAgent;

use orch_api_contract::documents::BlockContent;

use orch_api_contract::documents::DOCUMENT_DESCRIPTION_MAX_CHARS;

use orch_api_contract::documents::DOCUMENT_LINE_MAX_CHARS;

use orch_api_contract::documents::DiscussionResolved;

pub use orch_api_contract::documents::Document;

use orch_api_contract::documents::DocumentAccess;

pub use orch_api_contract::documents::DocumentBlock;

pub use orch_api_contract::documents::DocumentComment;

pub use orch_api_contract::documents::DocumentDiscussion;

pub use orch_api_contract::documents::DocumentRef;

pub use orch_api_contract::documents::Inline;

pub use orch_api_contract::documents::InlineRefKind;

use orch_api_contract::documents::TableCell;

use orch_api_contract::documents::TableRow;

pub use orch_api_contract::nodes::NodeRef;

use orch_api_contract::users::UserRef;

pub use orch_authz::documents::Listed;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_db::queries::documents::discussions::CommentRow;

pub use orch_db::queries::documents::discussions::Full;

pub use orch_db::queries::documents as rows;

pub use orch_db::queries::documents::BlockRow;

pub use orch_db::queries::documents::DocumentRow;

pub use orch_documents::blocks;

use orch_documents::comments;

use orch_documents::describe;

use orch_documents::position::Position;

pub use orch_types::Id;

pub use orch_types::Time;
/// The resource whose already-readable label is copied into a newly written reference.
/// Attached files retain their actual document/chat owner from the ordinary Cast producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WrittenLabelSource {
    /// A node's current name.
    Node(Id),
    /// A document's title or an attached file's name.
    Document(Id),
    /// A chat's current name or a file carried by one of its messages.
    Chat(Id),
}

orch_api_base::api_family_callback! {
    bind __api_bind_5c43bf1498, __api_install_5c43bf1498, __api_callback_5c43bf1498;
    slot __ApiCallback5c43bf1498, __API_CALLBACK_5C43BF1498, __api_set_5c43bf1498;
    base orch_api_base::routes::documents::build;

    async fn scoped_references<'__api>(arg0: &'__api mut orch_db::Conn, arg1: Vec<orch_api_contract::documents::Inline>, arg2: &'__api std::collections::BTreeSet<orch_api_base::routes::documents::build::WrittenLabelSource>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::documents::Inline>>;
}

pub fn visit_inline(content: &BlockContent, mut visit: impl FnMut(&Inline)) {
    match content {
        BlockContent::Inline(runs) => runs.iter().for_each(&mut visit),
        BlockContent::Rows(rows) => {
            for cell in rows.iter().flat_map(|row| &row.cells) {
                cell.content.iter().for_each(&mut visit);
            }
        }
    }
}

/// What a reader's answer is going to name.
#[derive(Debug, Default)]
pub struct Wanted {
    pub users: Vec<Id>,
    pub nodes: Vec<Id>,
    pub documents: Vec<Id>,
    pub chats: Vec<Id>,
    pub files: Vec<Id>,
    pub environments: Vec<Id>,
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

    pub fn comment_people(&mut self, row: &CommentRow) {
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
pub struct ReferenceFileRows {
    pub attached: Vec<(Id, Id, String)>,
    pub carried: Vec<(Id, Id, i32, orch_types::Json)>,
}

impl ReferenceFileRows {
    /// Governing identities only: no filename or carried message content is read before scope filtering.
    pub async fn sources(
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

    pub async fn load(
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
    pub users: HashMap<Id, UserRef>,
    pub nodes: HashMap<Id, NodeRef>,
    pub titles: HashMap<Id, Option<String>>,
    /// The name of each chat a chip names, where the reader may read the chat: its own, or its place's.
    pub chats: HashMap<Id, Option<String>>,
    pub chat_name_nodes: HashMap<Id, Id>,
    pub files: HashMap<Id, String>,
    pub file_sources: HashMap<Id, WrittenLabelSource>,
    pub environments: HashMap<Id, String>,
    pub reference_sources: Option<BTreeSet<WrittenLabelSource>>,
}

impl Cast {
    /// Existing live labels selected by this native Cast, before any block or comment DTO is built.
    pub fn live_sources<'a>(
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
    pub async fn for_blocks(
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
    pub async fn for_comments(
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

    pub async fn load_for_labels(
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

    pub fn materialized_source(
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

    pub fn reference_name(
        &self,
        kind: InlineRefKind,
        id: Id,
        path: Option<&str>,
    ) -> Option<String> {
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

    pub fn content(&self, content: BlockContent) -> BlockContent {
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

orch_api_base::api_family_callback! {
    bind __api_bind_de17b89813, __api_install_de17b89813, __api_callback_de17b89813;
    slot __ApiCallbackde17b89813, __API_CALLBACK_DE17B89813, __api_set_de17b89813;
    base orch_api_base::routes::documents::build;

    fn block_type_of<'__api>(arg0: &'__api str) -> orch_api_contract::documents::BlockType;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8529c110e3, __api_install_8529c110e3, __api_callback_8529c110e3;
    slot __ApiCallback8529c110e3, __API_CALLBACK_8529C110E3, __api_set_8529c110e3;
    base orch_api_base::routes::documents::build;

    async fn document_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::documents::DocumentRow, arg2: &'__api orch_authz::documents::Listed) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::access::page::NameSources>;
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

orch_api_base::api_family_callback! {
    bind __api_bind_e7a615ba5e, __api_install_e7a615ba5e, __api_callback_e7a615ba5e;
    slot __ApiCallbacke7a615ba5e, __API_CALLBACK_E7A615BA5E, __api_set_e7a615ba5e;
    base orch_api_base::routes::documents::build;

    async fn refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [(orch_db::queries::documents::DocumentRow, orch_authz::documents::Listed)], arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::documents::DocumentRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1fc6486326, __api_install_1fc6486326, __api_callback_1fc6486326;
    slot __ApiCallback1fc6486326, __API_CALLBACK_1FC6486326, __api_set_1fc6486326;
    base orch_api_base::routes::documents::build;

    async fn refs_with_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [(orch_db::queries::documents::DocumentRow, orch_authz::documents::Listed)], arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy, arg5: Option<&'__api std::collections::BTreeSet<orch_api_base::routes::documents::build::WrittenLabelSource>>) -> orch_api_base::middleware::requests::ApiResult<(Vec<orch_api_contract::documents::DocumentRef>, Vec<orch_api_base::routes::documents::build::WrittenLabelSource>)>;
}

/// The existing document-reference projection with its actual protected external node names.
#[derive(Debug)]
pub struct ProjectedDocumentRef {
    pub document: DocumentRef,
    pub sources: Vec<WrittenLabelSource>,
}

impl ProjectedDocumentRef {
    /// Parent and stored agent-attribution identities, plus qualified provenance path identities.
    pub async fn candidates(
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
    pub async fn candidates_many(
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
    pub async fn capture_allowed(
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
    pub async fn capture_many(
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
    pub async fn capture_many_allowed(
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

    pub fn sources(&self) -> &[WrittenLabelSource] {
        &self.sources
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_b5e18a5d64, __api_install_b5e18a5d64, __api_callback_b5e18a5d64;
    slot __ApiCallbackb5e18a5d64, __API_CALLBACK_B5E18A5D64, __api_set_b5e18a5d64;
    base orch_api_base::routes::documents::build;

    async fn reference<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::documents::DocumentRow, arg3: &'__api orch_authz::documents::Listed, arg4: chrono::DateTime<chrono::Utc>, arg5: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::documents::DocumentRef>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_839e75b1a6, __api_install_839e75b1a6, __api_callback_839e75b1a6;
    slot __ApiCallback839e75b1a6, __API_CALLBACK_839E75B1A6, __api_set_839e75b1a6;
    base orch_api_base::routes::documents::build;

    fn files_for<'__api>(arg0: &'__api [orch_db::queries::documents::FileRow], arg1: &'__api orch_api_base::routes::documents::build::Cast) -> Vec<orch_api_contract::documents::DocumentFile>;
}

/// Current protected label bounds for one full document projection.
#[derive(Default)]
pub struct DocumentLabels<'a> {
    pub names: Option<&'a crate::routes::access::page::NameBounds>,
    pub sources: Option<&'a [WrittenLabelSource]>,
}

/// The same full document producer with existing typed name bounds; stored content is unchanged.
pub async fn document_bounded(
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
