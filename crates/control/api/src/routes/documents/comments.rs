//! Discussions on a block, or on words of it: opened, answered, resolved, and their comments edited, deleted and reacted to.
//!
//! A discussion sits beside the text; nothing of it is a block, and the
//! document's text, its versions and the stamps of its blocks are left alone.
//! Each change takes the next number of the document's sequence all the same,
//! so that whoever has the document open sees a comment arrive as they see a
//! block change. A viewer comments (the shipped Viewers row of the document
//! gives `doc.comment`) and cannot edit. Under an archived node nothing is
//! added or changed. `client_id` is the comment's own identifier: a request
//! sent twice answers as the first did.

use super::{build, notices};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{actor_name, invalid};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::MissingAccess;
use orch_api_contract::common::{ErrorCode, FieldFault, Page, PageQuery, Problem};
use orch_api_contract::documents::{
    DiscussionResolvedFilter, DocumentComment, DocumentCommentCreate, DocumentCommentCreateBody,
    DocumentCommentDelete, DocumentCommentList, DocumentCommentListQuery, DocumentCommentPage,
    DocumentCommentReactionSet, DocumentCommentReactionSetBody, DocumentCommentUpdate,
    DocumentCommentUpdateBody, DocumentDiscussion, DocumentDiscussionCreate,
    DocumentDiscussionCreateBody, DocumentDiscussionList, DocumentDiscussionListQuery,
    DocumentDiscussionResolve, DocumentDiscussionResolveBody, Inline,
};
use orch_authz::documents::{self, State};
use orch_authz::{Act, Error as AuthError, Policy, Principal};
use orch_db::queries::documents::discussions::{self, Answered, Cursor, NewComment, Opened, Which};
use orch_db::{Conn, DbError};
use orch_documents::blocks::Fault;
use orch_documents::comments;
use orch_types::Id;
use serde_json::json;

mod edit_replay;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(DocumentDiscussionList, list)
        .on(DocumentCommentList, list_comments)
        .on(DocumentDiscussionCreate, open)
        .on(DocumentCommentCreate, answer)
        .on(DocumentDiscussionResolve, resolve)
        .on(DocumentCommentUpdate, edit)
        .on(DocumentCommentDelete, delete)
        .on(DocumentCommentReactionSet, react);
}

/// Faults of a request as the problem that names each member.
fn invalid_faults(faults: Vec<Fault>) -> ApiError {
    Problem::invalid(
        faults
            .into_iter()
            .map(|fault| FieldFault {
                path: fault.path,
                message: fault.message,
            })
            .collect(),
    )
    .into()
}

fn client_id(text: &str) -> ApiResult<Id> {
    text.parse()
        .map_err(|_| invalid("client_id", "must be a UUID"))
}

/// The refusal of what only the author of a comment does.
fn not_the_author(document: Id) -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "only the author of a comment does this",
    )
    .with("required", "comment.author")
    .with("object", json!({ "kind": "document", "id": document }))
    .with("requestable", false)
    .with("override", false)
    .with("missing", Vec::<MissingAccess>::new())
}

/// `409 node.archived`: the document is read-only under the node.
async fn archived(conn: &mut Conn, node: Id) -> ApiError {
    refusals::problem(conn, AuthError::Archived { node }).await
}

/// Requires that the caller may read the document and that its node is not
/// archived: what the author of a comment needs to change their own.
async fn may_change_own(conn: &mut Conn, principal: &Principal, document: Id) -> ApiResult<()> {
    let listed = build::standing(conn, principal, document, Policy::default()).await?;
    if !listed.holds(Permission::DocView) {
        authorization::require_document(
            conn,
            principal,
            document,
            Permission::DocView,
            Policy::default(),
        )
        .await?;
    }
    match listed.archived_node {
        Some(node) => Err(archived(conn, node).await),
        None => Ok(()),
    }
}

/// A comment as the caller reads it.
async fn comment_for(
    conn: &mut Conn,
    principal: &Principal,
    row: &discussions::CommentRow,
) -> ApiResult<DocumentComment> {
    let mut wanted = build::Wanted::default();
    wanted.comment(row);
    let cast = build::Cast::load(conn, principal, wanted, Policy::default()).await?;
    cast.comment(row)
        .ok_or_else(|| ApiError::internal("the author of a comment is not there"))
}

/// A discussion as the caller reads it.
async fn discussion_for(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    full: &discussions::Full,
) -> ApiResult<DocumentDiscussion> {
    build::discussions_for(
        conn,
        principal,
        document,
        std::slice::from_ref(full),
        Policy::default(),
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| ApiError::internal("a discussion could not be shown"))
}

/// The words of a comment as plain text, for a notice to quote.
fn words(content: &[Inline]) -> String {
    orch_documents::blocks::inline_text(content)
}

fn read_cursor(text: Option<&str>) -> ApiResult<Option<Cursor>> {
    let Some(text) = text else {
        return Ok(None);
    };
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    let at = micros
        .parse::<i64>()
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_micros)
        .ok_or_else(bad)?;
    Ok(Some(Cursor {
        at,
        id: id.parse().map_err(|_| bad())?,
    }))
}

/// The discussions beside a document, the newest answer first.
async fn list(
    ctx: Ctx,
    query: DocumentDiscussionListQuery,
    (): (),
) -> ApiResult<Page<DocumentDiscussion>> {
    let id = ctx.id("id")?;
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(paging.limit()).unwrap_or(usize::MAX);
    let after = read_cursor(query.cursor.as_deref())?;
    let which = match query.resolved.unwrap_or_default() {
        DiscussionResolvedFilter::Exclude => Which::Open,
        DiscussionResolvedFilter::Include => Which::All,
        DiscussionResolvedFilter::Only => Which::Resolved,
    };
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    super::live(&mut conn, id).await?;
    let found = discussions::list(
        &mut conn,
        id,
        which,
        after,
        i64::try_from(limit + 1).unwrap_or(i64::MAX),
    )
    .await?;
    let more = found.len() > limit;
    let shown: Vec<_> = found.into_iter().take(limit).collect();
    let fulls: Vec<discussions::Full> = shown.iter().map(|(full, _)| full.clone()).collect();
    let items =
        build::discussions_for(&mut conn, &principal, id, &fulls, Policy::default()).await?;
    let next_cursor = more
        .then(|| {
            shown.last().map(|(full, active)| {
                format!("{}|{}", active.timestamp_micros(), full.discussion.id)
            })
        })
        .flatten();
    Ok(Page { items, next_cursor })
}

/// Reads one comment page after rechecking the document and discussion boundary.
async fn list_comments(
    ctx: Ctx,
    query: DocumentCommentListQuery,
    (): (),
) -> ApiResult<DocumentCommentPage> {
    let document = ctx.id("id")?;
    let discussion = ctx.id("discussion_id")?;
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(paging.limit()).unwrap_or(200);
    let after = read_cursor(query.cursor.as_deref())?;
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    super::live(&mut conn, document).await?;
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
        &mut conn,
        discussion,
        after,
        i64::try_from(limit + 1).unwrap_or(201),
    )
    .await?;
    let more = found.len() > limit;
    let shown: Vec<_> = found.into_iter().take(limit).collect();
    let mut wanted = build::Wanted::default();
    for row in &shown {
        wanted.comment(row);
    }
    let cast = build::Cast::load(&mut conn, &principal, wanted, Policy::default()).await?;
    let items = shown
        .iter()
        .rev()
        .filter_map(|row| cast.comment(row))
        .collect();
    let next_cursor = more
        .then(|| {
            shown
                .last()
                .map(|row| format!("{}|{}", row.created_at.timestamp_micros(), row.id))
        })
        .flatten();
    let wrote = discussions::wrote_in(&mut conn, discussion, principal.user()).await?;
    conn.commit().await.map_err(DbError::from)?;
    Ok(DocumentCommentPage {
        items,
        next_cursor,
        wrote,
    })
}

/// Opens a discussion on a block, with its first comment.
async fn open(
    mut ctx: Ctx,
    (): (),
    body: DocumentDiscussionCreateBody,
) -> ApiResult<DocumentDiscussion> {
    let id = ctx.id("id")?;
    let first = client_id(&body.client_id)?;
    let content = comments::check_content(&body.content).map_err(invalid_faults)?;
    let quote = comments::clean_quote(body.quote.as_deref())
        .map_err(|fault| invalid(&fault.path, &fault.message))?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let content = super::build::label_written(&mut tx, &principal, content).await?;
    let stored = serde_json::to_value(&content).map_err(ApiError::internal)?;
    let row = super::live(&mut tx, id).await?;
    let opened = discussions::open(
        &mut tx,
        id,
        body.block_id,
        &quote,
        principal.user(),
        &NewComment {
            id: first,
            content: &stored,
        },
        ctx.now,
    )
    .await?;
    let full = match opened {
        Opened::NoSuchBlock => {
            return Err(invalid("block_id", "is no live block of this document"));
        }
        Opened::Taken => {
            return Err(ApiError::new(
                ErrorCode::ConflictExists,
                "that client_id is another person's",
            ));
        }
        Opened::Repeated(full) => *full,
        Opened::Created(full) => {
            let by_name = actor_name(&ctx)?;
            let said = notices::Said {
                by: principal.user(),
                by_name: &by_name,
                text: &words(&content),
            };
            let at = notices::Where {
                block: Some(body.block_id),
                discussion: Some(full.discussion.id),
                comment: Some(first),
            };
            notices::started(&mut tx, &row, at, said, ctx.now).await?;
            *full
        }
    };
    let shown = discussion_for(&mut tx, &principal, row.id, &full).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Answers in a discussion.
async fn answer(
    mut ctx: Ctx,
    (): (),
    body: DocumentCommentCreateBody,
) -> ApiResult<DocumentComment> {
    let id = ctx.id("id")?;
    let discussion = ctx.id("discussion_id")?;
    let written = client_id(&body.client_id)?;
    let content = comments::check_content(&body.content).map_err(invalid_faults)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let content = super::build::label_written(&mut tx, &principal, content).await?;
    let stored = serde_json::to_value(&content).map_err(ApiError::internal)?;
    let row = super::live(&mut tx, id).await?;
    let answered = discussions::answer(
        &mut tx,
        id,
        discussion,
        principal.user(),
        &NewComment {
            id: written,
            content: &stored,
        },
        ctx.now,
    )
    .await?;
    let comment = match answered {
        Answered::NoSuchDiscussion => return Err(ApiError::not_found()),
        Answered::Taken => {
            return Err(ApiError::new(
                ErrorCode::ConflictExists,
                "that client_id is another person's",
            ));
        }
        Answered::Repeated(comment) => *comment,
        Answered::Created { comment, .. } => {
            let by_name = actor_name(&ctx)?;
            let text = words(&content);
            let said = notices::Said {
                by: principal.user(),
                by_name: &by_name,
                text: &text,
            };
            let at = notices::Where {
                block: None,
                discussion: Some(discussion),
                comment: Some(written),
            };
            notices::answered(&mut tx, &row, at, said, ctx.now).await?;
            *comment
        }
    };
    let shown = comment_for(&mut tx, &principal, &comment).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Resolves a discussion, or opens it again: whoever may edit the document, or
/// has written in that discussion.
async fn resolve(
    mut ctx: Ctx,
    (): (),
    body: DocumentDiscussionResolveBody,
) -> ApiResult<DocumentDiscussion> {
    let id = ctx.id("id")?;
    let discussion = ctx.id("discussion_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    super::live(&mut tx, id).await?;
    match documents::check_all(
        &mut tx,
        &principal,
        id,
        &[Permission::DocEdit],
        Act::Use,
        State::Live,
        policy,
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => {}
        Ok(Decision::Refused(refusal)) => {
            // Whoever wrote in the discussion may resolve it; whether there is
            // one is told to nobody who is refused.
            let wrote = discussions::wrote_in(&mut tx, discussion, principal.user()).await?;
            if !wrote {
                return Err(refusals::denied(&mut tx, refusal).await?);
            }
            may_change_own(&mut tx, &principal, id).await?;
        }
        Err(error) => return Err(refusals::problem(&mut tx, error).await),
    }
    discussions::get(&mut tx, id, discussion)
        .await?
        .ok_or_else(ApiError::not_found)?;
    discussions::resolve(
        &mut tx,
        id,
        discussion,
        body.resolved,
        principal.user(),
        ctx.now,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let full = discussions::get(&mut tx, id, discussion)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let shown = discussion_for(&mut tx, &principal, id, &full).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Edits a comment: its author alone.
async fn edit(mut ctx: Ctx, (): (), body: DocumentCommentUpdateBody) -> ApiResult<DocumentComment> {
    let id = ctx.id("id")?;
    let comment = ctx.id("comment_id")?;
    let content = comments::check_content(&body.content).map_err(invalid_faults)?;
    let attempt = edit_replay::attempt(&body)?;
    let (mut tx, _admission) = transaction_guard::begin(&ctx).await?;
    orch_db::queries::documents::containers::lock_document_scope(&mut tx, id).await?;
    super::live_locked(&mut tx, id).await?;
    // Initial authority precedes the scope/row locks. Recheck the same guarded
    // identity after that wait, including a credential deadline crossed in it.
    let guard = transaction_guard::check(&ctx, &mut tx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let content = super::build::label_written(&mut tx, &principal, content).await?;
    let stored = serde_json::to_value(&content).map_err(ApiError::internal)?;
    authorization::require_document(
        &mut tx,
        &principal,
        id,
        Permission::DocView,
        Policy::default(),
    )
    .await?;
    let known = discussions::comment(&mut tx, id, comment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if known.author_id != principal.user() {
        return Err(not_the_author(id));
    }
    may_change_own(&mut tx, &principal, id).await?;
    if known.deleted_at.is_some() {
        return Err(ApiError::not_found());
    }
    if let Some(attempt) = &attempt {
        if let Some(recorded) = edit_replay::find(&ctx, &mut tx, &known, attempt).await? {
            let shown = comment_for(&mut tx, &principal, &recorded).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(shown);
        }
        if known.seq_updated != attempt.expected_seq {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the comment changed after this draft was begun",
            )
            .with("expected_seq", attempt.expected_seq)
            .with(
                "current_comment",
                comment_for(&mut tx, &principal, &known).await?,
            ));
        }
    }
    let edited = discussions::edit(&mut tx, id, comment, &stored, ctx.now)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(attempt) = &attempt {
        edit_replay::keep(&ctx, &mut tx, &edited, attempt).await?;
    }
    let shown = comment_for(&mut tx, &principal, &edited).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Deletes a comment, which stays as an emptied line: its author, or whoever
/// manages the document, whose deleting is written to the audit trail.
async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let comment = ctx.id("comment_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let row = super::live(&mut tx, id).await?;
    authorization::require_document(&mut tx, &principal, id, Permission::DocView, policy).await?;
    let known = discussions::comment(&mut tx, id, comment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let by_someone_else = known.author_id != principal.user();
    if by_someone_else {
        authorization::require_document(&mut tx, &principal, id, Permission::DocManage, policy)
            .await?;
    } else {
        may_change_own(&mut tx, &principal, id).await?;
    }
    let was_deleted = known.deleted_at.is_some();
    discussions::delete(&mut tx, id, comment, ctx.now)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if by_someone_else && !was_deleted {
        let who = actor_name(&ctx)?;
        super::record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::DocumentCommentDeleted),
            &row,
            &format!(
                "{who} deleted a comment by somebody else on \"{}\"",
                row.title
            ),
            json!({ "comment": comment, "author": known.author_id }),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Puts the caller's reaction to a comment on, or takes it off.
async fn react(
    mut ctx: Ctx,
    (): (),
    body: DocumentCommentReactionSetBody,
) -> ApiResult<DocumentComment> {
    let id = ctx.id("id")?;
    let comment = ctx.id("comment_id")?;
    let emoji =
        comments::clean_emoji(&body.emoji).map_err(|fault| invalid(&fault.path, &fault.message))?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    super::live(&mut tx, id).await?;
    let reacted = discussions::react(&mut tx, id, comment, &emoji, principal.user(), body.on)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let shown = comment_for(&mut tx, &principal, &reacted).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
