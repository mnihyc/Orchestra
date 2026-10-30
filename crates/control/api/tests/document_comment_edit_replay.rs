//! Modern edits through the real HTTP router and database: replay is not a second write or a stale overwrite.

mod desk;
mod places;
mod support;

use chrono::Duration;
use desk::{Desk, text};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentComment, DocumentCommentDelete, DocumentCommentUpdate, DocumentCommentUpdateBody,
    DocumentDelete, DocumentDiscussionCreate, DocumentDiscussionCreateBody, Inline,
};
use orch_api_contract::nodes::NodeArchive;
use orch_api_core::testing::Reply;
use orch_config::ServerConfig;
use orch_db::idempotency;
use orch_db::queries::documents::discussions;
use orch_db::queries::{access, documents, identity};
use orch_types::{Id, Json, new_id};
use places::Who;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn office(hours: u64) -> Desk {
    let mut config = ServerConfig::default();
    config.api.idempotency_ttl_hours = hours;
    let world = support::World::with(config).await;
    let ada = places::person(&world, "ada").await;
    let org = places::make_org(&world, &ada, "Office", "office")
        .await
        .base
        .id;
    let project = places::make_project(&world, &ada, org, "Work")
        .await
        .base
        .id;
    let topic = places::make_topic(&world, &ada, project, "Plans")
        .await
        .base
        .id;
    Desk {
        world,
        ada,
        org,
        project,
        topic,
    }
}

async fn authored(desk: &Desk, author: &Who) -> (Id, DocumentComment) {
    authored_named(desk, author, "Handbook").await
}

async fn authored_named(desk: &Desk, author: &Who, name: &str) -> (Id, DocumentComment) {
    let document = desk.make(&desk.ada, name).await.base.id;
    let blocks = desk
        .paragraphs(&desk.ada, document, &["A synthetic paragraph."])
        .await;
    let opened = desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&author.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: blocks[0].id,
            quote: None,
            content: text("Original comment"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    (document, opened.comments[0].clone())
}

fn attempt(comment: &DocumentComment, content: Vec<Inline>) -> DocumentCommentUpdateBody {
    DocumentCommentUpdateBody {
        content,
        client_id: Some(new_id().to_string()),
        expected_seq: Some(comment.seq_updated),
    }
}

async fn edit(
    desk: &Desk,
    author: &Who,
    document: Id,
    comment: Id,
    body: &DocumentCommentUpdateBody,
) -> Reply<DocumentComment> {
    desk.world
        .app
        .call(DocumentCommentUpdate, &[&document, &comment])
        .token(&author.token)
        .body(body)
        .send()
        .await
}

fn said(comment: &DocumentComment) -> String {
    comment
        .content
        .iter()
        .filter_map(|run| match run {
            Inline::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct State {
    document_seq: i64,
    discussion_seq: i64,
    comment: discussions::CommentRow,
    told: Vec<Id>,
    mentions: Vec<Json>,
}

async fn state(desk: &Desk, document: Id, comment: Id) -> Proof<State> {
    let mut conn = desk.world.conn().await;
    let doc = documents::get(&mut conn, document)
        .await?
        .ok_or("the synthetic document disappeared")?;
    let row = discussions::comment(&mut conn, document, comment)
        .await?
        .ok_or("the synthetic comment disappeared")?;
    let discussion_seq =
        sqlx::query_scalar("SELECT seq_updated FROM document_discussions WHERE id=$1")
            .bind(row.discussion_id)
            .fetch_one(&mut *conn)
            .await?;
    let mentions = sqlx::query_scalar("SELECT to_jsonb(n) FROM notifications n WHERE kind='doc.mention' AND data->>'document_id'=$1 ORDER BY id")
        .bind(document.to_string()).fetch_all(&mut *conn).await?;
    Ok(State {
        document_seq: doc.doc_seq,
        discussion_seq,
        comment: row,
        told: doc.told,
        mentions,
    })
}

fn receipt_key(document: Id, comment: Id, body: &DocumentCommentUpdateBody) -> Proof<String> {
    Ok(format!(
        "document-comment-edit:{document}:{comment}:{}",
        body.client_id
            .as_deref()
            .ok_or("a modern attempt has a key")?
    ))
}

#[tokio::test]
async fn concurrent_and_sequential_exact_repeats_do_not_restamp_or_resend_mentions() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let reader = desk.member("reader", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let mut content = text("Ask ");
    content.push(Inline::Mention {
        user_id: reader.id(),
        user: None,
    });
    let body = attempt(&comment, content);
    let before = state(&desk, document, comment.id).await?;
    let (first, concurrent) = tokio::join!(
        edit(&desk, &author, document, comment.id, &body),
        edit(&desk, &author, document, comment.id, &body)
    );
    let first = first.ok();
    assert_eq!(concurrent.ok(), first);
    let committed = state(&desk, document, comment.id).await?;
    assert_eq!(committed.document_seq, before.document_seq + 1);
    assert_eq!(committed.discussion_seq, committed.document_seq);
    assert_eq!(committed.comment.seq_updated, first.seq_updated);
    assert!(
        committed.mentions.is_empty(),
        "editing a mention sends no notice"
    );
    assert!(!committed.told.contains(&reader.id()));
    desk.world.advance(Duration::seconds(2));
    assert_eq!(
        edit(&desk, &author, document, comment.id, &body).await.ok(),
        first
    );
    assert_eq!(state(&desk, document, comment.id).await?, committed);
    let key = receipt_key(document, comment.id, &body)?;
    let mut conn = desk.world.conn().await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(author.id())
            .bind(key)
            .fetch_one(&mut *conn)
            .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
async fn a_changed_payload_or_base_under_the_same_identity_is_a_refusal_with_no_effect() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("First exact edit"));
    let first = edit(&desk, &author, document, comment.id, &body).await.ok();
    let kept = state(&desk, document, comment.id).await?;
    let mut changed = body.clone();
    changed.content = text("Different payload");
    assert_eq!(
        edit(&desk, &author, document, comment.id, &changed)
            .await
            .problem()
            .code,
        ErrorCode::ConflictIdempotencyMismatch
    );
    let mut changed_base = body.clone();
    changed_base.expected_seq = Some(first.seq_updated);
    assert_eq!(
        edit(&desk, &author, document, comment.id, &changed_base)
            .await
            .problem()
            .code,
        ErrorCode::ConflictIdempotencyMismatch
    );
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn replay_after_a_later_edit_returns_its_own_canonical_result_with_current_identities_without_overwriting()
-> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let teammate = desk.member("teammate", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let mut content = text("Earlier words ");
    content.push(Inline::Mention {
        user_id: teammate.id(),
        user: None,
    });
    let body = attempt(&comment, content);
    let first = edit(&desk, &author, document, comment.id, &body).await.ok();
    let later = edit(
        &desk,
        &author,
        document,
        comment.id,
        &attempt(&first, text("A later edit")),
    )
    .await
    .ok();
    {
        let mut conn = desk.world.conn().await;
        sqlx::query("UPDATE users SET display_name='Current author' WHERE id=$1")
            .bind(author.id())
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE users SET display_name='Current teammate' WHERE id=$1")
            .bind(teammate.id())
            .execute(&mut *conn)
            .await?;
        let receipt = idempotency::find(
            &mut conn,
            author.id(),
            &receipt_key(document, comment.id, &body)?,
            desk.world.now() - Duration::hours(24),
        )
        .await?
        .ok_or("the committed native receipt is absent")?;
        assert_eq!(
            receipt.response["result"]["author_id"],
            author.id().to_string()
        );
        assert!(receipt.response["result"].get("author").is_none());
        assert!(
            receipt.response["result"]["content"][1]
                .get("user")
                .is_none()
        );
    }
    let kept = state(&desk, document, comment.id).await?;
    let replay = edit(&desk, &author, document, comment.id, &body).await.ok();
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.seq_updated, first.seq_updated);
    assert_eq!(replay.edited_at, first.edited_at);
    assert_eq!(said(&replay), said(&first));
    assert_ne!(said(&replay), said(&later));
    assert_eq!(replay.author.display_name, "Current author");
    let Inline::Mention {
        user: Some(shown), ..
    } = &replay.content[1]
    else {
        return Err("the current mention identity is missing".into());
    };
    assert_eq!(shown.display_name, "Current teammate");
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn configured_receipt_expiry_and_actual_purge_leave_old_cas_unable_to_overwrite() -> Proof {
    let desk = office(2).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Committed edit"));
    edit(&desk, &author, document, comment.id, &body).await.ok();
    let kept = state(&desk, document, comment.id).await?;
    // Past this configured lifetime, but still within the usual 24-hour default.
    desk.world
        .advance(Duration::hours(2) + Duration::seconds(1));
    let stale = edit(&desk, &author, document, comment.id, &body)
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        stale.extra["current_comment"]["seq_updated"],
        kept.comment.seq_updated
    );
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    {
        let mut conn = desk.world.conn().await;
        assert_eq!(
            idempotency::purge(&mut conn, desk.world.now() - Duration::hours(2)).await?,
            1
        );
    }
    assert_eq!(
        edit(&desk, &author, document, comment.id, &body)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn a_client_id_is_comment_scoped_and_cannot_return_another_comments_result() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document_a, a) = authored_named(&desk, &author, "Handbook A").await;
    let (document_b, b) = authored_named(&desk, &author, "Handbook B").await;
    let body_a = attempt(&a, text("A edit"));
    let mut body_b = attempt(&b, text("B edit"));
    body_b.client_id.clone_from(&body_a.client_id);
    let edited_a = edit(&desk, &author, document_a, a.id, &body_a).await.ok();
    let edited_b = edit(&desk, &author, document_b, b.id, &body_b).await.ok();
    assert_eq!(edited_b.id, b.id);
    assert_ne!(edited_a.id, edited_b.id);
    assert_eq!(said(&edited_b), "B edit");
    assert_eq!(
        edit(&desk, &author, document_a, a.id, &body_a).await.ok(),
        edited_a
    );
    assert_eq!(
        edit(&desk, &author, document_b, b.id, &body_b).await.ok(),
        edited_b
    );
    Ok(())
}

#[tokio::test]
async fn another_author_and_current_view_withdrawal_cannot_bypass_authority_with_a_receipt() -> Proof
{
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let other = desk.member("other", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Committed edit"));
    edit(&desk, &author, document, comment.id, &body).await.ok();
    let kept = state(&desk, document, comment.id).await?;
    let refused = edit(&desk, &other, document, comment.id, &body)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "comment.author");
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    {
        let mut tx = desk.world.db.pool.begin().await?;
        let roles = access::roles_in(&mut tx, desk.org, desk.world.now()).await?;
        let owned: Vec<_> = roles
            .iter()
            .filter(|role| role.subject_kind == "user" && role.subject_id == author.id())
            .collect();
        assert_eq!(
            owned.len(),
            1,
            "the explicit native role is the author's only content authority"
        );
        for role in owned {
            access::end_role(&mut tx, role.id, desk.ada.id(), desk.world.now()).await?;
        }
        access::bump_epoch(&mut tx).await?;
        tx.commit().await?;
    }
    let refused = edit(&desk, &author, document, comment.id, &body)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.view");
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn a_deleted_comment_never_replays_its_old_live_body() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Committed edit"));
    edit(&desk, &author, document, comment.id, &body).await.ok();
    desk.world
        .app
        .call(DocumentCommentDelete, &[&document, &comment.id])
        .token(&author.token)
        .send()
        .await
        .ok();
    let kept = state(&desk, document, comment.id).await?;
    assert!(kept.comment.deleted_at.is_some());
    assert_eq!(
        edit(&desk, &author, document, comment.id, &body)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn a_deleted_document_and_an_inspection_request_never_replay_a_live_edit() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Committed edit"));
    edit(&desk, &author, document, comment.id, &body).await.ok();
    let inspect = desk
        .world
        .app
        .call(DocumentCommentUpdate, &[&document, &comment.id])
        .token(&author.token)
        .raw_query("inspect_deleted=true")
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(inspect.code, ErrorCode::RequestInvalid);
    desk.world
        .app
        .call(DocumentDelete, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let kept = state(&desk, document, comment.id).await?;
    assert_eq!(
        edit(&desk, &author, document, comment.id, &body)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn an_archived_place_cannot_return_a_retained_edit_as_an_allowed_mutation() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Committed edit"));
    edit(&desk, &author, document, comment.id, &body).await.ok();
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await)
        .send()
        .await
        .ok();
    let kept = state(&desk, document, comment.id).await?;
    assert_eq!(
        edit(&desk, &author, document, comment.id, &body)
            .await
            .problem()
            .code,
        ErrorCode::NodeArchived
    );
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn a_revoked_current_session_cannot_retrieve_its_persons_retained_edit() -> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Committed edit"));
    edit(&desk, &author, document, comment.id, &body).await.ok();
    {
        let mut conn = desk.world.conn().await;
        assert_eq!(
            identity::revoke_sessions_of(&mut conn, author.id(), None, desk.world.now()).await?,
            1
        );
    }
    let kept = state(&desk, document, comment.id).await?;
    let refused = edit(&desk, &author, document, comment.id, &body).await;
    assert_eq!(refused.status, 401);
    assert_eq!(refused.problem().code, ErrorCode::AuthInvalidToken);
    assert_eq!(state(&desk, document, comment.id).await?, kept);
    Ok(())
}

#[tokio::test]
async fn modern_fields_are_paired_and_legacy_content_only_calls_keep_their_explicit_behavior()
-> Proof {
    let desk = office(24).await;
    let author = desk.member("author", "viewer").await;
    let (document, comment) = authored(&desk, &author).await;
    let body = attempt(&comment, text("Uncommitted"));
    for missing_client in [true, false] {
        let mut incomplete = body.clone();
        if missing_client {
            incomplete.client_id = None;
        } else {
            incomplete.expected_seq = None;
        }
        assert_eq!(
            edit(&desk, &author, document, comment.id, &incomplete)
                .await
                .problem()
                .code,
            ErrorCode::RequestInvalid
        );
    }
    let mut negative = body.clone();
    negative.expected_seq = Some(-1);
    assert_eq!(
        edit(&desk, &author, document, comment.id, &negative)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let before = state(&desk, document, comment.id).await?;
    assert_eq!(before.comment.seq_updated, comment.seq_updated);
    let legacy = DocumentCommentUpdateBody {
        content: text("Legacy edit"),
        client_id: None,
        expected_seq: None,
    };
    let first = edit(&desk, &author, document, comment.id, &legacy)
        .await
        .ok();
    let second = edit(&desk, &author, document, comment.id, &legacy)
        .await
        .ok();
    assert_eq!(second.seq_updated, first.seq_updated + 1);
    assert_eq!(
        state(&desk, document, comment.id).await?.document_seq,
        before.document_seq + 2
    );
    Ok(())
}
