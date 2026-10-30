//! A document and its parts as a reader is shown them: names put in the place of identifiers, and what the reader may not see left without a name.
//!
//! The database keeps identifiers: the person who changed a block, the thread
//! of the agent that did, the person a mention names, the node or the document
//! a chip points at. A reader is given names, but only where they may know
//! them: a thread they cannot see is named `null` and marked redacted, and so
//! is a chip to a document they cannot read. This is looked up once for a
//! whole answer, in a few statements however many blocks it holds.

use std::collections::HashMap;

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
use orch_api_contract::nodes::{FieldSource, NodeRef, PageState};
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

/// Captures missing labels from the author's current readable names before storing newly written content.
pub async fn label_written<T: serde::Serialize + serde::de::DeserializeOwned>(
    conn: &mut Conn,
    principal: &Principal,
    content: T,
) -> ApiResult<T> {
    fn references(value: &serde_json::Value, out: &mut Vec<Inline>) {
        match value {
            serde_json::Value::Object(object)
                if object.get("type").and_then(serde_json::Value::as_str) == Some("ref") =>
            {
                if let Ok(reference) = serde_json::from_value(value.clone()) {
                    out.push(reference);
                }
            }
            serde_json::Value::Object(object) => {
                for value in object.values() {
                    references(value, out);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    references(value, out);
                }
            }
            _ => {}
        }
    }
    fn fill(
        value: &mut serde_json::Value,
        labels: &HashMap<(InlineRefKind, Id, Option<String>), String>,
    ) {
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
    let mut refs = Vec::new();
    references(&value, &mut refs);
    if !refs.is_empty() {
        let mut wanted = Wanted::default();
        wanted.runs(&refs);
        let cast = Cast::load(conn, principal, wanted, Policy::default()).await?;
        let labels = cast
            .expand(refs)
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Ref {
                    kind,
                    id,
                    path,
                    name: Some(Some(name)),
                    ..
                } => Some(((kind, id, path), name)),
                _ => None,
            })
            .collect();
        fill(&mut value, &labels);
    }
    serde_json::from_value(value).map_err(ApiError::internal)
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

    /// Who wrote a comment, who reacted to it, and what it names.
    pub fn comment(&mut self, row: &CommentRow) {
        self.users.push(row.author_id);
        for each in row.reaction_list() {
            self.users.extend(each.users);
        }
        if let Ok(runs) = serde_json::from_value::<Vec<Inline>>(row.content.clone()) {
            self.runs(&runs);
        }
    }

    /// Who started a discussion, who resolved it, and what is said in it.
    pub fn discussion(&mut self, full: &Full) {
        self.users.push(full.discussion.created_by);
        self.users.extend(full.discussion.resolved_by);
        for each in &full.comments {
            self.comment(each);
        }
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
    files: HashMap<Id, String>,
    environments: HashMap<Id, String>,
}

impl Cast {
    /// Looks up everything `wanted` names, for `principal`.
    pub async fn load(
        conn: &mut Conn,
        principal: &Principal,
        mut wanted: Wanted,
        policy: Policy,
    ) -> ApiResult<Self> {
        let attached: Vec<(Id, Id, String)> =
            sqlx::query_as("SELECT id,document_id,name FROM document_files WHERE id=ANY($1)")
                .bind(&wanted.files)
                .fetch_all(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        wanted
            .documents
            .extend(attached.iter().map(|(_, owner, _)| *owner));
        let carried: Vec<(Id, Id, i32, orch_types::Json)> = sqlx::query_as("SELECT f.id,f.chat_id,f.carried,m.content FROM chat_file_references f JOIN chat_messages m ON m.id=f.message_id WHERE f.id=ANY($1) AND m.deleted_at IS NULL")
            .bind(&wanted.files).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
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
        let nodes = view::refs(conn, principal, &wanted.nodes, policy).await?;
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
        if !wanted.chats.is_empty() {
            let rows = orch_db::queries::chats::chats(conn, &wanted.chats).await?;
            let readable = orch_authz::chats::listed_access(conn, principal, &rows)
                .await
                .map_err(ApiError::internal)?;
            let places: Vec<Id> = rows.iter().filter_map(|row| row.node_id).collect();
            let named = view::refs(conn, principal, &places, policy).await?;
            for row in rows {
                let name = row.name.clone().or_else(|| {
                    row.node_id
                        .and_then(|node| named.get(&node))
                        .and_then(|node| node.name.clone())
                });
                chats.insert(row.id, name.filter(|_| readable.contains_key(&row.id)));
            }
        }
        let mut files = HashMap::new();
        for (id, owner, name) in attached {
            if titles.get(&owner).is_some_and(Option::is_some) {
                files.insert(id, name);
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
            files,
            environments,
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
                    let name = match kind {
                        InlineRefKind::Node => self.node(id).and_then(|node| node.name),
                        InlineRefKind::Document => self.titles.get(&id).cloned().flatten(),
                        InlineRefKind::Chat => self.chats.get(&id).cloned().flatten(),
                        InlineRefKind::File => self.files.get(&id).cloned(),
                        InlineRefKind::EnvironmentFile => self
                            .environments
                            .get(&id)
                            .and(path.as_ref())
                            .map(|path| path.rsplit('/').next().unwrap_or(path).to_owned()),
                    };
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

/// Blocks as a reader is shown them, with the names looked up once.
pub async fn blocks_for(
    conn: &mut Conn,
    principal: &Principal,
    list: &[BlockRow],
    policy: Policy,
) -> ApiResult<Vec<DocumentBlock>> {
    let mut wanted = Wanted::default();
    for row in list {
        wanted.block(row);
    }
    let cast = Cast::load(conn, principal, wanted, policy).await?;
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
    let mut wanted = Wanted::default();
    for full in fulls {
        wanted.discussion(full);
    }
    let cast = Cast::load(conn, principal, wanted, policy).await?;
    let about = blocks_about(conn, document, fulls).await?;
    Ok(fulls
        .iter()
        .filter_map(|full| cast.discussion(full, &about))
        .collect())
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

/// Who maintains a document's description, as the API words it.
#[must_use]
pub fn description_source(word: &str) -> FieldSource {
    if word == "user" {
        FieldSource::User
    } else {
        FieldSource::Auto
    }
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
    if entries.is_empty() {
        return Ok(Vec::new());
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
    let cast = Cast::load(conn, principal, wanted, policy).await?;
    let marker_places: HashMap<_, _> = entries
        .iter()
        .filter_map(|(row, listed)| listed.admin_access.map(|place| (row.id, place)))
        .collect();
    let admin_markers = view::named_admin_access(conn, principal, &marker_places).await?;
    entries
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
                description_source: description_source(&row.description_source),
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
        .collect()
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
    let base = reference(conn, principal, row, listed, now, policy).await?;
    let admin_access = base.admin_access.clone();
    let files = rows::files(conn, row.id).await?;
    let path = view::ways(conn, principal, &[row.node_id], policy)
        .await?
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
    let cast = Cast::load(conn, principal, wanted, policy).await?;
    let act_places: Vec<Id> = listed
        .admin_place
        .and_then(orch_access_contract::roles::PlaceRef::node)
        .into_iter()
        .collect();
    let act_places = view::qualified_refs(conn, principal, &act_places, policy).await?;
    let settings = DocSettings::at(conn, row.node_id).await?;
    let location =
        crate::routes::document_tree::metadata::location(conn, principal, row, Time::new(now))
            .await?;
    let permissions: Vec<String> = listed
        .held
        .iter()
        .map(|held| held.permission.as_str().to_owned())
        .filter(|name| name.starts_with("doc.") || name.starts_with("access."))
        .collect();
    Ok(Document {
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
