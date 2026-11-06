//! Existing author/editor/manager comment acts use their own current Document scopes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;
mod thread_history_scope;

use desk::{Desk, text};
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentCommentCreate, DocumentCommentCreateBody, DocumentCommentDelete, DocumentCommentUpdate,
    DocumentCommentUpdateBody, DocumentDiscussion, DocumentDiscussionCreate,
    DocumentDiscussionCreateBody, DocumentDiscussionResolve, DocumentDiscussionResolveBody, Inline,
    InlineRefKind,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::documents::{self as documents, discussions};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;
use thread_history_scope::{key, narrow, rule};

struct Scene {
    desk: Desk,
    author: Who,
    other: Who,
    document: Id,
    discussion: Id,
    comment: Id,
}

impl Scene {
    async fn new() -> Self {
        let desk = Desk::new().await;
        let author = desk.member("comment_author", "viewer").await;
        let other = desk.member("other_viewer", "viewer").await;
        let document = desk
            .make(&desk.ada, "Actual comment writer document")
            .await
            .base
            .id;
        let block = desk
            .paragraphs(&desk.ada, document, &["Anchor words."])
            .await[0]
            .id;
        let opened: DocumentDiscussion = desk
            .world
            .app
            .call(DocumentDiscussionCreate, &[&document])
            .token(&author.token)
            .body(&DocumentDiscussionCreateBody {
                block_id: block,
                quote: Some("Anchor".into()),
                content: text("Original viewer-authored words"),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .ok();
        Self {
            desk,
            author,
            other,
            document,
            discussion: opened.id,
            comment: opened.comments[0].id,
        }
    }

    async fn full_key(&self, who: &Who, operation: &str) -> thread_history_scope::Key {
        key(
            &self.desk.world,
            who.id(),
            vec![
                rule(operation, "document", self.document, "self", "write"),
                rule(operation, "document", self.document, "self", "read"),
            ],
        )
        .await
        .unwrap()
    }
}

fn edit_body(words: &str) -> DocumentCommentUpdateBody {
    DocumentCommentUpdateBody {
        content: text(words),
        client_id: None,
        expected_seq: None,
    }
}

async fn edit(
    s: &Scene,
    token: &str,
    document: Id,
    comment: Id,
    body: &DocumentCommentUpdateBody,
) -> RawReply {
    s.desk
        .world
        .app
        .call(DocumentCommentUpdate, &[&document, &comment])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
async fn resolve(s: &Scene, token: &str, document: Id, discussion: Id, resolved: bool) -> RawReply {
    s.desk
        .world
        .app
        .call(DocumentDiscussionResolve, &[&document, &discussion])
        .token(token)
        .body(&DocumentDiscussionResolveBody { resolved })
        .send_bytes()
        .await
}
async fn delete(s: &Scene, token: &str, document: Id, comment: Id) -> RawReply {
    s.desk
        .world
        .app
        .call(DocumentCommentDelete, &[&document, &comment])
        .token(token)
        .send_bytes()
        .await
}
#[track_caller]
fn shown(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn denied(reply: &RawReply) {
    assert_eq!(
        reply.status.as_u16(),
        403,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
fn empty(reply: &RawReply) {
    assert_eq!(
        reply.status.as_u16(),
        204,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(reply.raw.is_empty());
}
fn said(value: &Json) -> String {
    value["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|run| run["text"].as_str())
        .collect()
}
async fn state(s: &Scene) -> (i64, discussions::Full) {
    let mut conn = s.desk.world.conn().await;
    let document = documents::get(&mut conn, s.document)
        .await
        .unwrap()
        .unwrap();
    let full = discussions::get(&mut conn, s.document, s.discussion)
        .await
        .unwrap()
        .unwrap();
    (document.doc_seq, full)
}
async fn moderator_audits(s: &Scene) -> usize {
    s.desk
        .world
        .audit()
        .await
        .iter()
        .filter(|action| action.as_str() == "document.comment_deleted")
        .count()
}

#[tokio::test]
async fn a_viewer_author_updates_their_comment_with_own_document_write_and_read_without_doc_edit() {
    let s = Scene::new().await;
    let ordinary = shown(
        &edit(
            &s,
            &s.author.token,
            s.document,
            s.comment,
            &edit_body("Ordinary author edit"),
        )
        .await,
    );
    assert_eq!(said(&ordinary), "Ordinary author edit");
    let not_author = edit(
        &s,
        &s.desk.ada.token,
        s.document,
        s.comment,
        &edit_body("Owner is not author"),
    )
    .await;
    denied(&not_author);
    assert_eq!(not_author.problem().extra["required"], "comment.author");
    let selected = s.full_key(&s.author, "DocumentCommentUpdate").await;
    let answer = shown(
        &edit(
            &s,
            &selected.secret,
            s.document,
            s.comment,
            &edit_body("Key author edit"),
        )
        .await,
    );
    assert_eq!(said(&answer), "Key author edit");
    assert_eq!(answer["author"]["id"], json!(s.author.id()));
    assert_eq!(answer["id"], json!(s.comment));
    assert!(answer["edited_at"].is_string());
    assert_eq!(
        state(&s).await.1.comments[0].content,
        serde_json::to_value(text("Key author edit")).unwrap()
    );
}

#[tokio::test]
async fn a_viewer_participant_resolves_and_reopens_without_an_added_doc_edit_gate() {
    let s = Scene::new().await;
    let outsider = resolve(&s, &s.other.token, s.document, s.discussion, true).await;
    denied(&outsider);
    assert_eq!(outsider.problem().extra["required"], "doc.edit");
    let prime = shown(&resolve(&s, &s.author.token, s.document, s.discussion, true).await);
    assert_eq!(prime["resolved"]["by"]["id"], json!(s.author.id()));
    let selected = s.full_key(&s.author, "DocumentDiscussionResolve").await;
    let opened = shown(&resolve(&s, &selected.secret, s.document, s.discussion, false).await);
    assert!(opened["resolved"].is_null());
    let resolved = shown(&resolve(&s, &selected.secret, s.document, s.discussion, true).await);
    assert_eq!(resolved["resolved"]["by"]["id"], json!(s.author.id()));
    assert_eq!(resolved["quote"], "Anchor");
    assert_eq!(
        resolved["comments"][0]["author"]["id"],
        json!(s.author.id())
    );
}

#[tokio::test]
async fn an_editor_who_never_wrote_in_the_discussion_keeps_the_native_resolve_branch() {
    let s = Scene::new().await;
    let wrote = discussions::wrote_in(
        &mut *s.desk.world.conn().await,
        s.discussion,
        s.desk.ada.id(),
    )
    .await
    .unwrap();
    assert!(!wrote);
    let ordinary = shown(&resolve(&s, &s.desk.ada.token, s.document, s.discussion, true).await);
    assert_eq!(ordinary["resolved"]["by"]["id"], json!(s.desk.ada.id()));
    // The existing legacy ceiling can allow DocEdit while excluding DocView. Resolve's
    // successful editor branch must not acquire the author fallback's extra View check.
    let legacy = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: s.desk.ada.id(),
            name: "Native resolve editor-only ceiling",
            token_hash: &token_hash(&legacy),
            scopes: &["doc.edit".to_owned()],
            node_id: None,
            at: s.desk.world.now(),
            expires_at: Some(s.desk.world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    let legacy_full = shown(&resolve(&s, &legacy, s.document, s.discussion, false).await);
    assert!(legacy_full["resolved"].is_null());
    let selected = s.full_key(&s.desk.ada, "DocumentDiscussionResolve").await;
    let opened = shown(&resolve(&s, &selected.secret, s.document, s.discussion, false).await);
    assert!(opened["resolved"].is_null());
    assert_eq!(opened["comments"][0]["id"], json!(s.comment));
}

#[tokio::test]
async fn own_comment_delete_uses_its_write_variant_and_preserves_empty_idempotent_no_audit_result()
{
    let s = Scene::new().await;
    let own = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentCommentDelete.own",
            "document",
            s.document,
            "self",
            "write",
        )],
    )
    .await
    .unwrap();
    let ordinary = s
        .desk
        .world
        .app
        .call(DocumentCommentCreate, &[&s.document, &s.discussion])
        .token(&s.author.token)
        .body(&DocumentCommentCreateBody {
            content: text("Ordinary author-delete premise"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let before = moderator_audits(&s).await;
    empty(&delete(&s, &s.author.token, s.document, ordinary.id).await);
    assert_eq!(moderator_audits(&s).await, before);
    empty(&delete(&s, &own.secret, s.document, s.comment).await);
    let after = state(&s).await;
    assert!(after.1.comments[0].deleted_at.is_some());
    assert_eq!(after.1.comments[0].content, json!([]));
    assert_eq!(moderator_audits(&s).await, before);
    empty(&delete(&s, &own.secret, s.document, s.comment).await);
    assert_eq!(state(&s).await, after);
    assert_eq!(moderator_audits(&s).await, before);
    let wrong_variant = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentCommentDelete.moderate",
            "document",
            s.document,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    denied(&delete(&s, &wrong_variant.secret, s.document, s.comment).await);
}

#[tokio::test]
async fn moderating_another_authors_comment_requires_the_actual_manage_variant_and_audits_once() {
    let s = Scene::new().await;
    let extra = s
        .desk
        .world
        .app
        .call(DocumentCommentCreate, &[&s.document, &s.discussion])
        .token(&s.author.token)
        .body(&DocumentCommentCreateBody {
            content: text("Ordinary moderator premise"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let before = moderator_audits(&s).await;
    empty(&delete(&s, &s.desk.ada.token, s.document, extra.id).await);
    assert_eq!(moderator_audits(&s).await, before + 1);
    let selected = key(
        &s.desk.world,
        s.desk.ada.id(),
        vec![rule(
            "DocumentCommentDelete.moderate",
            "document",
            s.document,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    empty(&delete(&s, &selected.secret, s.document, s.comment).await);
    let after = state(&s).await;
    assert_eq!(moderator_audits(&s).await, before + 2);
    empty(&delete(&s, &selected.secret, s.document, s.comment).await);
    assert_eq!(state(&s).await, after);
    assert_eq!(moderator_audits(&s).await, before + 2);
    let own_only = key(
        &s.desk.world,
        s.desk.ada.id(),
        vec![rule(
            "DocumentCommentDelete.own",
            "document",
            s.document,
            "self",
            "write",
        )],
    )
    .await
    .unwrap();
    denied(&delete(&s, &own_only.secret, s.document, s.comment).await);
}

#[tokio::test]
async fn write_only_comment_update_commits_and_returns_a_truthful_content_free_receipt() {
    let s = Scene::new().await;
    assert_eq!(
        said(&shown(
            &edit(
                &s,
                &s.author.token,
                s.document,
                s.comment,
                &edit_body("Ordinary first")
            )
            .await
        )),
        "Ordinary first"
    );
    let selected = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentCommentUpdate",
            "document",
            s.document,
            "self",
            "write",
        )],
    )
    .await
    .unwrap();
    let receipt = shown(
        &edit(
            &s,
            &selected.secret,
            s.document,
            s.comment,
            &edit_body("Accepted write-only words"),
        )
        .await,
    );
    let actual = state(&s).await;
    assert_eq!(
        actual.1.comments[0].content,
        serde_json::to_value(text("Accepted write-only words")).unwrap()
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["document_id"], json!(s.document));
    assert_eq!(receipt["comment_id"], json!(s.comment));
    assert_eq!(receipt["seq"], actual.1.comments[0].seq_updated);
    for absent in [
        "content",
        "author",
        "reactions",
        "discussion",
        "current_comment",
    ] {
        assert!(receipt.get(absent).is_none());
    }
    assert!(
        !String::from_utf8_lossy(&serde_json::to_vec(&receipt).unwrap())
            .contains("Accepted write-only words")
    );
}

#[tokio::test]
async fn write_only_discussion_resolve_commits_and_returns_its_actual_outcome_without_old_comments()
{
    let s = Scene::new().await;
    shown(&resolve(&s, &s.author.token, s.document, s.discussion, false).await);
    let selected = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentDiscussionResolve",
            "document",
            s.document,
            "self",
            "write",
        )],
    )
    .await
    .unwrap();
    let receipt = shown(&resolve(&s, &selected.secret, s.document, s.discussion, true).await);
    let actual = state(&s).await;
    assert!(actual.1.discussion.resolved_at.is_some());
    assert_eq!(actual.1.discussion.resolved_by, Some(s.author.id()));
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["document_id"], json!(s.document));
    assert_eq!(receipt["discussion_id"], json!(s.discussion));
    assert_eq!(receipt["seq"], actual.1.discussion.seq_updated);
    for absent in ["comments", "created_by", "resolved", "quote", "block_id"] {
        assert!(receipt.get(absent).is_none());
    }
}

fn reference(id: Id, label: Option<&str>) -> Inline {
    Inline::Ref {
        display: None,
        kind: InlineRefKind::Document,
        id,
        path: None,
        part: None,
        label: label.map(str::to_owned),
        name: None,
    }
}
fn ref_run(value: &Json) -> &Json {
    value["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["type"] == "ref")
        .unwrap()
}

#[tokio::test]
async fn updated_and_resolved_comments_bound_copied_and_live_labels_to_the_same_actual_operation() {
    const FOREIGN: &str = "Protected foreign document title";
    let s = Scene::new().await;
    let foreign = s.desk.make(&s.desk.ada, FOREIGN).await.base.id;
    let body = DocumentCommentUpdateBody {
        content: vec![
            reference(foreign, None),
            Inline::Mention {
                user_id: s.other.id(),
                user: None,
            },
        ],
        client_id: None,
        expected_seq: None,
    };
    let ordinary = shown(&edit(&s, &s.author.token, s.document, s.comment, &body).await);
    assert_eq!(ref_run(&ordinary)["name"], FOREIGN);
    assert_eq!(ref_run(&ordinary)["label"], FOREIGN);
    let selected = s.full_key(&s.author, "DocumentCommentUpdate").await;
    let bounded = shown(&edit(&s, &selected.secret, s.document, s.comment, &body).await);
    assert!(ref_run(&bounded)["name"].is_null());
    assert!(
        ref_run(&bounded)["label"].is_null(),
        "ungranted name must not become copied stored content"
    );
    let stored = &state(&s).await.1.comments[0].content;
    assert!(stored[0]["label"].is_null());
    assert_eq!(bounded["content"][1]["user"]["id"], json!(s.other.id()));

    let same_operation = key(
        &s.desk.world,
        s.author.id(),
        vec![
            rule(
                "DocumentCommentUpdate",
                "document",
                s.document,
                "self",
                "write",
            ),
            rule(
                "DocumentCommentUpdate",
                "document",
                s.document,
                "self",
                "read",
            ),
            rule("DocumentCommentUpdate", "document", foreign, "self", "read"),
        ],
    )
    .await
    .unwrap();
    let rich = shown(&edit(&s, &same_operation.secret, s.document, s.comment, &body).await);
    assert_eq!(ref_run(&rich)["name"], FOREIGN);
    assert_eq!(ref_run(&rich)["label"], FOREIGN);
    let explicit = DocumentCommentUpdateBody {
        content: vec![reference(foreign, Some("Supplied stored label"))],
        ..edit_body("")
    };
    let stored = shown(&edit(&s, &selected.secret, s.document, s.comment, &explicit).await);
    assert_eq!(ref_run(&stored)["label"], "Supplied stored label");
    assert!(ref_run(&stored)["name"].is_null());
    let resolve_key = s.full_key(&s.author, "DocumentDiscussionResolve").await;
    let resolved = shown(&resolve(&s, &resolve_key.secret, s.document, s.discussion, true).await);
    assert_eq!(
        ref_run(&resolved["comments"][0])["label"],
        "Supplied stored label"
    );
    assert!(ref_run(&resolved["comments"][0])["name"].is_null());
    let other_operation = key(
        &s.desk.world,
        s.author.id(),
        vec![
            rule(
                "DocumentDiscussionResolve",
                "document",
                s.document,
                "self",
                "write",
            ),
            rule(
                "DocumentDiscussionResolve",
                "document",
                s.document,
                "self",
                "read",
            ),
            rule("DocumentCommentUpdate", "document", foreign, "self", "read"),
        ],
    )
    .await
    .unwrap();
    let no_loan =
        shown(&resolve(&s, &other_operation.secret, s.document, s.discussion, false).await);
    assert!(ref_run(&no_loan["comments"][0])["name"].is_null());
}

#[tokio::test]
async fn exact_comment_edit_replay_and_stale_errors_use_the_selected_stored_result_and_current_read()
 {
    let s = Scene::new().await;
    let initial = state(&s).await.1.comments[0].clone();
    let attempt = DocumentCommentUpdateBody {
        content: text("Recorded exact first attempt"),
        client_id: Some(new_id().to_string()),
        expected_seq: Some(initial.seq_updated),
    };
    let recorded = shown(&edit(&s, &s.author.token, s.document, s.comment, &attempt).await);
    shown(
        &edit(
            &s,
            &s.author.token,
            s.document,
            s.comment,
            &edit_body("Newer current comment must not be overwritten"),
        )
        .await,
    );
    let before = state(&s).await;
    let selected = s.full_key(&s.author, "DocumentCommentUpdate").await;
    let repeated = shown(&edit(&s, &selected.secret, s.document, s.comment, &attempt).await);
    assert_eq!(repeated, recorded);
    assert_eq!(state(&s).await, before);
    let write_only = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentCommentUpdate",
            "document",
            s.document,
            "self",
            "write",
        )],
    )
    .await
    .unwrap();
    let receipt = shown(&edit(&s, &write_only.secret, s.document, s.comment, &attempt).await);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["seq"], recorded["seq_updated"]);
    assert_eq!(state(&s).await, before);
    let stale = DocumentCommentUpdateBody {
        client_id: Some(new_id().to_string()),
        ..attempt.clone()
    };
    let conflict = edit(&s, &selected.secret, s.document, s.comment, &stale).await;
    assert_eq!(conflict.status.as_u16(), 409);
    assert_eq!(conflict.problem().code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        said(&conflict.problem().extra["current_comment"]),
        "Newer current comment must not be overwritten"
    );
    let bounded = edit(&s, &write_only.secret, s.document, s.comment, &stale).await;
    assert_eq!(bounded.status.as_u16(), 409);
    assert_eq!(bounded.problem().code, ErrorCode::ConflictStaleRevision);
    assert!(bounded.problem().extra.get("current_comment").is_none());
    assert!(!String::from_utf8_lossy(&bounded.raw).contains("Newer current comment"));
    assert_eq!(state(&s).await, before);
}

#[tokio::test]
async fn comment_operations_do_not_loan_targets_variants_or_native_authority_and_keep_real_pair_404s()
 {
    let s = Scene::new().await;
    let selected = s.full_key(&s.author, "DocumentCommentUpdate").await;
    shown(
        &edit(
            &s,
            &s.author.token,
            s.document,
            s.comment,
            &edit_body("Ordinary pair prime"),
        )
        .await,
    );
    let foreign = s
        .desk
        .make(&s.desk.ada, "Another actual document")
        .await
        .base
        .id;
    let foreign_block = s
        .desk
        .paragraphs(&s.desk.ada, foreign, &["Other document anchor"])
        .await[0]
        .id;
    let foreign_discussion = s
        .desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&foreign])
        .token(&s.author.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: foreign_block,
            quote: None,
            content: text("A real comment associated only with the other document"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let foreign_comment = foreign_discussion.comments[0].id;
    let ordinary_pair = edit(
        &s,
        &s.author.token,
        s.document,
        foreign_comment,
        &edit_body("Other actual association"),
    )
    .await;
    assert_eq!(ordinary_pair.status.as_u16(), 404);
    assert_eq!(ordinary_pair.problem().code, ErrorCode::NotFound);
    let ordinary_discussion_pair =
        resolve(&s, &s.author.token, s.document, foreign_discussion.id, true).await;
    assert_eq!(ordinary_discussion_pair.status.as_u16(), 404);
    assert_eq!(ordinary_discussion_pair.problem().code, ErrorCode::NotFound);
    shown(
        &edit(
            &s,
            &selected.secret,
            s.document,
            s.comment,
            &edit_body("Modern pair prime"),
        )
        .await,
    );
    let before = state(&s).await;
    let wrong_doc = key(
        &s.desk.world,
        s.author.id(),
        vec![
            rule(
                "DocumentCommentUpdate",
                "document",
                foreign,
                "self",
                "write",
            ),
            rule("DocumentCommentUpdate", "document", foreign, "self", "read"),
        ],
    )
    .await
    .unwrap();
    denied(
        &edit(
            &s,
            &wrong_doc.secret,
            s.document,
            s.comment,
            &edit_body("Wrong document"),
        )
        .await,
    );
    let read_only = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentCommentUpdate",
            "document",
            s.document,
            "self",
            "read",
        )],
    )
    .await
    .unwrap();
    denied(
        &edit(
            &s,
            &read_only.secret,
            s.document,
            s.comment,
            &edit_body("Read is not write"),
        )
        .await,
    );
    let wrong_operation = s.full_key(&s.author, "DocumentDiscussionResolve").await;
    denied(
        &edit(
            &s,
            &wrong_operation.secret,
            s.document,
            s.comment,
            &edit_body("Wrong operation"),
        )
        .await,
    );
    let other_author = s.full_key(&s.other, "DocumentCommentUpdate").await;
    let not_author = edit(
        &s,
        &other_author.secret,
        s.document,
        s.comment,
        &edit_body("Key does not confer authorship"),
    )
    .await;
    denied(&not_author);
    assert_eq!(not_author.problem().extra["required"], "comment.author");
    let nonparticipant = s.full_key(&s.other, "DocumentDiscussionResolve").await;
    denied(&resolve(&s, &nonparticipant.secret, s.document, s.discussion, true).await);
    let nonmanager = key(
        &s.desk.world,
        s.other.id(),
        vec![rule(
            "DocumentCommentDelete.moderate",
            "document",
            s.document,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    denied(&delete(&s, &nonmanager.secret, s.document, s.comment).await);
    for comment in [new_id(), foreign_comment] {
        let missing = edit(
            &s,
            &selected.secret,
            s.document,
            comment,
            &edit_body("Unknown associated comment"),
        )
        .await;
        assert_eq!(missing.status.as_u16(), 404);
        assert_eq!(missing.problem().code, ErrorCode::NotFound);
    }
    let resolve_pair = s.full_key(&s.author, "DocumentDiscussionResolve").await;
    let missing = resolve(
        &s,
        &resolve_pair.secret,
        s.document,
        foreign_discussion.id,
        true,
    )
    .await;
    assert_eq!(missing.status.as_u16(), 404);
    assert_eq!(missing.problem().code, ErrorCode::NotFound);
    assert_eq!(state(&s).await, before);
}

#[tokio::test]
async fn current_key_scope_revocation_and_current_ordinary_doc_view_prevent_later_comment_writes() {
    let s = Scene::new().await;
    shown(
        &edit(
            &s,
            &s.author.token,
            s.document,
            s.comment,
            &edit_body("Ordinary current-rights prime"),
        )
        .await,
    );
    let selected = s.full_key(&s.author, "DocumentCommentUpdate").await;
    shown(
        &edit(
            &s,
            &selected.secret,
            s.document,
            s.comment,
            &edit_body("Actual accepted modern edit"),
        )
        .await,
    );
    let before = state(&s).await;
    narrow(
        &s.desk.world,
        &selected,
        vec![rule(
            "DocumentCommentUpdate",
            "document",
            s.document,
            "self",
            "read",
        )],
    )
    .await
    .unwrap();
    denied(
        &edit(
            &s,
            &selected.secret,
            s.document,
            s.comment,
            &edit_body("Narrowed key cannot write"),
        )
        .await,
    );
    assert_eq!(state(&s).await, before);
    let revoked = s.full_key(&s.author, "DocumentCommentUpdate").await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(revoked.id)
        .bind(s.desk.world.now())
        .execute(&s.desk.world.db.pool)
        .await
        .unwrap();
    let reply = edit(
        &s,
        &revoked.secret,
        s.document,
        s.comment,
        &edit_body("Revoked key cannot write"),
    )
    .await;
    assert_eq!(reply.status.as_u16(), 401);
    assert_eq!(reply.problem().code, ErrorCode::AuthInvalidToken);
    let current = s.full_key(&s.author, "DocumentCommentUpdate").await;
    s.desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &s.document])
        .token(&s.desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    // Changing the permission page legitimately advances doc_seq, not comment content.
    let after_view_withdrawal = state(&s).await;
    assert_eq!(after_view_withdrawal.1, before.1);
    denied(
        &edit(
            &s,
            &s.author.token,
            s.document,
            s.comment,
            &edit_body("Native View gone"),
        )
        .await,
    );
    denied(
        &edit(
            &s,
            &current.secret,
            s.document,
            s.comment,
            &edit_body("Scope cannot replace View"),
        )
        .await,
    );
    assert_eq!(state(&s).await, after_view_withdrawal);
}

#[tokio::test]
async fn archived_documents_preserve_native_author_editor_and_moderator_refusals_without_inspection_upgrade()
 {
    use support::FullArchiveReply as _;
    let s = Scene::new().await;
    let update = s.full_key(&s.author, "DocumentCommentUpdate").await;
    let resolve_key = s.full_key(&s.author, "DocumentDiscussionResolve").await;
    let own = key(
        &s.desk.world,
        s.author.id(),
        vec![rule(
            "DocumentCommentDelete.own",
            "document",
            s.document,
            "self",
            "write",
        )],
    )
    .await
    .unwrap();
    let moderate = key(
        &s.desk.world,
        s.desk.ada.id(),
        vec![rule(
            "DocumentCommentDelete.moderate",
            "document",
            s.document,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    s.desk
        .world
        .app
        .call(orch_api_contract::nodes::NodeArchive, &[&s.desk.topic])
        .token(&s.desk.ada.token)
        .body(&places::archive_body(&s.desk.world, &s.desk.ada.token, s.desk.topic, true).await)
        .send()
        .await
        .full_archive()
        .ok();
    let before = state(&s).await;
    let ordinary = [
        edit(
            &s,
            &s.author.token,
            s.document,
            s.comment,
            &edit_body("Archived ordinary edit"),
        )
        .await,
        resolve(&s, &s.author.token, s.document, s.discussion, true).await,
        delete(&s, &s.author.token, s.document, s.comment).await,
        delete(&s, &s.desk.ada.token, s.document, s.comment).await,
    ];
    for reply in ordinary {
        assert_eq!(reply.problem().code, ErrorCode::NodeArchived);
    }
    let scoped = [
        edit(
            &s,
            &update.secret,
            s.document,
            s.comment,
            &edit_body("Archived scoped edit"),
        )
        .await,
        resolve(&s, &resolve_key.secret, s.document, s.discussion, true).await,
        delete(&s, &own.secret, s.document, s.comment).await,
        delete(&s, &moderate.secret, s.document, s.comment).await,
    ];
    for reply in scoped {
        assert_eq!(reply.status.as_u16(), 409);
        assert_eq!(reply.problem().code, ErrorCode::NodeArchived);
    }
    assert_eq!(state(&s).await, before);
}
