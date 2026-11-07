//! Existing discussion/comment windows retain ordinary content under their own current read scopes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use crate::support::FullPermissionPageReply;
use desk::{Desk, text};
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DiscussionResolvedFilter, DocumentCommentCreate, DocumentCommentCreateBody,
    DocumentCommentList, DocumentCommentListQuery, DocumentCommentReactionSet,
    DocumentCommentReactionSetBody, DocumentDelete, DocumentDiscussionCreate,
    DocumentDiscussionCreateBody, DocumentDiscussionList, DocumentDiscussionListQuery,
    DocumentDiscussionResolve, DocumentDiscussionResolveBody, Inline, InlineRefKind,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;

#[derive(Clone, Copy)]
enum Reader {
    Discussions,
    Comments,
}
impl Reader {
    fn selection(self, inspect: bool) -> String {
        format!(
            "{}.{}",
            match self {
                Self::Discussions => "DocumentDiscussionList",
                Self::Comments => "DocumentCommentList",
            },
            if inspect { "inspect_deleted" } else { "read" }
        )
    }
    async fn read(self, s: &Scene, token: &str, inspect: bool) -> RawReply {
        match self {
            Self::Discussions => {
                discussion_page(
                    &s.desk,
                    token,
                    s.document,
                    &DocumentDiscussionListQuery {
                        inspect_deleted: inspect.then_some(true),
                        resolved: Some(DiscussionResolvedFilter::Include),
                        ..Default::default()
                    },
                )
                .await
            }
            Self::Comments => {
                comment_page(
                    &s.desk,
                    token,
                    s.document,
                    s.discussion,
                    &DocumentCommentListQuery {
                        inspect_deleted: inspect.then_some(true),
                        ..Default::default()
                    },
                )
                .await
            }
        }
    }
    fn comments(self, page: &Json) -> Vec<&Json> {
        match self {
            Self::Comments => page["items"].as_array().unwrap().iter().collect(),
            Self::Discussions => page["items"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|discussion| discussion["comments"].as_array().unwrap())
                .collect(),
        }
    }
}
const READERS: [Reader; 2] = [Reader::Discussions, Reader::Comments];

struct Scene {
    desk: Desk,
    document: Id,
    block: Id,
    discussion: Id,
}
impl Scene {
    async fn new() -> Self {
        let desk = Desk::new().await;
        let document = desk
            .make(&desk.ada, "Document with current discussion windows")
            .await
            .base
            .id;
        let block = desk
            .paragraphs(
                &desk.ada,
                document,
                &["Anchor words remain in this paragraph."],
            )
            .await[0]
            .id;
        let discussion = open(
            &desk,
            &desk.ada,
            document,
            block,
            text("Stored first comment"),
        )
        .await;
        Self {
            desk,
            document,
            block,
            discussion,
        }
    }
}
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(desk: &Desk, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let scopes = if policy.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Document comment reader control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn open(desk: &Desk, by: &Who, document: Id, block: Id, content: Vec<Inline>) -> Id {
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&by.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block,
            quote: Some("Anchor words".into()),
            content,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .id
}
async fn answer(desk: &Desk, by: &Who, document: Id, discussion: Id, content: Vec<Inline>) -> Id {
    desk.world
        .app
        .call(DocumentCommentCreate, &[&document, &discussion])
        .token(&by.token)
        .body(&DocumentCommentCreateBody {
            content,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .id
}
async fn discussion_page(
    desk: &Desk,
    token: &str,
    document: Id,
    query: &DocumentDiscussionListQuery,
) -> RawReply {
    desk.world
        .app
        .call(DocumentDiscussionList, &[&document])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
async fn comment_page(
    desk: &Desk,
    token: &str,
    document: Id,
    discussion: Id,
    query: &DocumentCommentListQuery,
) -> RawReply {
    desk.world
        .app
        .call(DocumentCommentList, &[&document, &discussion])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
fn reference(kind: InlineRefKind, id: Id, label: &str) -> Inline {
    Inline::Ref {
        display: None,
        kind,
        id,
        path: None,
        part: None,
        label: Some(label.into()),
        name: None,
    }
}
fn assert_reference(comment: &Json, id: Id, stored: &str, live: Option<&str>) {
    let found = comment["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["type"] == "ref" && run["id"] == json!(id))
        .unwrap();
    assert_eq!(found["label"], stored);
    assert_eq!(found["name"], json!(live));
}

#[tokio::test]
async fn both_comment_readers_require_their_exact_doc_selection_and_current_ordinary_view() {
    let s = Scene::new().await;
    let viewer = s.desk.member("comment-reader", "viewer").await;
    let outsider = s.desk.stranger("no-comment-rights").await;
    let other = s
        .desk
        .make(&s.desk.ada, "Another document target")
        .await
        .base
        .id;
    for reader in READERS {
        let operation = reader.selection(false);
        let ordinary = shown(reader.read(&s, &viewer.token, false).await);
        assert!(!reader.comments(&ordinary).is_empty());
        let actual = key(
            &s.desk,
            viewer.id(),
            Some(vec![rule(&operation, "document", s.document, "read")]),
        )
        .await;
        assert_eq!(
            shown(reader.read(&s, &actual.secret, false).await),
            ordinary
        );
        let legacy = key(&s.desk, viewer.id(), None).await;
        assert_eq!(
            shown(reader.read(&s, &legacy.secret, false).await),
            ordinary
        );
        for rules in [
            Vec::new(),
            vec![rule(&operation, "document", other, "read")],
            vec![rule("DocumentGet.read", "document", s.document, "read")],
            vec![rule(&operation, "document", s.document, "manage")],
        ] {
            let wrong = key(&s.desk, viewer.id(), Some(rules)).await;
            denied(reader.read(&s, &wrong.secret, false).await);
        }
        let no_authority = key(
            &s.desk,
            outsider.id(),
            Some(vec![rule(&operation, "document", s.document, "read")]),
        )
        .await;
        denied(reader.read(&s, &outsider.token, false).await);
        denied(reader.read(&s, &no_authority.secret, false).await);
    }
}

#[tokio::test]
async fn discussion_pages_keep_resolved_filters_anchoring_order_and_cursor_rows_when_labels_are_ungranted()
 {
    let s = Scene::new().await;
    let external = s
        .desk
        .make(&s.desk.ada, "Foreign live page name")
        .await
        .base
        .id;
    s.desk.world.advance(chrono::Duration::seconds(1));
    let second = open(
        &s.desk,
        &s.desk.ada,
        s.document,
        s.block,
        vec![reference(
            InlineRefKind::Document,
            external,
            "Stored label remains",
        )],
    )
    .await;
    s.desk.world.advance(chrono::Duration::seconds(1));
    let third = open(
        &s.desk,
        &s.desk.ada,
        s.document,
        s.block,
        text("Third current discussion"),
    )
    .await;
    s.desk
        .world
        .app
        .call(DocumentDiscussionResolve, &[&s.document, &second])
        .token(&s.desk.ada.token)
        .body(&DocumentDiscussionResolveBody { resolved: true })
        .send()
        .await
        .ok();
    let operation = "DocumentDiscussionList.read";
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(operation, "document", s.document, "read")]),
    )
    .await;
    for resolved in [
        DiscussionResolvedFilter::Exclude,
        DiscussionResolvedFilter::Include,
        DiscussionResolvedFilter::Only,
    ] {
        let mut query = DocumentDiscussionListQuery {
            resolved: Some(resolved),
            limit: Some(1),
            ..Default::default()
        };
        let mut found = Vec::new();
        let mut cursors = std::collections::BTreeSet::new();
        loop {
            let ordinary =
                shown(discussion_page(&s.desk, &s.desk.ada.token, s.document, &query).await);
            let page = shown(discussion_page(&s.desk, &token.secret, s.document, &query).await);
            assert_eq!(page["next_cursor"], ordinary["next_cursor"]);
            assert_eq!(
                page["items"].as_array().unwrap().len(),
                ordinary["items"].as_array().unwrap().len()
            );
            for row in page["items"].as_array().unwrap() {
                assert!(row["anchored"].as_bool().unwrap());
                found.push(row["id"].clone());
            }
            let Some(cursor) = page["next_cursor"].as_str() else {
                break;
            };
            assert!(cursors.insert(cursor.to_owned()));
            query.cursor = Some(cursor.to_owned());
        }
        let expected = match resolved {
            DiscussionResolvedFilter::Exclude => vec![json!(third), json!(s.discussion)],
            DiscussionResolvedFilter::Include => {
                vec![json!(third), json!(second), json!(s.discussion)]
            }
            DiscussionResolvedFilter::Only => vec![json!(second)],
        };
        assert_eq!(found, expected);
    }
    let empty = s.desk.make(&s.desk.ada, "No discussions yet").await.base.id;
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(operation, "document", empty, "read")]),
    )
    .await;
    let page = shown(discussion_page(&s.desk, &token.secret, empty, &Default::default()).await);
    assert!(page["items"].as_array().unwrap().is_empty() && page["next_cursor"].is_null());
}

#[tokio::test]
async fn comment_windows_preserve_creation_order_public_people_reactions_and_wrote_across_older_pages()
 {
    let s = Scene::new().await;
    let viewer = s.desk.member("comment-writer", "viewer").await;
    s.desk.world.advance(chrono::Duration::seconds(1));
    let written = answer(
        &s.desk,
        &viewer,
        s.document,
        s.discussion,
        text("Viewer wrote on an older page"),
    )
    .await;
    s.desk.world.advance(chrono::Duration::seconds(1));
    let last = answer(
        &s.desk,
        &s.desk.ada,
        s.document,
        s.discussion,
        text("Latest comment"),
    )
    .await;
    s.desk
        .world
        .app
        .call(DocumentCommentReactionSet, &[&s.document, &written])
        .token(&s.desk.ada.token)
        .body(&DocumentCommentReactionSetBody {
            emoji: "👍".into(),
            on: true,
        })
        .send()
        .await
        .ok();
    let token = key(
        &s.desk,
        viewer.id(),
        Some(vec![rule(
            "DocumentCommentList.read",
            "document",
            s.document,
            "read",
        )]),
    )
    .await;
    let mut query = DocumentCommentListQuery {
        limit: Some(1),
        ..Default::default()
    };
    let mut found = Vec::new();
    let mut cursors = std::collections::BTreeSet::new();
    loop {
        let ordinary =
            shown(comment_page(&s.desk, &viewer.token, s.document, s.discussion, &query).await);
        let page =
            shown(comment_page(&s.desk, &token.secret, s.document, s.discussion, &query).await);
        assert_eq!(page, ordinary);
        assert_eq!(page["wrote"], true);
        found.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].clone()),
        );
        assert!(
            found.len() <= 3,
            "the three actual comments cannot create extra pages"
        );
        let Some(cursor) = page["next_cursor"].as_str() else {
            break;
        };
        assert!(cursors.insert(cursor.to_owned()));
        query.cursor = Some(cursor.to_owned());
    }
    assert_eq!(found[0], json!(last));
    assert_eq!(found[1], json!(written));
    assert_eq!(found.len(), 3);
    let page = shown(
        comment_page(
            &s.desk,
            &token.secret,
            s.document,
            s.discussion,
            &Default::default(),
        )
        .await,
    );
    let writer = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(written))
        .unwrap();
    assert_eq!(writer["author"]["id"], json!(viewer.id()));
    assert_eq!(writer["reactions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn comment_page_uses_the_actual_document_discussion_pair_and_keeps_cursor_refusals() {
    let s = Scene::new().await;
    let other = s
        .desk
        .make(&s.desk.ada, "Different discussion owner")
        .await
        .base
        .id;
    let other_block = s
        .desk
        .paragraphs(
            &s.desk.ada,
            other,
            &["Anchor words on a different document."],
        )
        .await[0]
        .id;
    let foreign = open(
        &s.desk,
        &s.desk.ada,
        other,
        other_block,
        text("A foreign discussion"),
    )
    .await;
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(
            "DocumentCommentList.read",
            "document",
            s.document,
            "read",
        )]),
    )
    .await;
    shown(
        comment_page(
            &s.desk,
            &token.secret,
            s.document,
            s.discussion,
            &Default::default(),
        )
        .await,
    );
    for discussion in [foreign, new_id()] {
        let ordinary = comment_page(
            &s.desk,
            &s.desk.ada.token,
            s.document,
            discussion,
            &Default::default(),
        )
        .await;
        assert_eq!(ordinary.problem().code, ErrorCode::NotFound);
        let refused = comment_page(
            &s.desk,
            &token.secret,
            s.document,
            discussion,
            &Default::default(),
        )
        .await;
        assert_eq!(refused.problem().code, ErrorCode::NotFound);
    }
    let query = DocumentCommentListQuery {
        cursor: Some("not-an-issued-cursor".into()),
        ..Default::default()
    };
    assert_eq!(
        comment_page(&s.desk, &s.desk.ada.token, s.document, s.discussion, &query)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        comment_page(&s.desk, &token.secret, s.document, s.discussion, &query)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn live_reference_labels_use_each_reader_operation_without_rewriting_stored_content_or_people()
 {
    let s = Scene::new().await;
    let external = s
        .desk
        .make(&s.desk.ada, "Current external document title")
        .await
        .base
        .id;
    let content = vec![
        reference(InlineRefKind::Document, external, "A stored copied label"),
        reference(InlineRefKind::Node, s.desk.topic, "A stored place label"),
    ];
    let id = answer(&s.desk, &s.desk.ada, s.document, s.discussion, content).await;
    let stored: Json = sqlx::query_scalar("SELECT content FROM document_comments WHERE id=$1")
        .bind(id)
        .fetch_one(&s.desk.world.db.pool)
        .await
        .unwrap();
    let mut full_keys = Vec::new();
    for reader in READERS {
        let operation = reader.selection(false);
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, false).await);
        let expected = reader
            .comments(&ordinary)
            .into_iter()
            .find(|row| row["id"] == json!(id))
            .unwrap();
        assert_reference(
            expected,
            external,
            "A stored copied label",
            Some("Current external document title"),
        );
        let narrow = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![
                rule(&operation, "document", s.document, "read"),
                rule("DocumentGet.read", "document", external, "read"),
                rule("DocumentGet.read", "topic", s.desk.topic, "read"),
            ]),
        )
        .await;
        let page = shown(reader.read(&s, &narrow.secret, false).await);
        let row = reader
            .comments(&page)
            .into_iter()
            .find(|row| row["id"] == json!(id))
            .unwrap();
        assert_reference(row, external, "A stored copied label", None);
        assert_reference(row, s.desk.topic, "A stored place label", None);
        assert_eq!(row["author"], expected["author"]);
        assert_eq!(row["created_at"], expected["created_at"]);
        let full = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![
                rule(&operation, "document", s.document, "read"),
                rule(&operation, "document", external, "read"),
                rule(&operation, "topic", s.desk.topic, "read"),
            ]),
        )
        .await;
        let page = shown(reader.read(&s, &full.secret, false).await);
        assert_eq!(
            reader
                .comments(&page)
                .into_iter()
                .find(|row| row["id"] == json!(id))
                .unwrap(),
            expected
        );
        full_keys.push((reader, full));
    }
    s.desk
        .world
        .app
        .call(orch_api_contract::documents::DocumentUpdate, &[&external])
        .token(&s.desk.ada.token)
        .body(&orch_api_contract::documents::DocumentUpdateBody {
            title: Some("Renamed external document title".into()),
            description: None,
        })
        .send()
        .await
        .ok();
    for (reader, full) in full_keys {
        let page = shown(reader.read(&s, &full.secret, false).await);
        let row = reader
            .comments(&page)
            .into_iter()
            .find(|row| row["id"] == json!(id))
            .unwrap();
        assert_reference(
            row,
            external,
            "A stored copied label",
            Some("Renamed external document title"),
        );
    }
    let after: Json = sqlx::query_scalar("SELECT content FROM document_comments WHERE id=$1")
        .bind(id)
        .fetch_one(&s.desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, stored,
        "reading current labels never rewrites the stored comment"
    );
}

#[tokio::test]
async fn comment_inspection_keeps_its_explicit_manage_selection_and_real_restore_authority() {
    let s = Scene::new().await;
    let viewer = s.desk.member("retained-comment-viewer", "viewer").await;
    s.desk
        .world
        .app
        .call(DocumentDelete, &[&s.document])
        .token(&s.desk.ada.token)
        .send()
        .await
        .ok();
    for reader in READERS {
        let read = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![rule(
                &reader.selection(false),
                "document",
                s.document,
                "read",
            )]),
        )
        .await;
        let manager = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![rule(
                &reader.selection(true),
                "document",
                s.document,
                "manage",
            )]),
        )
        .await;
        let no_authority = key(
            &s.desk,
            viewer.id(),
            Some(vec![rule(
                &reader.selection(true),
                "document",
                s.document,
                "manage",
            )]),
        )
        .await;
        assert_eq!(
            reader
                .read(&s, &s.desk.ada.token, false)
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, true).await);
        assert!(!reader.comments(&ordinary).is_empty());
        assert_eq!(
            shown(reader.read(&s, &manager.secret, true).await),
            ordinary
        );
        denied(reader.read(&s, &read.secret, true).await);
        denied(reader.read(&s, &no_authority.secret, true).await);
        denied(reader.read(&s, &viewer.token, true).await);
        assert!(matches!(
            reader.read(&s, &read.secret, false).await.problem().code,
            ErrorCode::NotFound | ErrorCode::AuthzDenied
        ));
        assert!(matches!(
            reader.read(&s, &manager.secret, false).await.problem().code,
            ErrorCode::NotFound | ErrorCode::AuthzDenied
        ));
    }
}

#[tokio::test]
async fn both_comment_readers_restore_current_key_scope_and_revocation_before_returning_stored_words()
 {
    let s = Scene::new().await;
    for reader in READERS {
        for revoke in [false, true] {
            let token = key(
                &s.desk,
                s.desk.ada.id(),
                Some(vec![rule(
                    &reader.selection(false),
                    "document",
                    s.document,
                    "read",
                )]),
            )
            .await;
            shown(reader.read(&s, &token.secret, false).await);
            if revoke {
                identity::revoke_api_token(
                    &mut *s.desk.world.conn().await,
                    token.id,
                    s.desk.ada.id(),
                    s.desk.world.now(),
                )
                .await
                .unwrap();
            } else {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(serde_json::to_value(KeyScope::default()).unwrap())
                    .execute(&s.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            let after = reader.read(&s, &token.secret, false).await;
            assert_eq!(
                after.status,
                if revoke {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&after.raw).contains("Stored first comment"));
        }
    }
    let viewer = s.desk.member("scoped-comment-viewer", "viewer").await;
    let mut readers = Vec::new();
    for reader in READERS {
        let token = key(
            &s.desk,
            viewer.id(),
            Some(vec![rule(
                &reader.selection(false),
                "document",
                s.document,
                "read",
            )]),
        )
        .await;
        shown(reader.read(&s, &token.secret, false).await);
        readers.push((reader, token));
    }
    s.desk
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"documents", &s.document],
        )
        .token(&s.desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .full_permission_page()
        .ok();
    for (reader, token) in readers {
        denied(reader.read(&s, &viewer.token, false).await);
        denied(reader.read(&s, &token.secret, false).await);
    }
}

#[tokio::test]
async fn an_attached_file_label_uses_its_actual_document_owner_for_each_comment_reader() {
    use orch_api_contract::documents::{DocumentFileCreate, DocumentFileCreateQuery};
    use orch_api_contract::threads::FILENAME_HEADER;
    let s = Scene::new().await;
    let owner = s
        .desk
        .make(&s.desk.ada, "Actual attachment owner")
        .await
        .base
        .id;
    let file = s
        .desk
        .world
        .app
        .call(DocumentFileCreate, &[&owner])
        .token(&s.desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "owned-comment-file.txt")
        .bytes(b"File bytes are not a comment's live label")
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    let comment = answer(
        &s.desk,
        &s.desk.ada,
        s.document,
        s.discussion,
        vec![reference(InlineRefKind::File, file.id, "Stored file link")],
    )
    .await;
    for reader in READERS {
        let operation = reader.selection(false);
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, false).await);
        let expected = reader
            .comments(&ordinary)
            .into_iter()
            .find(|row| row["id"] == json!(comment))
            .unwrap();
        assert_reference(
            expected,
            file.id,
            "Stored file link",
            Some("owned-comment-file.txt"),
        );
        for extra in [
            Vec::new(),
            vec![rule(&operation, "topic", s.desk.topic, "read")],
            vec![rule(&operation, "document", file.id, "read")],
            vec![rule("DocumentGet.read", "document", owner, "read")],
        ] {
            let mut rules = vec![rule(&operation, "document", s.document, "read")];
            rules.extend(extra);
            let token = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
            let page = shown(reader.read(&s, &token.secret, false).await);
            let row = reader
                .comments(&page)
                .into_iter()
                .find(|row| row["id"] == json!(comment))
                .unwrap();
            assert_reference(row, file.id, "Stored file link", None);
        }
        let token = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![
                rule(&operation, "document", s.document, "read"),
                rule(&operation, "document", owner, "read"),
            ]),
        )
        .await;
        let page = shown(reader.read(&s, &token.secret, false).await);
        assert_eq!(
            reader
                .comments(&page)
                .into_iter()
                .find(|row| row["id"] == json!(comment))
                .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn carried_file_and_node_chat_names_keep_their_distinct_current_sources_in_comment_pages() {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatBlobGet, ChatBlobGetQuery, ChatMessageCreate,
        ChatMessageCreateBody, ChatPart,
    };
    use orch_api_contract::threads::FILENAME_HEADER;
    let s = Scene::new().await;
    let topic = s.desk.another_topic("External comment node chat").await;
    let orch_api_contract::nodes::Node::Topic(node) =
        places::read(&s.desk.world, &s.desk.ada, topic).await
    else {
        panic!("topic");
    };
    let chat = node.base.chat_id;
    let upload = s
        .desk
        .world
        .app
        .call(ChatAttachmentCreate, &[&chat])
        .token(&s.desk.ada.token)
        .header(FILENAME_HEADER, "carried-comment-file.txt")
        .bytes(b"Stored carried file")
        .send()
        .await
        .ok();
    let message = s
        .desk
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&s.desk.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Attachment {
                blob: upload.blob,
                name: upload.name.clone(),
                size: upload.size,
                media_type: upload.media_type.clone(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .message;
    let preview = s
        .desk
        .world
        .app
        .call(ChatBlobGet, &[&chat, &upload.blob])
        .token(&s.desk.ada.token)
        .query(&ChatBlobGetQuery {
            preview: Some(true),
            download: None,
        })
        .send_bytes()
        .await;
    assert_eq!(preview.status, StatusCode::OK);
    let file: Id = preview.headers["x-orch-file-id"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let witness: (Id, Id, i32) =
        sqlx::query_as("SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1")
            .bind(file)
            .fetch_one(&s.desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(witness, (chat, message.id, 1));
    let comment = answer(
        &s.desk,
        &s.desk.ada,
        s.document,
        s.discussion,
        vec![
            reference(InlineRefKind::File, file, "Stored carried link"),
            reference(InlineRefKind::Chat, chat, "Stored chat link"),
        ],
    )
    .await;
    for reader in READERS {
        let operation = reader.selection(false);
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, false).await);
        let expected = reader
            .comments(&ordinary)
            .into_iter()
            .find(|row| row["id"] == json!(comment))
            .unwrap();
        assert_reference(
            expected,
            file,
            "Stored carried link",
            Some("carried-comment-file.txt"),
        );
        assert_reference(
            expected,
            chat,
            "Stored chat link",
            Some("External comment node chat"),
        );
        let root = rule(&operation, "document", s.document, "read");
        let ambient = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![root.clone(), rule(&operation, "topic", topic, "read")]),
        )
        .await;
        let page = shown(reader.read(&s, &ambient.secret, false).await);
        let row = reader
            .comments(&page)
            .into_iter()
            .find(|row| row["id"] == json!(comment))
            .unwrap();
        assert_reference(row, file, "Stored carried link", None);
        assert_reference(row, chat, "Stored chat link", None);
        let source = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![root.clone(), rule(&operation, "chat", chat, "read")]),
        )
        .await;
        let page = shown(reader.read(&s, &source.secret, false).await);
        let row = reader
            .comments(&page)
            .into_iter()
            .find(|row| row["id"] == json!(comment))
            .unwrap();
        assert_reference(
            row,
            file,
            "Stored carried link",
            Some("carried-comment-file.txt"),
        );
        assert_reference(row, chat, "Stored chat link", None);
        let full = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![
                root,
                rule(&operation, "chat", chat, "read"),
                rule(&operation, "topic", topic, "read"),
            ]),
        )
        .await;
        let page = shown(reader.read(&s, &full.secret, false).await);
        assert_eq!(
            reader
                .comments(&page)
                .into_iter()
                .find(|row| row["id"] == json!(comment))
                .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn comment_readers_keep_retained_parent_identity_without_restoring_or_read_upgrading_it() {
    let s = Scene::new().await;
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(
            READERS
                .into_iter()
                .map(|reader| rule(&reader.selection(true), "document", s.document, "manage"))
                .collect(),
        ),
    )
    .await;
    s.desk
        .world
        .app
        .call(orch_api_contract::ownership::NodeDelete, &[&s.desk.project])
        .token(&s.desk.ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let state = || async {
        sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT (SELECT deleted_at FROM nodes WHERE id=$1),deleted_at FROM documents WHERE id=$2")
            .bind(s.desk.project).bind(s.document).fetch_one(&s.desk.world.db.pool).await.unwrap()
    };
    let before = state().await;
    assert!(before.0.is_some());
    for reader in READERS {
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, true).await);
        assert!(!reader.comments(&ordinary).is_empty());
        assert_eq!(shown(reader.read(&s, &token.secret, true).await), ordinary);
        assert!(matches!(
            reader.read(&s, &token.secret, false).await.problem().code,
            ErrorCode::AuthzDenied | ErrorCode::NotFound
        ));
    }
    assert_eq!(state().await, before);
}
