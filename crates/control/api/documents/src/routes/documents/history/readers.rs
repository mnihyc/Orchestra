//! Actual version windows share current document authority and the existing block/author producer.

use super::authors_of;

use super::entry;

use super::from_version;

use super::open_span;

use super::parse_author;

use super::state_at;

use super::version_number;

use super::want_writers;

use super::writers_of;

use super::wrote_in;

use crate::key_operations::native;

use crate::middleware::inspection;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::page::NameBounds;

use crate::routes::documents::DocumentRow;

use crate::routes::documents::build;

use crate::routes::documents::read_operation;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::PageQuery;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::HistorySource;

use orch_api_contract::documents::DocumentBlock;

use orch_api_contract::documents::DocumentChanges;

use orch_api_contract::documents::DocumentChangesGetQuery;

use orch_api_contract::documents::DocumentVersionGetReply;

use orch_api_contract::documents::DocumentVersionListQuery;

use orch_api_contract::documents::{DocumentHistoryRetention, DocumentVersionListReply};

use orch_api_contract::nodes::DeletedInspectionQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::documents::BlockRow;

use orch_db::queries::documents::history;

use orch_documents::changes;

use orch_documents::versions::Span;

use orch_documents::versions::Writer;

use orch_types::Id;

use orch_types::Time;

use orch_settings::{Place, Snapshot, Wanted};

use orch_settings_contract::provenance::Source;

use orch_settings_contract::registry::{self, keys};

use std::collections::HashMap;

use std::sync::Arc;
enum Query {
    List(DocumentVersionListQuery),
    Version,
    Changes(DocumentChangesGetQuery),
}

impl Query {
    fn declaration(&self, inspect: bool) -> &'static Declaration<HistorySource> {
        let family = match self {
            Self::List(_) => &sources::VERSIONS,
            Self::Version => &sources::VERSION,
            Self::Changes(_) => &sources::CHANGES,
        };
        if inspect {
            &family.inspect
        } else {
            &family.read
        }
    }
}

enum Rows {
    List {
        title: String,
        bytes: i64,
        block_count: i64,
        span: Span,
        open: bool,
        versions: Vec<history::VersionRow>,
        next: Option<String>,
        retention: Option<RetentionRows>,
    },
    Version {
        version: history::VersionRow,
        blocks: Vec<BlockRow>,
    },
    Changes {
        from: i32,
        to: Option<i32>,
        earlier: Vec<BlockRow>,
        later: Vec<BlockRow>,
        earlier_title: Option<String>,
        later_title: Option<String>,
        tombstones: HashMap<Id, BlockRow>,
        cast_blocks: Vec<BlockRow>,
        writers: Vec<Writer>,
    },
}

struct RetentionRows {
    oldest: Option<Time>,
    versions: i64,
    bytes: i64,
    version_source: Source<Id>,
    byte_source: Source<Id>,
    measured_at: Time,
}

impl Rows {
    fn source_nodes(&self) -> Vec<Id> {
        match self {
            Self::List {
                retention: Some(retention),
                ..
            } => [retention.version_source.node, retention.byte_source.node]
                .into_iter()
                .flatten()
                .collect(),
            _ => Vec::new(),
        }
    }
    fn blocks(&self) -> &[BlockRow] {
        match self {
            Self::List { .. } => &[],
            Self::Version { blocks, .. } => blocks,
            Self::Changes { cast_blocks, .. } => cast_blocks,
        }
    }
    /// Only authors of entries actually returned belong to the response source set.
    fn writers(&self) -> Vec<Writer> {
        match self {
            Self::List {
                span,
                open,
                versions,
                ..
            } => {
                let mut writers = if *open {
                    span.authors.clone()
                } else {
                    Vec::new()
                };
                writers.extend(versions.iter().flat_map(|row| writers_of(&row.authors)));
                writers
            }
            Self::Version { version, .. } => writers_of(&version.authors),
            Self::Changes { writers, .. } => writers.clone(),
        }
    }
}

struct PreparedPage {
    document: Id,
    rows: Rows,
    writers: Vec<Writer>,
    names: NameBounds,
    references: Vec<build::WrittenLabelSource>,
    inspection: Option<inspection::Info>,
    cut: crate::routes::document_tree::metadata::Cut,
}

enum Reply {
    List(DocumentVersionListReply),
    Version(DocumentVersionGetReply),
    Changes(DocumentChanges),
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentVersionListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, Query::List(query)))
}

pub(super) fn version<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: DeletedInspectionQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, Query::Version))
}

pub(super) fn changes<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentChangesGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, Query::Changes(query)))
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: Query,
) -> ApiResult<OperationPlan> {
    let document = ctx.id("id")?;
    let declaration = query.declaration(ctx.inspect_deleted);
    let read_operation::ReadRoot {
        mut plan,
        principal,
        inspection,
    } = read_operation::prepare_root(ctx, conn, caller).await?;
    let reads = inspection
        .as_ref()
        .map(|info| info.reads.clone())
        .unwrap_or_default();
    orch_db::queries::trace::read_scope(&reads, async {
        let row = crate::routes::documents::live(conn, document).await?;
        let rows = match query {
            Query::List(query) => list_rows(conn, &row, query, Time::new(principal.now())).await?,
            Query::Version => version_rows(conn, document, version_number(ctx)?).await?,
            Query::Changes(query) => changes_rows(conn, &principal, &row, query).await?,
        };
        let writers = rows.writers();
        let mut references = build::ProjectedBlocks::candidates(conn, rows.blocks()).await?;
        references.extend(writers.iter().filter_map(|writer| match writer {
            Writer::Agent { thread, .. } => Some(build::WrittenLabelSource::Node(*thread)),
            Writer::User(_) => None,
        }));
        references.extend(
            rows.source_nodes()
                .into_iter()
                .map(build::WrittenLabelSource::Node),
        );
        let (names, references) =
            read_operation::prepare_references(conn, &principal, &mut plan, document, references)
                .await?;
        let cut = crate::routes::document_tree::metadata::cut(
            conn,
            &principal,
            row.node_id,
            principal.now(),
        )
        .await?;
        let plan = declared(ctx, declaration, plan, document)?;
        Ok(plan.with_data(PreparedPage {
            document,
            rows,
            writers,
            names,
            references,
            inspection,
            cut,
        }))
    })
    .await
}

/// The existing native producer already resolved every required source, including
/// checked-empty reference roles. Reuse those facts without another graph lookup.
fn declared(
    ctx: &Ctx,
    declaration: &Declaration<HistorySource>,
    plan: OperationPlan,
    document: Id,
) -> ApiResult<OperationPlan> {
    if plan.variant().operation != declaration.operation {
        return Err(ApiError::internal("the native history variant changed"));
    }
    let mut facts: Vec<_> = declaration
        .bindings
        .iter()
        .map(|binding| native::Facts {
            source: binding.source,
            cardinality: binding.cardinality,
            groups: Vec::new(),
        })
        .collect();
    for facet in plan
        .shared()
        .required_facets()
        .map_err(crate::key_operations::admission::binding_error)?
    {
        let source = match facet.resource_kind {
            KeyTargetKind::Object(ObjectKind::Document)
                if facet.target.object.id == Some(document) =>
            {
                HistorySource::Document
            }
            KeyTargetKind::Object(ObjectKind::Document) => HistorySource::ReferencedDocuments,
            KeyTargetKind::Object(ObjectKind::Org) => HistorySource::Orgs,
            KeyTargetKind::Object(ObjectKind::Project) => HistorySource::Projects,
            KeyTargetKind::Object(ObjectKind::Topic) => HistorySource::Topics,
            KeyTargetKind::Object(ObjectKind::Thread) => HistorySource::Threads,
            KeyTargetKind::Identity(KeyIdentityKind::Chat) => HistorySource::ReferencedChats,
            _ => {
                return Err(ApiError::internal(
                    "the native history source is undeclared",
                ));
            }
        };
        let fact = facts
            .iter_mut()
            .find(|fact| fact.source == source)
            .ok_or_else(|| ApiError::internal("the history source row is missing"))?;
        if let Some((_, targets)) = fact
            .groups
            .iter_mut()
            .find(|(family, _)| *family == facet.resource_kind)
        {
            targets.push(facet.target);
        } else {
            fact.groups.push((facet.resource_kind, vec![facet.target]));
        }
    }
    native::bind(ctx, declaration, &facts)
}

/// The original open-span/page selection, including the page's lookahead and cursor.
async fn list_rows(
    conn: &mut Conn,
    row: &DocumentRow,
    query: DocumentVersionListQuery,
    measured_at: Time,
) -> ApiResult<Rows> {
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(paging.limit()).unwrap_or(usize::MAX);
    let author = parse_author(query.author.as_deref())?;
    let before: Option<i32> = match query.cursor.as_deref() {
        None => None,
        Some(text) => Some(text.parse().map_err(|_| {
            crate::routes::nodes::invalid("cursor", "is not a cursor of this list")
        })?),
    };
    let span = row.span();
    let open =
        before.is_none() && !span.is_empty() && author.is_none_or(|author| wrote_in(&span, author));
    let take = limit - usize::from(open);
    let found = history::list(
        conn,
        row.id,
        author,
        before,
        i64::try_from(take + 1).unwrap_or(i64::MAX),
    )
    .await?;
    let more = found.len() > take;
    let shown: Vec<_> = found.into_iter().take(take).collect();
    let next = more
        .then(|| shown.last().map(|each| each.version.to_string()))
        .flatten();
    Ok(Rows::List {
        title: row.title.clone(),
        bytes: row.bytes,
        block_count: i64::from(row.block_count),
        span,
        open,
        versions: shown,
        next,
        retention: retention_rows(conn, row, measured_at).await?,
    })
}

async fn retention_rows(
    conn: &mut Conn,
    row: &DocumentRow,
    measured_at: Time,
) -> ApiResult<Option<RetentionRows>> {
    let defs = [keys::DOCS_VERSIONS_KEPT, keys::DOCS_HISTORY_MAX_BYTES]
        .into_iter()
        .map(|key| {
            registry::get(key)
                .ok_or_else(|| ApiError::internal("a document retention setting is absent"))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let snapshot = match Snapshot::load(conn, Place::Node(row.node_id), Wanted::Keys(&defs)).await {
        Ok(snapshot) => snapshot,
        Err(orch_settings::Error::NotFound) => return Ok(None),
        Err(error) => return Err(ApiError::internal(error)),
    };
    let versions = snapshot.resolve(defs[0]);
    let bytes = snapshot.resolve(defs[1]);
    let number = |value: &Option<orch_types::Json>| {
        value
            .as_ref()
            .and_then(orch_types::Json::as_i64)
            .ok_or_else(|| ApiError::internal("a document retention allowance is not a number"))
    };
    Ok(Some(RetentionRows {
        oldest: history::oldest_restorable(conn, row.id)
            .await?
            .map(Time::new),
        versions: number(&versions.value)?,
        bytes: number(&bytes.value)?,
        version_source: versions.source,
        byte_source: bytes.source,
        measured_at,
    }))
}

fn retention_source(
    source: &Source<Id>,
    cast: &build::Cast,
) -> ApiResult<orch_api_contract::settings::SettingSource> {
    Ok(orch_api_contract::settings::SettingSource {
        scope: serde_json::from_value(
            serde_json::to_value(source.scope).map_err(ApiError::internal)?,
        )
        .map_err(ApiError::internal)?,
        node: source.node.and_then(|id| cast.node(id)),
    })
}

async fn version_rows(conn: &mut Conn, document: Id, number: i32) -> ApiResult<Rows> {
    let version = history::get(conn, document, number)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let blocks = history::snapshot(conn, document, number)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Rows::Version { version, blocks })
}

async fn changes_rows(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
    query: DocumentChangesGetQuery,
) -> ApiResult<Rows> {
    let from = from_version(conn, principal.user(), row.id, &query.from).await?;
    let to = query
        .to
        .map(|number| {
            i32::try_from(number)
                .ok()
                .filter(|number| *number >= 0)
                .ok_or_else(|| crate::routes::nodes::invalid("to", "is a version number"))
        })
        .transpose()?;
    let (earlier, earlier_title) = state_at(conn, row.id, Some(from), &row.title).await?;
    let (later, later_title) = state_at(conn, row.id, to, &row.title).await?;
    let writers = history::authors_between(conn, row.id, from, to).await?;
    let gone: Vec<_> = earlier
        .iter()
        .map(|block| block.id)
        .filter(|id| !later.iter().any(|block| block.id == *id))
        .collect();
    let tombstones: HashMap<_, _> =
        crate::routes::documents::rows::blocks_by_ids(conn, row.id, &gone)
            .await?
            .into_iter()
            .filter(|block| block.removed_at.is_some())
            .map(|block| (block.id, block))
            .collect();
    let cast_blocks = earlier
        .iter()
        .chain(later.iter())
        .chain(tombstones.values())
        .cloned()
        .collect();
    Ok(Rows::Changes {
        from,
        to,
        earlier,
        later,
        earlier_title,
        later_title,
        tombstones,
        cast_blocks,
        writers,
    })
}

pub(super) async fn list_get(
    ctx: Ctx,
    _query: DocumentVersionListQuery,
    (): (),
) -> ApiResult<DocumentVersionListReply> {
    match project(ctx).await? {
        Reply::List(page) => Ok(page),
        _ => Err(ApiError::internal("the version window changed kind")),
    }
}

pub(super) async fn version_get(
    ctx: Ctx,
    _query: DeletedInspectionQuery,
    (): (),
) -> ApiResult<DocumentVersionGetReply> {
    match project(ctx).await? {
        Reply::Version(version) => Ok(version),
        _ => Err(ApiError::internal("the version snapshot changed kind")),
    }
}

pub(super) async fn changes_get(
    ctx: Ctx,
    _query: DocumentChangesGetQuery,
    (): (),
) -> ApiResult<DocumentChanges> {
    match project(ctx).await? {
        Reply::Changes(changes) => Ok(changes),
        _ => Err(ApiError::internal("the changes snapshot changed kind")),
    }
}

async fn project(ctx: Ctx) -> ApiResult<Reply> {
    let (reply, prepared, current) = native::read(ctx, |ctx, conn, principal| {
        Box::pin(project_snapshot(ctx, conn, principal))
    })
    .await?;
    read_operation::finish_sources(
        &current,
        &prepared.cut,
        prepared.document,
        &prepared.names,
        &prepared.references,
        None,
    )
    .await?;
    Ok(reply)
}

async fn project_snapshot(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<(Reply, Arc<PreparedPage>, Ctx)> {
    let prepared = ctx.operation_data::<PreparedPage>()?;
    read_operation::same_inspection_info(ctx, prepared.inspection.as_ref())?;
    let mut wanted = build::Wanted::default();
    want_writers(&mut wanted, &prepared.writers);
    for node in prepared.rows.source_nodes() {
        wanted.node(node);
    }
    let allowed = prepared
        .names
        .limited
        .then_some(prepared.references.as_slice());
    let cast = build::Cast::for_blocks(
        conn,
        principal,
        wanted,
        prepared.rows.blocks(),
        Policy::default(),
        allowed,
    )
    .await?;
    let reply = match &prepared.rows {
        Rows::List {
            title,
            bytes,
            block_count,
            span,
            open,
            versions,
            next,
            retention,
        } => {
            let mut items = Vec::new();
            if *open {
                items.extend(open_span(title, *bytes, *block_count, span, &cast));
            }
            items.extend(versions.iter().map(|row| entry(row, &cast)));
            Reply::List(DocumentVersionListReply {
                items,
                next_cursor: next.clone(),
                retention: retention
                    .as_ref()
                    .map(|retention| -> ApiResult<_> {
                        Ok(DocumentHistoryRetention {
                            global_oldest_restorable: retention.oldest,
                            effective_version_allowance: retention.versions,
                            byte_allowance: retention.bytes,
                            version_source: retention_source(&retention.version_source, &cast)?,
                            byte_source: retention_source(&retention.byte_source, &cast)?,
                            measured_at: retention.measured_at,
                        })
                    })
                    .transpose()?,
            })
        }
        Rows::Version { version, blocks } => Reply::Version(DocumentVersionGetReply {
            version: entry(version, &cast),
            blocks: blocks.iter().map(|row| cast.block(row)).collect(),
        }),
        Rows::Changes {
            from,
            to,
            earlier,
            later,
            earlier_title,
            later_title,
            tombstones,
            writers,
            ..
        } => {
            let before: Vec<DocumentBlock> = earlier.iter().map(|row| cast.block(row)).collect();
            let after: Vec<DocumentBlock> = later.iter().map(|row| cast.block(row)).collect();
            let removed_by = |block: Id| {
                let row = tombstones.get(&block)?;
                cast.author(
                    row.updated_by,
                    row.updated_by_thread,
                    row.updated_by_agent.as_deref(),
                )
            };
            Reply::Changes(DocumentChanges {
                from: i64::from(*from),
                to: to.map(i64::from),
                title: match (earlier_title, later_title) {
                    (Some(before), Some(after)) => changes::title(before, after),
                    _ => None,
                },
                blocks: changes::between(&before, &after, &removed_by),
                authors: authors_of(writers, &cast),
            })
        }
    };
    Ok((reply, prepared, ctx.clone()))
}
