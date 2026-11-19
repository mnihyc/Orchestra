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

use super::{DocumentRow, build, notices};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{actor_name, invalid};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::MissingAccess;
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::documents::{
    DocumentComment, DocumentCommentCreate, DocumentCommentCreateBody, DocumentCommentDelete,
    DocumentCommentList, DocumentCommentReactionSet, DocumentCommentReactionSetBody,
    DocumentCommentUpdate, DocumentCommentUpdateBody, DocumentCommentUpdateResponse,
    DocumentDiscussion, DocumentDiscussionCreate, DocumentDiscussionCreateBody,
    DocumentDiscussionList, DocumentDiscussionResolve, DocumentDiscussionResolveBody,
    DocumentDiscussionResolveResponse, Inline,
};
use orch_authz::documents::{self, State};
use orch_authz::{Act, Error as AuthError, Policy, Principal};
use orch_db::queries::documents::discussions::{self, Answered, Cursor, NewComment, Opened};
use orch_db::{Conn, DbError, Tx};
use orch_documents::blocks::Fault;
use orch_documents::comments;
use orch_types::Id;
use serde_json::json;

mod edit_replay;
mod readers;
mod writers;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            DocumentDiscussionList,
            readers::discussions,
            readers::discussions_get,
        )
        .on_with_operation(
            DocumentCommentList,
            readers::comments,
            readers::comments_get,
        )
        .on(DocumentDiscussionCreate, open)
        .on(DocumentCommentCreate, answer)
        .on_with_operation(DocumentDiscussionResolve, writers::resolve, resolve)
        .on_with_operation(DocumentCommentUpdate, writers::edit, edit)
        .on_with_operation(DocumentCommentDelete, writers::delete, delete)
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

/// The three native Rule prefixes are rebuilt by their existing operation producer.
/// Their retained outcomes belong to the final guard after the document-row wait.
pub(super) enum EditPrefix {
    NotAuthor,
    Deleted,
    Repeated(discussions::CommentRow),
    Stale(discussions::CommentRow),
    Ready(discussions::CommentRow),
}

pub(super) struct DeletePrefix {
    known: discussions::CommentRow,
    by_someone_else: bool,
}

pub(super) async fn resolve_prefix(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    discussion: Id,
) -> ApiResult<discussions::DiscussionRow> {
    super::live(conn, document).await?;
    let policy = Policy::default();
    match documents::check_all(
        conn,
        principal,
        document,
        &[Permission::DocEdit],
        Act::Use,
        State::Live,
        policy,
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => {}
        Ok(Decision::Refused(refusal)) => {
            // The native editor path needs no added View gate. Only the author
            // alternative requires current View and the unarchived document.
            if !discussions::wrote_in(conn, discussion, principal.user()).await? {
                return Err(refusals::denied(conn, refusal).await?);
            }
            may_change_own(conn, principal, document).await?;
        }
        Err(error) => return Err(refusals::problem(conn, error).await),
    }
    discussions::get(conn, document, discussion)
        .await?
        .map(|full| full.discussion)
        .ok_or_else(ApiError::not_found)
}

async fn edit_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    comment: Id,
    attempt: Option<&edit_replay::Attempt>,
) -> ApiResult<EditPrefix> {
    super::live(conn, document).await?;
    authorization::require_document(
        conn,
        principal,
        document,
        Permission::DocView,
        Policy::default(),
    )
    .await?;
    let known = discussions::comment(conn, document, comment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if known.author_id != principal.user() {
        return Ok(EditPrefix::NotAuthor);
    }
    may_change_own(conn, principal, document).await?;
    if known.deleted_at.is_some() {
        return Ok(EditPrefix::Deleted);
    }
    if let Some(attempt) = attempt {
        if let Some(recorded) = edit_replay::find(ctx, conn, &known, attempt).await? {
            return Ok(EditPrefix::Repeated(recorded));
        }
        if known.seq_updated != attempt.expected_seq {
            return Ok(EditPrefix::Stale(known));
        }
    }
    Ok(EditPrefix::Ready(known))
}

pub(super) async fn delete_prefix(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    comment: Id,
) -> ApiResult<DeletePrefix> {
    super::live(conn, document).await?;
    let policy = Policy::default();
    authorization::require_document(conn, principal, document, Permission::DocView, policy).await?;
    let known = discussions::comment(conn, document, comment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let by_someone_else = known.author_id != principal.user();
    if by_someone_else {
        authorization::require_document(conn, principal, document, Permission::DocManage, policy)
            .await?;
    } else {
        may_change_own(conn, principal, document).await?;
    }
    Ok(DeletePrefix {
        known,
        by_someone_else,
    })
}

/// Keep the existing early admission and actual native scope/document locks. The
/// last guard rebuilds the complete native effect-free prefix on this connection.
async fn begin_change(ctx: &Ctx, document: Id) -> ApiResult<(Ctx, Tx, Guard, DocumentRow)> {
    let (mut tx, _initial) = transaction_guard::begin(ctx).await?;
    orch_db::queries::documents::containers::lock_document_scope(&mut tx, document).await?;
    let row = super::live_locked(&mut tx, document).await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    Ok((current, tx, guard, row))
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
) -> ApiResult<DocumentDiscussionResolveResponse> {
    let result = async {
        let id = ctx.id("id")?;
        let discussion = ctx.id("discussion_id")?;
        let (current, mut tx, guard, _) = begin_change(&ctx, id).await?;
        ctx = current;
        let principal = guard.principal().clone();
        writers::pending(&ctx, &principal);
        let prepared = guard.operation_data::<writers::PreparedWrite>()?;
        let writers::Native::Resolve(known) = &prepared.native else {
            return Err(ApiError::internal("the resolve prefix changed kind"));
        };
        guard.check_deadline(&ctx)?;
        discussions::resolve_prepared(
            &mut tx,
            known.document_id,
            known.id,
            body.resolved,
            principal.user(),
            ctx.now,
        )
        .await?
        .ok_or_else(ApiError::not_found)?;
        let full = discussions::get(&mut tx, id, discussion)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let shown = writers::discussion(&ctx, &mut tx, &principal, &full).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(writers::finish_discussion(&ctx, &full, shown))
    }
    .await;
    authorization::projected_result(ctx, std::future::ready(result), false).await
}

/// Edits a comment: its author alone.
async fn edit(
    mut ctx: Ctx,
    (): (),
    body: DocumentCommentUpdateBody,
) -> ApiResult<DocumentCommentUpdateResponse> {
    let result = async {
        let id = ctx.id("id")?;
        let _comment = ctx.id("comment_id")?;
        let content = comments::check_content(&body.content).map_err(invalid_faults)?;
        let attempt = edit_replay::attempt(&body)?;
        let (current, mut tx, guard, _) = begin_change(&ctx, id).await?;
        ctx = current;
        let principal = guard.principal().clone();
        writers::pending(&ctx, &principal);
        let prepared = guard.operation_data::<writers::PreparedWrite>()?;
        let content = prepared.labels.apply(content)?;
        let stored = serde_json::to_value(&content).map_err(ApiError::internal)?;
        let writers::Native::Edit(prefix) = &prepared.native else {
            return Err(ApiError::internal("the comment edit prefix changed kind"));
        };
        let known = match prefix {
            EditPrefix::NotAuthor => {
                // Preserve the existing static error and its optional Read projection.
                writers::empty(&ctx, &mut tx, &principal).await?;
                return Err(not_the_author(id));
            }
            EditPrefix::Deleted => return Err(ApiError::not_found()),
            EditPrefix::Repeated(recorded) => {
                let shown = writers::comment(&ctx, &mut tx, &principal, recorded).await?;
                tx.commit().await.map_err(DbError::from)?;
                return Ok(writers::finish_comment(&ctx, recorded, shown));
            }
            EditPrefix::Stale(known) => {
                let shown = writers::comment(&ctx, &mut tx, &principal, known).await?;
                let expected_seq = attempt
                    .as_ref()
                    .ok_or_else(|| ApiError::internal("a stale comment has no edit attempt"))?
                    .expected_seq;
                let mut problem = ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the comment changed after this draft was begun",
                )
                .with("expected_seq", expected_seq);
                if let Some(shown) = shown {
                    problem = problem.with("current_comment", shown);
                }
                return Err(problem);
            }
            EditPrefix::Ready(known) => known,
        };
        guard.check_deadline(&ctx)?;
        let edited = discussions::edit_prepared(&mut tx, known, &stored, ctx.now)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if let Some(attempt) = &attempt {
            edit_replay::keep(&ctx, &mut tx, &edited, attempt).await?;
        }
        let shown = writers::comment(&ctx, &mut tx, &principal, &edited).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(writers::finish_comment(&ctx, &edited, shown))
    }
    .await;
    authorization::projected_result(ctx, std::future::ready(result), false).await
}

/// Deletes a comment, which stays as an emptied line: its author, or whoever
/// manages the document, whose deleting is written to the audit trail.
async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let result = async {
        let id = ctx.id("id")?;
        let comment = ctx.id("comment_id")?;
        let (current, mut tx, guard, row) = begin_change(&ctx, id).await?;
        ctx = current;
        let prepared = guard.operation_data::<writers::PreparedWrite>()?;
        let writers::Native::Delete(prefix) = &prepared.native else {
            return Err(ApiError::internal("the comment delete prefix changed kind"));
        };
        let known = &prefix.known;
        let by_someone_else = prefix.by_someone_else;
        let was_deleted = known.deleted_at.is_some();
        // Empty acknowledgement was completed in the final effect-free producer;
        // no Read/credential re-decision follows an accepted tombstone or audit.
        guard.check_deadline(&ctx)?;
        discussions::delete_prepared(&mut tx, known, ctx.now)
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
    .await;
    authorization::projected_result(ctx, std::future::ready(result), false).await
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
