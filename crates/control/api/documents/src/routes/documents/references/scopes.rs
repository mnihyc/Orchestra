//! Actual HTTP card disclosure uses its own operation and real typed owner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use super::*;

use crate::rig::Scene;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::documents::DocumentFileCreate;

use orch_api_contract::documents::DocumentFileCreateQuery;

use orch_api_contract::documents::NodeDocumentCreate;

use orch_api_contract::documents::NodeDocumentCreateBody;

use orch_db::queries::identity;

use orch_db::queries::identity::NewApiToken;

use serde_json::json;
async fn key(scene: &Scene, user: Id, rules: Vec<serde_json::Value>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let token = crate::middleware::authentication::new_token(
        crate::middleware::authentication::API_TOKEN_PREFIX,
    );
    identity::create_api_token_scoped(
        &mut scene.rig.db.acquire().await.unwrap(),
        &NewApiToken {
            id: orch_types::new_id(),
            user_id: user,
            name: "Reference card",
            token_hash: &crate::middleware::authentication::token_hash(&token),
            scopes: &[],
            node_id: None,
            at: scene.rig.world.now(),
            expires_at: Some(scene.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}

fn rule(op: &str, kind: &str, id: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":op,"actions":actions}]})
}

async fn card(
    scene: &Scene,
    token: &str,
    query: &ReferencePreviewQuery,
) -> crate::testing::RawReply {
    let request = scene
        .rig
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(token)
        .query(query);
    request.send_bytes().await
}

#[cfg(any(test, feature = "test-support"))]
async fn reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names() {
    let scene = Scene::new().await;
    let request = scene
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Private card source".into(),
            blocks: None,
            markdown: None,
            access: None,
        });
    let doc = request.send().await.ok().into_full().unwrap().base.id;
    let request = scene
        .rig
        .world
        .app
        .call(DocumentFileCreate, &[&doc])
        .token(&scene.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(orch_api_contract::threads::FILENAME_HEADER, "actual.txt")
        .bytes(b"native card bytes");
    let file = request.send().await.ok().into_full().unwrap().id;
    let query = whole(InlineRefKind::File, file);
    for bad in [
        rule("DocumentGet.read", "document", doc, &["read"]),
        rule("ReferencePreviewGet", "document", doc, &["write"]),
        rule("ReferencePreviewGet", "document_container", doc, &["read"]),
        rule("ReferencePreviewGet", "document", file, &["read"]),
    ] {
        let token = key(&scene, scene.ada.id(), vec![bad]).await;
        assert_eq!(
            card(&scene, &token, &query).await.status,
            http::StatusCode::FORBIDDEN
        );
    }
    let token = key(
        &scene,
        scene.ada.id(),
        vec![rule("ReferencePreviewGet", "document", doc, &["read"])],
    )
    .await;
    let reply = card(&scene, &token, &query).await;
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let shown: ReferencePreview = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(shown.file.unwrap().owner_id, Some(doc));
    assert_eq!(shown.title, "actual.txt");
    assert!(shown.place.unwrap().name.is_none());
    let token = key(
        &scene,
        scene.ada.id(),
        vec![
            rule("ReferencePreviewGet", "document", doc, &["read"]),
            rule(
                "ReferencePreviewGet",
                "topic",
                scene.tree.topic.base.id,
                &["read"],
            ),
        ],
    )
    .await;
    let shown: ReferencePreview =
        serde_json::from_slice(&card(&scene, &token, &query).await.raw).unwrap();
    assert!(shown.place.unwrap().name.is_some());
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn reference_cards_keep_each_actual_native_node_kind() {
    let scene = Scene::new().await;
    for (kind, id) in [
        ("org", scene.tree.org.base.id),
        ("project", scene.tree.project.base.id),
        ("topic", scene.tree.topic.base.id),
    ] {
        let token = key(
            &scene,
            scene.ada.id(),
            vec![rule("ReferencePreviewGet", kind, id, &["read"])],
        )
        .await;
        let reply = card(&scene, &token, &whole(InlineRefKind::Node, id)).await;
        assert_eq!(
            reply.status,
            http::StatusCode::OK,
            "{kind}: {}",
            String::from_utf8_lossy(&reply.raw)
        );
        let shown: ReferencePreview = serde_json::from_slice(&reply.raw).unwrap();
        assert_eq!(shown.id, id);
        assert!(!shown.title.is_empty());
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_reference_cards_keep_each_actual_native_node_kind()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::reference_cards_keep_each_actual_native_node_kind())
}
