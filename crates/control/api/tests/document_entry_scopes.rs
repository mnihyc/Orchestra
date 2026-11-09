//! Typed native entry metadata and its own-operation protected context sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::document_tree::*;
use orch_api_contract::documents::DocumentDelete;
use orch_api_contract::nodes::DeletedInspectionQuery;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

#[derive(Clone, Copy)]
enum Reader {
    Container,
    Entry,
}

impl Reader {
    fn operation(self, inspect: bool) -> String {
        format!(
            "{}.{}",
            match self {
                Self::Container => "DocumentContainerGet",
                Self::Entry => "DocumentTreeEntryGet",
            },
            if inspect { "inspect_deleted" } else { "read" }
        )
    }

    async fn read(
        self,
        desk: &Desk,
        token: &str,
        id: Id,
        kind: DocumentTreeKind,
        inspect: bool,
    ) -> RawReply {
        match self {
            Self::Container => {
                desk.world
                    .app
                    .call(DocumentContainerGet, &[&id])
                    .token(token)
                    .query(&DeletedInspectionQuery {
                        inspect_deleted: inspect.then_some(true),
                    })
                    .send_bytes()
                    .await
            }
            Self::Entry => {
                desk.world
                    .app
                    .call(DocumentTreeEntryGet, &[&id])
                    .token(token)
                    .query(&DocumentTreeEntryGetQuery {
                        kind,
                        inspect_deleted: inspect.then_some(true),
                    })
                    .send_bytes()
                    .await
            }
        }
    }
}

fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Typed entry scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

async fn folder(desk: &Desk, parent: Option<Id>, name: &str) -> Id {
    let receipt = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: parent,
            name: name.into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    match receipt.entries[0].item {
        DocumentTreeItem::Container(id) => id,
        _ => panic!("the native folder creator returned the wrong kind"),
    }
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

#[tokio::test]
async fn both_entry_rows_bind_their_native_folder_and_bound_names_under_their_own_operation() {
    let desk = Desk::new().await;
    let outer = folder(&desk, None, "Protected outer folder").await;
    let inner = folder(&desk, Some(outer), "Selected inner folder").await;
    for reader in [Reader::Container, Reader::Entry] {
        let ordinary = shown(
            reader
                .read(
                    &desk,
                    &desk.ada.token,
                    inner,
                    DocumentTreeKind::Container,
                    false,
                )
                .await,
        );
        assert!(ordinary.to_string().contains("Protected outer folder"));
        let operation = reader.operation(false);
        let token = key(
            &desk,
            desk.ada.id(),
            vec![rule(&operation, "document_container", inner, "read")],
        )
        .await;
        let limited = shown(
            reader
                .read(&desk, &token, inner, DocumentTreeKind::Container, false)
                .await,
        );
        assert_eq!(
            limited["entries"][0]["entry"]["title"],
            "Selected inner folder"
        );
        assert_eq!(
            limited["entries"][0]["entry"]["item"],
            ordinary["entries"][0]["entry"]["item"]
        );
        assert!(!limited.to_string().contains("Protected outer folder"));
        assert!(
            limited["place"]["path"]
                .as_array()
                .unwrap()
                .iter()
                .all(|part| part["name"].is_null())
        );
        let unrelated = key(
            &desk,
            desk.ada.id(),
            vec![
                rule(&operation, "document_container", inner, "read"),
                rule("DocumentGet.read", "document_container", outer, "read"),
            ],
        )
        .await;
        assert!(
            !shown(
                reader
                    .read(&desk, &unrelated, inner, DocumentTreeKind::Container, false)
                    .await
            )
            .to_string()
            .contains("Protected outer folder")
        );
        let full = key(
            &desk,
            desk.ada.id(),
            vec![
                rule(&operation, "document_container", inner, "read"),
                rule(&operation, "document_container", outer, "read"),
                rule(&operation, "org", desk.org, "read"),
                rule(&operation, "project", desk.project, "read"),
                rule(&operation, "topic", desk.topic, "read"),
            ],
        )
        .await;
        let full = shown(
            reader
                .read(&desk, &full, inner, DocumentTreeKind::Container, false)
                .await,
        );
        assert!(full.to_string().contains("Protected outer folder"));
        assert_eq!(full["place"], ordinary["place"]);
    }
}

#[tokio::test]
async fn entry_document_reference_keeps_native_content_and_does_not_loan_node_names() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Selected document metadata")
        .await
        .base
        .id;
    let operation = Reader::Entry.operation(false);
    let token = key(
        &desk,
        desk.ada.id(),
        vec![rule(&operation, "document", document, "read")],
    )
    .await;
    let page = shown(
        Reader::Entry
            .read(&desk, &token, document, DocumentTreeKind::Document, false)
            .await,
    );
    assert_eq!(
        page["entries"][0]["entry"]["title"],
        "Selected document metadata"
    );
    assert_eq!(
        page["entries"][0]["document"]["title"],
        "Selected document metadata"
    );
    assert!(page["entries"][0]["document"]["node"]["name"].is_null());
    assert_eq!(page["entries"][0]["document"]["owner"]["username"], "ada");
    assert_eq!(
        Reader::Container
            .read(&desk, &token, document, DocumentTreeKind::Container, false)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn entry_scope_does_not_replace_typed_kind_or_current_ordinary_view() {
    let desk = Desk::new().await;
    let folder = folder(&desk, None, "Private selected folder").await;
    let stranger = desk.stranger("entry_scope_outsider").await;
    for reader in [Reader::Container, Reader::Entry] {
        let operation = reader.operation(false);
        for (user, rules, expected) in [
            (
                desk.ada.id(),
                vec![rule(&operation, "document", folder, "read")],
                StatusCode::FORBIDDEN,
            ),
            (
                desk.ada.id(),
                vec![rule(&operation, "document_container", folder, "write")],
                StatusCode::FORBIDDEN,
            ),
            (
                stranger.id(),
                vec![rule(&operation, "document_container", folder, "read")],
                StatusCode::NOT_FOUND,
            ),
        ] {
            let token = key(&desk, user, rules).await;
            let reply = reader
                .read(&desk, &token, folder, DocumentTreeKind::Container, false)
                .await;
            assert_eq!(reply.status, expected);
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Private selected folder"));
        }
    }
    let token = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "DocumentTreeEntryGet.read",
            "document_container",
            folder,
            "read",
        )],
    )
    .await;
    assert_eq!(
        Reader::Entry
            .read(&desk, &token, folder, DocumentTreeKind::Document, false)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn retained_typed_entry_requires_its_exact_manage_variant_and_native_restore_authority() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Retained typed entry").await.base.id;
    let viewer = desk.member("entry_retained_viewer", "viewer").await;
    desk.world
        .app
        .call(DocumentDelete, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let manager = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "DocumentTreeEntryGet.inspect_deleted",
            "document",
            document,
            "manage",
        )],
    )
    .await;
    let page = shown(
        Reader::Entry
            .read(&desk, &manager, document, DocumentTreeKind::Document, true)
            .await,
    );
    assert_eq!(page["entries"][0]["entry"]["title"], "Retained typed entry");
    assert!(page["entries"][0]["inspection"].is_object());
    for (user, operation, action, expected) in [
        (
            desk.ada.id(),
            "DocumentTreeEntryGet.read",
            "read",
            StatusCode::FORBIDDEN,
        ),
        (
            viewer.id(),
            "DocumentTreeEntryGet.inspect_deleted",
            "manage",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let token = key(
            &desk,
            user,
            vec![rule(operation, "document", document, action)],
        )
        .await;
        assert_eq!(
            Reader::Entry
                .read(&desk, &token, document, DocumentTreeKind::Document, true)
                .await
                .status,
            expected
        );
    }
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert!(
        deleted,
        "metadata inspection never restores the native entry"
    );
}
