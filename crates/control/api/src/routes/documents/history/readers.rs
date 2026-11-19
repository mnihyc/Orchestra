//! Actual version windows share current document authority and the existing block/author producer.

use super::{entry, open_span, parse_author, version_number, want_writers, writers_of, wrote_in};
use crate::middleware::{
    inspection,
    requests::{ApiError, ApiResult},
};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::page::NameBounds;
use crate::routes::documents::{DocumentRow, build, read_operation};
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::documents::{
    DocumentVersion, DocumentVersionGetReply, DocumentVersionListQuery,
};
use orch_api_contract::nodes::DeletedInspectionQuery;
use orch_authz::{Caller, Policy};
use orch_db::queries::documents::{BlockRow, history};
use orch_db::{Conn, DbError};
use orch_documents::versions::{Span, Writer};
use orch_types::Id;

enum Query {
    List(DocumentVersionListQuery),
    Version,
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
    },
    Version {
        version: history::VersionRow,
        blocks: Vec<BlockRow>,
    },
}
impl Rows {
    fn blocks(&self) -> &[BlockRow] {
        match self {
            Self::List { .. } => &[],
            Self::Version { blocks, .. } => blocks,
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
    List(Page<DocumentVersion>),
    Version(DocumentVersionGetReply),
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

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: Query,
) -> ApiResult<OperationPlan> {
    let document = ctx.id("id")?;
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
            Query::List(query) => list_rows(conn, &row, query).await?,
            Query::Version => version_rows(conn, document, version_number(ctx)?).await?,
        };
        let writers = rows.writers();
        let mut references = build::ProjectedBlocks::candidates(conn, rows.blocks()).await?;
        references.extend(writers.iter().filter_map(|writer| match writer {
            Writer::Agent { thread, .. } => Some(build::WrittenLabelSource::Node(*thread)),
            Writer::User(_) => None,
        }));
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

/// The original open-span/page selection, including the page's lookahead and cursor.
async fn list_rows(
    conn: &mut Conn,
    row: &DocumentRow,
    query: DocumentVersionListQuery,
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

pub(super) async fn list_get(
    ctx: Ctx,
    _query: DocumentVersionListQuery,
    (): (),
) -> ApiResult<Page<DocumentVersion>> {
    match project(ctx).await? {
        Reply::List(page) => Ok(page),
        Reply::Version(_) => Err(ApiError::internal("the version window changed kind")),
    }
}
pub(super) async fn version_get(
    ctx: Ctx,
    _query: DeletedInspectionQuery,
    (): (),
) -> ApiResult<DocumentVersionGetReply> {
    match project(ctx).await? {
        Reply::Version(version) => Ok(version),
        Reply::List(_) => Err(ApiError::internal("the version snapshot changed kind")),
    }
}

async fn project(ctx: Ctx) -> ApiResult<Reply> {
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedPage>()?;
    read_operation::same_inspection_info(&ctx, prepared.inspection.as_ref())?;
    let mut wanted = build::Wanted::default();
    want_writers(&mut wanted, &prepared.writers);
    let allowed = prepared
        .names
        .limited
        .then_some(prepared.references.as_slice());
    let cast = build::Cast::for_blocks(
        &mut tx,
        &principal,
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
        } => {
            let mut items = Vec::new();
            if *open {
                items.extend(open_span(title, *bytes, *block_count, span, &cast));
            }
            items.extend(versions.iter().map(|row| entry(row, &cast)));
            Reply::List(Page {
                items,
                next_cursor: next.clone(),
            })
        }
        Rows::Version { version, blocks } => Reply::Version(DocumentVersionGetReply {
            version: entry(version, &cast),
            blocks: blocks.iter().map(|row| cast.block(row)).collect(),
        }),
    };
    tx.commit().await.map_err(DbError::from)?;
    read_operation::finish_sources(
        &ctx,
        &prepared.cut,
        prepared.document,
        &prepared.names,
        &prepared.references,
    )
    .await?;
    Ok(reply)
}
