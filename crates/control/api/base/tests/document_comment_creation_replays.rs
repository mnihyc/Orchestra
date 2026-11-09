//! Current native creation replay keeps comments once and never borrows spent Write.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
mod places;
mod support;
mod thread_history_scope;
use desk::Desk;
use http::StatusCode;
use orch_api_contract::{documents::*, routes::Endpoint};
use orch_api_core::testing::RawReply;
use orch_types::{Id, Json, new_id};
use serde_json::json;

async fn retry<E: Endpoint>(
    d: &Desk,
    endpoint: E,
    token: &str,
    act: &str,
    document: Id,
    discussion: Option<Id>,
    body: &Json,
) -> RawReply {
    let call = match discussion {
        Some(discussion) => d.world.app.call(endpoint, &[&document, &discussion]),
        None => d.world.app.call(endpoint, &[&document]),
    };
    call.token(token)
        .header("Idempotency-Key", act)
        .raw_body(body)
        .send_bytes()
        .await
}
fn value(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
#[tokio::test]
async fn discussion_and_comment_keys_read_current_content_and_keep_native_body_identities() {
    let d = Desk::new().await;
    let doc = d.make(&d.ada, "Native keyed comments").await;
    let block = d
        .paragraphs(&d.ada, doc.base.id, &["The actual anchor"])
        .await[0]
        .id;
    let act = new_id().to_string();
    let body = json!({"block_id":block,"quote":null,"content":desk::text("First native discussion"),"client_id":new_id().to_string()});
    let first = value(
        retry(
            &d,
            DocumentDiscussionCreate,
            &d.ada.token,
            &act,
            doc.base.id,
            None,
            &body,
        )
        .await,
    );
    let discussion: DocumentDiscussion = serde_json::from_value(first).unwrap();
    let comment_act = new_id().to_string();
    let comment_body =
        json!({"content":desk::text("A native reply"),"client_id":new_id().to_string()});
    let first = value(
        retry(
            &d,
            DocumentCommentCreate,
            &d.ada.token,
            &comment_act,
            doc.base.id,
            Some(discussion.id),
            &comment_body,
        )
        .await,
    );
    let comment: DocumentComment = serde_json::from_value(first).unwrap();
    let edit = d
        .world
        .app
        .call(DocumentCommentUpdate, &[&doc.base.id, &comment.id]);
    assert!(
        edit.token(&d.ada.token)
            .raw_body(&json!({"content":desk::text("Current native reply")}))
            .send_bytes()
            .await
            .status
            .is_success()
    );
    let reader = thread_history_scope::key(
        &d.world,
        d.ada.id(),
        vec![
            thread_history_scope::rule(
                "DocumentDiscussionCreate",
                "document",
                doc.base.id,
                "self",
                "read",
            ),
            thread_history_scope::rule(
                "DocumentCommentCreate",
                "document",
                doc.base.id,
                "self",
                "read",
            ),
        ],
    )
    .await
    .unwrap();
    let current = value(
        retry(
            &d,
            DocumentDiscussionCreate,
            &reader.secret,
            &act,
            doc.base.id,
            None,
            &body,
        )
        .await,
    );
    let current = serde_json::from_value::<DocumentDiscussionCreateResponse>(current)
        .unwrap()
        .into_full()
        .unwrap();
    assert_eq!(current.id, discussion.id);
    assert_eq!(current.comments.len(), 2);
    assert!(
        serde_json::to_string(&current)
            .unwrap()
            .contains("Current native reply")
    );
    let current = value(
        retry(
            &d,
            DocumentCommentCreate,
            &reader.secret,
            &comment_act,
            doc.base.id,
            Some(discussion.id),
            &comment_body,
        )
        .await,
    );
    let current = serde_json::from_value::<DocumentCommentCreateResponse>(current)
        .unwrap()
        .into_full()
        .unwrap();
    assert_eq!(current.id, comment.id);
    assert!(
        serde_json::to_string(&current)
            .unwrap()
            .contains("Current native reply")
    );
    let wrong = thread_history_scope::key(
        &d.world,
        d.ada.id(),
        vec![thread_history_scope::rule(
            "DocumentSyncGet.read",
            "document",
            doc.base.id,
            "self",
            "read",
        )],
    )
    .await
    .unwrap();
    let receipt = value(
        retry(
            &d,
            DocumentDiscussionCreate,
            &wrong.secret,
            &act,
            doc.base.id,
            None,
            &body,
        )
        .await,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["discussion_id"], json!(discussion.id));
    assert!(!receipt.to_string().contains("native reply"));
    let receipt = value(
        retry(
            &d,
            DocumentCommentCreate,
            &wrong.secret,
            &comment_act,
            doc.base.id,
            Some(discussion.id),
            &comment_body,
        )
        .await,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["comment_id"], json!(comment.id));
    assert!(
        retry(
            &d,
            DocumentCommentCreate,
            &reader.secret,
            &new_id().to_string(),
            doc.base.id,
            Some(discussion.id),
            &comment_body
        )
        .await
        .status
        .is_client_error()
    );
    let repeated = value(
        retry(
            &d,
            DocumentCommentCreate,
            &d.ada.token,
            &new_id().to_string(),
            doc.base.id,
            Some(discussion.id),
            &comment_body,
        )
        .await,
    );
    assert_eq!(
        repeated["id"],
        json!(comment.id),
        "a new header preserves native body client_id replay"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM document_discussions WHERE document_id=$1"
        )
        .bind(doc.base.id)
        .fetch_one(&d.world.db.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_comments WHERE document_id=$1")
            .bind(doc.base.id)
            .fetch_one(&d.world.db.pool)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn a_refused_native_creation_cut_changes_no_sequence_or_comment_on_the_open_transaction() {
    use orch_db::{
        DbError,
        queries::documents::discussions::{self as rows, NewComment, Opened},
    };
    let d = Desk::new().await;
    let doc = d.make(&d.ada, "Guarded native discussion").await;
    let block = d.paragraphs(&d.ada, doc.base.id, &["Native anchor"]).await[0].id;
    let mut tx = d.world.db.pool.begin().await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT doc_seq FROM documents WHERE id=$1")
        .bind(doc.base.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let content = json!(desk::text("One native first comment"));
    let id = new_id();
    let first = NewComment {
        id,
        content: &content,
    };
    let refused = rows::open_before(
        &mut tx,
        doc.base.id,
        block,
        "",
        d.ada.id(),
        &first,
        d.world.now(),
        || {
            Err(DbError::Other(sqlx::Error::Protocol(
                "the source ended".into(),
            )))
        },
    )
    .await;
    assert!(refused.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT doc_seq FROM documents WHERE id=$1")
            .bind(doc.base.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_comments WHERE document_id=$1")
            .bind(doc.base.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        0
    );
    assert!(matches!(
        rows::open_before(
            &mut tx,
            doc.base.id,
            block,
            "",
            d.ada.id(),
            &first,
            d.world.now(),
            || Ok::<(), DbError>(())
        )
        .await
        .unwrap(),
        Opened::Created(_)
    ));
    assert!(matches!(
        rows::open_before(
            &mut tx,
            doc.base.id,
            block,
            "",
            d.ada.id(),
            &first,
            d.world.now(),
            || Err(DbError::Other(sqlx::Error::Protocol(
                "a no-op consumes no new cut".into()
            )))
        )
        .await
        .unwrap(),
        Opened::Repeated(_)
    ));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn write_only_comment_creators_keep_both_acts_and_return_only_native_identities() {
    let d = Desk::new().await;
    let doc = d.make(&d.ada, "Write-only native comments").await;
    let block = d.paragraphs(&d.ada, doc.base.id, &["Native anchor"]).await[0].id;
    let writer = thread_history_scope::key(
        &d.world,
        d.ada.id(),
        vec![
            thread_history_scope::rule(
                "DocumentDiscussionCreate",
                "document",
                doc.base.id,
                "self",
                "write",
            ),
            thread_history_scope::rule(
                "DocumentCommentCreate",
                "document",
                doc.base.id,
                "self",
                "write",
            ),
        ],
    )
    .await
    .unwrap();
    let discussion=value(retry(&d,DocumentDiscussionCreate,&writer.secret,&new_id().to_string(),doc.base.id,None,&json!({"block_id":block,"quote":null,"content":desk::text("Supplied private first comment"),"client_id":new_id().to_string()})).await);
    assert_eq!(discussion["projection"], "receipt");
    assert_eq!(discussion.as_object().unwrap().len(), 4);
    let id: Id = serde_json::from_value(discussion["discussion_id"].clone()).unwrap();
    let body = json!({"content":desk::text("Supplied private native reply"),"client_id":new_id().to_string()});
    let act = new_id().to_string();
    let comment = value(
        retry(
            &d,
            DocumentCommentCreate,
            &writer.secret,
            &act,
            doc.base.id,
            Some(id),
            &body,
        )
        .await,
    );
    assert_eq!(comment["projection"], "receipt");
    assert_eq!(comment.as_object().unwrap().len(), 4);
    assert!(!comment.to_string().contains("private native reply"));
    let repeated = value(
        retry(
            &d,
            DocumentCommentCreate,
            &writer.secret,
            &act,
            doc.base.id,
            Some(id),
            &body,
        )
        .await,
    );
    assert_eq!(repeated, comment);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_comments WHERE document_id=$1")
            .bind(doc.base.id)
            .fetch_one(&d.world.db.pool)
            .await
            .unwrap(),
        2
    );
}
