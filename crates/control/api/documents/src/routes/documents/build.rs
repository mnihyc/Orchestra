//! A document and its parts as a reader is shown them: names put in the place of identifiers, and what the reader may not see left without a name.
//!
//! The database keeps identifiers: the person who changed a block, the thread
//! of the agent that did, the person a mention names, the node or the document
//! a chip points at. A reader is given names, but only where they may know
//! them: a thread they cannot see is named `null` and marked redacted, and so
//! is a chip to a document they cannot read. This is looked up once for a
//! whole answer, in a few statements however many blocks it holds.

use std::collections::BTreeSet;

use std::collections::HashMap;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::routes::nodes::view;

use chrono::DateTime;

use chrono::Utc;

use orch_access_contract::pages::PageState as StoredPageState;

use orch_access_contract::permissions::Permission;

use orch_api_contract::documents::Author;

use orch_api_contract::documents::BlockType;

use orch_api_contract::documents::Document;

use orch_api_contract::documents::DocumentBlock;

use orch_api_contract::documents::DocumentComment;

use orch_api_contract::documents::DocumentDiscussion;

use orch_api_contract::documents::DocumentFile;

use orch_api_contract::documents::DocumentRef;

use orch_api_contract::documents::Inline;

use orch_api_contract::documents::InlineRefKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::nodes::PageState;

use orch_authz::documents::Listed;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::documents::discussions::CommentRow;

use orch_db::queries::documents::discussions::Full;

use orch_db::queries::documents as rows;

use orch_db::queries::documents::BlockRow;

use orch_db::queries::documents::DocumentRow;

use orch_db::queries::documents::FileRow;

use orch_documents::blocks;

use orch_types::Id;

use orch_types::Time;

pub use orch_api_base::routes::documents::build::WrittenLabelSource;

pub use orch_api_base::routes::documents::build::visit_inline;

pub use orch_api_base::routes::documents::build::Wanted;

pub use orch_api_base::routes::documents::build::ReferenceFileRows;

pub use orch_api_base::routes::documents::build::Cast;

pub use orch_api_base::routes::documents::build::lines_for;

pub use orch_api_base::routes::documents::build::ProjectedDocumentRef;

pub use orch_api_base::routes::documents::build::DocumentLabels;

pub use orch_api_base::routes::documents::build::document_bounded;
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
orch_api_base::__api_bind_5c43bf1498!();

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

/// The type of a stored block; a word that is none of the twenty reads as a
/// paragraph, which nothing stored here is.
#[must_use]
pub fn block_type_of(word: &str) -> BlockType {
    serde_json::from_value(serde_json::Value::String(word.to_owned()))
        .unwrap_or(BlockType::Paragraph)
}
orch_api_base::__api_bind_de17b89813!();

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
orch_api_base::__api_bind_8529c110e3!();

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
orch_api_base::__api_bind_e7a615ba5e!();

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
orch_api_base::__api_bind_1fc6486326!();

fn named_node_source(node: &NodeRef, sources: &mut BTreeSet<WrittenLabelSource>) {
    if node.name.is_some() || node.thread_display.is_some() {
        sources.insert(WrittenLabelSource::Node(node.id));
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
orch_api_base::__api_bind_b5e18a5d64!();

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
orch_api_base::__api_bind_839e75b1a6!();

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
