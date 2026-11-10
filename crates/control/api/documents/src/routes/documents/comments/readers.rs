//! The existing discussion/comment windows use the document reader's authority and label bounds.

use super::super::build;

use super::super::read_operation;

use crate::middleware::inspection;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::routes::access::page::NameBounds;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::documents::DiscussionResolvedFilter;

use orch_api_contract::documents::DocumentCommentListQuery;

use orch_api_contract::documents::DocumentCommentPage;

use orch_api_contract::documents::DocumentDiscussion;

use orch_api_contract::documents::DocumentDiscussionListQuery;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_db::queries::documents::discussions;

use orch_db::queries::documents::discussions::CommentRow;

use orch_db::queries::documents::discussions::Full;

use orch_db::queries::documents::discussions::Which;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;
enum Query {
    Discussions(DocumentDiscussionListQuery),
    Comments(DocumentCommentListQuery),
}

enum Rows {
    Discussions {
        fulls: Vec<Full>,
        next: Option<String>,
    },
    Comments {
        comments: Vec<CommentRow>,
        next: Option<String>,
        wrote: bool,
    },
}

struct PreparedPage {
    document: Id,
    rows: Rows,
    names: NameBounds,
    references: Vec<build::WrittenLabelSource>,
    inspection: Option<inspection::Info>,
    cut: crate::routes::document_tree::metadata::Cut,
}

enum Reply {
    Discussions(Page<DocumentDiscussion>),
    Comments(DocumentCommentPage),
}

pub(super) fn discussions<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentDiscussionListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, Query::Discussions(query)))
}

pub(super) fn comments<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentCommentListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, Query::Comments(query)))
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: Query,
) -> ApiResult<crate::registry::OperationPlan> {
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
        let row = super::super::live(conn, document).await?;
        let rows = match query {
            Query::Discussions(query) => discussion_rows(conn, document, query).await?,
            Query::Comments(query) => {
                comment_rows(
                    conn,
                    document,
                    ctx.id("discussion_id")?,
                    principal.user(),
                    query,
                )
                .await?
            }
        };
        let references = match &rows {
            Rows::Discussions { fulls, .. } => build::discussion_sources(conn, fulls).await?,
            Rows::Comments { comments, .. } => build::comment_sources(conn, comments).await?,
        };
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
            names,
            references,
            inspection,
            cut,
        }))
    })
    .await
}

/// Preserve the original filter, lookahead and last-activity cursor before projection.
async fn discussion_rows(
    conn: &mut Conn,
    document: Id,
    query: DocumentDiscussionListQuery,
) -> ApiResult<Rows> {
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(paging.limit()).unwrap_or(usize::MAX);
    let after = super::read_cursor(query.cursor.as_deref())?;
    let which = match query.resolved.unwrap_or_default() {
        DiscussionResolvedFilter::Exclude => Which::Open,
        DiscussionResolvedFilter::Include => Which::All,
        DiscussionResolvedFilter::Only => Which::Resolved,
    };
    let found = discussions::list(
        conn,
        document,
        which,
        after,
        i64::try_from(limit + 1).unwrap_or(i64::MAX),
    )
    .await?;
    let more = found.len() > limit;
    let shown: Vec<_> = found.into_iter().take(limit).collect();
    let next = more
        .then(|| {
            shown.last().map(|(full, active)| {
                format!("{}|{}", active.timestamp_micros(), full.discussion.id)
            })
        })
        .flatten();
    Ok(Rows::Discussions {
        fulls: shown.into_iter().map(|(full, _)| full).collect(),
        next,
    })
}

/// Discussion identity remains subordinate to its actual document, not a new ACL target.
async fn comment_rows(
    conn: &mut Conn,
    document: Id,
    discussion: Id,
    user: Id,
    query: DocumentCommentListQuery,
) -> ApiResult<Rows> {
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(paging.limit()).unwrap_or(200);
    let after = super::read_cursor(query.cursor.as_deref())?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_discussions WHERE document_id=$1 AND id=$2)",
    )
    .bind(document)
    .bind(discussion)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if !exists {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "the discussion is not there",
        ));
    }
    let found = discussions::comments_page(
        conn,
        discussion,
        after,
        i64::try_from(limit + 1).unwrap_or(201),
    )
    .await?;
    let more = found.len() > limit;
    let shown: Vec<_> = found.into_iter().take(limit).collect();
    let next = more
        .then(|| {
            shown
                .last()
                .map(|row| format!("{}|{}", row.created_at.timestamp_micros(), row.id))
        })
        .flatten();
    let wrote = discussions::wrote_in(conn, discussion, user).await?;
    Ok(Rows::Comments {
        comments: shown,
        next,
        wrote,
    })
}

pub(super) async fn discussions_get(
    ctx: Ctx,
    _query: DocumentDiscussionListQuery,
    (): (),
) -> ApiResult<Page<DocumentDiscussion>> {
    match project(ctx).await? {
        Reply::Discussions(page) => Ok(page),
        Reply::Comments(_) => Err(ApiError::internal("the discussion window changed kind")),
    }
}

pub(super) async fn comments_get(
    ctx: Ctx,
    _query: DocumentCommentListQuery,
    (): (),
) -> ApiResult<DocumentCommentPage> {
    match project(ctx).await? {
        Reply::Comments(page) => Ok(page),
        Reply::Discussions(_) => Err(ApiError::internal("the comment window changed kind")),
    }
}

async fn project(ctx: Ctx) -> ApiResult<Reply> {
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedPage>()?;
    read_operation::same_inspection_info(&ctx, prepared.inspection.as_ref())?;
    let allowed = prepared
        .names
        .limited
        .then_some(prepared.references.as_slice());
    let reply = match &prepared.rows {
        Rows::Discussions { fulls, next } => Reply::Discussions(Page {
            items: build::discussions_bounded(
                &mut tx,
                &principal,
                prepared.document,
                fulls,
                Policy::default(),
                allowed,
            )
            .await?,
            next_cursor: next.clone(),
        }),
        Rows::Comments {
            comments,
            next,
            wrote,
        } => {
            let mut items =
                build::comments_bounded(&mut tx, &principal, comments, Policy::default(), allowed)
                    .await?;
            items.reverse();
            Reply::Comments(DocumentCommentPage {
                items,
                next_cursor: next.clone(),
                wrote: *wrote,
            })
        }
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
