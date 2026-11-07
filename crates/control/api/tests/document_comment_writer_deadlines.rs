//! Current credential deadlines survive a real wait for the existing document row writer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, text};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentCommentCreate, DocumentCommentCreateBody, DocumentCommentDelete, DocumentDiscussion,
    DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentDiscussionResolve,
    DocumentDiscussionResolveBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{
    documents,
    documents::discussions,
    identity::{self, NewApiToken},
};
use orch_types::{Id, new_id};
use std::time::Duration;

// The existing reader rendezvous allowance, not a race-selection delay or credential lifetime.
const WAIT: Duration = Duration::from_secs(25);

#[derive(Clone, Copy)]
enum Act {
    Resolve,
    DeleteOwn,
    DeleteModerate,
}

impl Act {
    async fn call(
        self,
        desk: &Desk,
        document: Id,
        discussion: Id,
        comment: Id,
        token: &str,
    ) -> RawReply {
        match self {
            Self::Resolve => {
                desk.world
                    .app
                    .call(DocumentDiscussionResolve, &[&document, &discussion])
                    .token(token)
                    .body(&DocumentDiscussionResolveBody { resolved: true })
                    .send_bytes()
                    .await
            }
            Self::DeleteOwn | Self::DeleteModerate => {
                desk.world
                    .app
                    .call(DocumentCommentDelete, &[&document, &comment])
                    .token(token)
                    .send_bytes()
                    .await
            }
        }
    }
}

async fn state(desk: &Desk, document: Id, discussion: Id) -> (i64, discussions::Full, usize) {
    let mut conn = desk.world.conn().await;
    let seq = documents::get(&mut conn, document)
        .await
        .unwrap()
        .unwrap()
        .doc_seq;
    let full = discussions::get(&mut conn, document, discussion)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let audits = desk
        .world
        .audit()
        .await
        .iter()
        .filter(|action| action.as_str() == "document.comment_deleted")
        .count();
    (seq, full, audits)
}

async fn deadline_over_real_row_wait(act: Act) {
    let desk = Desk::new().await;
    let author = desk.member("deadline_author", "viewer").await;
    let document = desk
        .make(&desk.ada, "Actual document row deadline boundary")
        .await
        .base
        .id;
    let block = desk
        .paragraphs(&desk.ada, document, &["Deadline anchor."])
        .await[0]
        .id;
    let opened: DocumentDiscussion = desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&author.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block,
            quote: None,
            content: text("Original deadline-protected comment"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let person = if matches!(act, Act::DeleteModerate) {
        desk.ada.id()
    } else {
        author.id()
    };
    let token_id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let now = desk.world.now();
    // Exactly one logical clock step gives a strict initial-before/current-at-deadline pair.
    let until = now + chrono::Duration::seconds(1);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: token_id,
            user_id: person,
            name: "Legacy native row-wait deadline control",
            token_hash: &token_hash(&secret),
            scopes: &["*".to_owned()],
            node_id: None,
            at: now,
            expires_at: Some(until),
        },
        None,
    )
    .await
    .unwrap();

    // An actual allowed use of this SAME key must precede the rendezvous.
    match act {
        Act::Resolve => {
            let prime = act
                .call(&desk, document, opened.id, opened.comments[0].id, &secret)
                .await;
            assert_eq!(
                prime.status.as_u16(),
                200,
                "{}",
                String::from_utf8_lossy(&prime.raw)
            );
            desk.world
                .app
                .call(DocumentDiscussionResolve, &[&document, &opened.id])
                .token(&author.token)
                .body(&DocumentDiscussionResolveBody { resolved: false })
                .send()
                .await
                .ok();
        }
        Act::DeleteOwn | Act::DeleteModerate => {
            let extra = desk
                .world
                .app
                .call(DocumentCommentCreate, &[&document, &opened.id])
                .token(&author.token)
                .body(&DocumentCommentCreateBody {
                    content: text("Separate valid deletion prime"),
                    client_id: new_id().to_string(),
                })
                .send()
                .await
                .ok();
            let prime = act
                .call(&desk, document, opened.id, extra.id, &secret)
                .await;
            assert_eq!(
                prime.status.as_u16(),
                204,
                "{}",
                String::from_utf8_lossy(&prime.raw)
            );
            assert!(prime.raw.is_empty());
        }
    }
    assert!(desk.world.now() < until);
    let before = state(&desk, document, opened.id).await;
    let mut blocker = desk.world.db.pool.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let locked: Id = sqlx::query_scalar("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
        .bind(document)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    assert_eq!(locked, document);

    let request = act.call(&desk, document, opened.id, opened.comments[0].id, &secret);
    let expire = async {
        // This database is private to this test and has one target Document. Observe the
        // actual HTTP writer waiting on our exact blocker PID, not a fixed wall-clock sleep.
        // Both native UPDATE and the corrected native FOR UPDATE are accepted rendezvous sites.
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a
                 WHERE a.datname=current_database() AND a.wait_event_type='Lock'
                   AND $1=ANY(pg_blocking_pids(a.pid)) AND a.query LIKE '%documents%'
                   AND (a.query LIKE '%FOR UPDATE%' OR a.query LIKE '%UPDATE documents%'))",
            )
            .bind(blocker_pid)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
        desk.world.advance(chrono::Duration::seconds(1));
        assert_eq!(desk.world.now(), until);
        blocker.rollback().await.unwrap();
    };
    let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, expire) })
        .await
        .expect("actual writer did not reach/release the document row boundary");
    let after = state(&desk, document, opened.id).await;
    assert_eq!(
        after, before,
        "expired actual writer mutated native rows/audit after its row wait; HTTP {}",
        reply.status
    );
    assert_eq!(
        reply.status.as_u16(),
        401,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthInvalidToken);
}

#[tokio::test]
async fn resolving_rechecks_its_actual_key_deadline_after_the_native_document_row_wait() {
    deadline_over_real_row_wait(Act::Resolve).await;
}

#[tokio::test]
async fn own_comment_delete_rechecks_its_actual_key_deadline_after_the_document_row_wait() {
    deadline_over_real_row_wait(Act::DeleteOwn).await;
}

#[tokio::test]
async fn moderator_comment_delete_rechecks_its_actual_key_deadline_before_mutation_or_audit() {
    deadline_over_real_row_wait(Act::DeleteModerate).await;
}
