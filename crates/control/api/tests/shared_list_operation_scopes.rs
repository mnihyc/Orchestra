//! The native Shared union retains its entrances, order and factual response sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, paragraph, put};
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::PageQuery;
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentTreeItem,
};
use orch_api_contract::nodes::{NodeGet, SharedList};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{identity, threads};
use orch_types::{Id, Json, new_id};
use places::{Who, join};
use serde_json::json;

struct Scene {
    desk: Desk,
    reader: Who,
    topic: Id,
    folder: Id,
    document: Id,
}
impl Scene {
    async fn new() -> Self {
        let desk = Desk::new().await;
        let reader = desk.stranger("shared-reader").await;
        let document = desk
            .make(&desk.ada, "Existing shared document content")
            .await
            .base
            .id;
        orch_api_core::routes::resources::sealing::install_master_key(
            orch_db::secrets::MasterKey::new([42; 32]),
        );
        let folder = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&DocumentContainerCreateBody {
                parent_container_id: None,
                name: "Existing shared folder content".into(),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .ok()
            .entries
            .into_iter()
            .find_map(|entry| match entry.item {
                DocumentTreeItem::Container(id) => Some(id),
                _ => None,
            })
            .expect("native folder constructor returns a container");
        let topic = desk.another_topic("Existing shared node content").await;
        for (object, id) in [
            ("nodes", topic),
            ("document-containers", folder),
            ("documents", document),
        ] {
            give(&desk, &reader, object, id).await;
        }
        // Native creation-time order is deterministic and deliberately puts the
        // selected document behind unselected candidates from both domains.
        for (table, id, seconds) in [
            ("nodes", topic, 1),
            ("document_containers", folder, 2),
            ("documents", document, 3),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET created_at=$2 WHERE id=$1"
            )))
            .bind(id)
            .bind(desk.world.now() - chrono::Duration::seconds(seconds))
            .execute(&desk.world.db.pool)
            .await
            .unwrap();
        }
        Self {
            desk,
            reader,
            topic,
            folder,
            document,
        }
    }
    fn rows(&self) -> [(&'static str, Id); 3] {
        [
            ("topic", self.topic),
            ("document_container", self.folder),
            ("document", self.document),
        ]
    }
    async fn key(&self, rows: &[(&str, Id)], extra: Vec<Json>) -> String {
        let mut rules: Vec<_> = rows
            .iter()
            .map(|(kind, id)| rule("SharedList", kind, *id))
            .collect();
        rules.extend(extra);
        make_key(&self.desk, self.reader.id(), rules).await
    }
    async fn fetch(&self, token: &str, limit: u32, cursor: Option<String>) -> RawReply {
        self.desk
            .world
            .app
            .call(SharedList, &[])
            .token(token)
            .query(&PageQuery {
                limit: Some(limit),
                cursor,
            })
            .send_bytes()
            .await
    }
}
async fn give(desk: &Desk, who: &Who, object: &str, id: Id) {
    desk.world.app.call(PermissionPageUpdate,&[&object,&id]).token(&desk.ada.token)
        .raw_body(&json!({"name":[{"holders":[{"kind":"user","id":who.id()}],"choice":"full_view","export":false}]}))
        .send().await.ok();
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn make_key(desk: &Desk, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Shared native union control",
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
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn item_id(row: &Json) -> Id {
    let id = match row["kind"].as_str() {
        Some("document") => &row["document"]["id"],
        Some("document_container") => &row["folder"]["id"],
        _ => &row["id"],
    };
    serde_json::from_value(id.clone()).unwrap()
}
fn ids(page: &Json) -> Vec<Id> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(item_id)
        .collect()
}
fn item(page: &Json, id: Id) -> &Json {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| item_id(row) == id)
        .expect("native selected item exists")
}

#[tokio::test]
async fn shared_native_family_table_and_filtered_merged_pages_use_actual_business_targets() {
    let s = Scene::new().await;
    let ordinary = shown(s.fetch(&s.reader.token, 20, None).await);
    assert_eq!(ids(&ordinary), s.rows().map(|(_, id)| id));
    let mut failures = Vec::new();
    for row in s.rows() {
        let token = s.key(&[row], vec![]).await;
        let reply = s.fetch(&token, 1, None).await;
        if reply.status != StatusCode::OK {
            failures.push(format!(
                "{} expected200 got{} {}",
                row.0,
                reply.status,
                String::from_utf8_lossy(&reply.raw)
            ));
            continue;
        }
        let page = shown(reply);
        assert_eq!(ids(&page), vec![row.1]);
        assert_eq!(
            item(&page, row.1),
            item(&ordinary, row.1),
            "sufficient native authority retains the real DTO"
        );
        assert_eq!(page["partial"], false);
        assert!(page["next_cursor"].is_null());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let token = s.key(&s.rows(), vec![]).await;
    let first = shown(s.fetch(&token, 2, None).await);
    assert_eq!(ids(&first), vec![s.topic, s.folder]);
    assert_eq!(first["partial"], true);
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let last = shown(s.fetch(&token, 2, Some(cursor.clone())).await);
    assert_eq!(ids(&last), vec![s.document]);
    assert_eq!(last["partial"], false);
    let only_folder = s.key(&[("document_container", s.folder)], vec![]).await;
    let exhausted = shown(s.fetch(&only_folder, 2, Some(cursor)).await);
    assert!(ids(&exhausted).is_empty());
    assert_eq!(exhausted["partial"], false);
}

#[tokio::test]
async fn key_parent_read_does_not_change_native_shared_entrance_suppression() {
    let s = Scene::new().await;
    let before = shown(s.fetch(&s.reader.token, 20, None).await);
    assert!(ids(&before).contains(&s.topic) && ids(&before).contains(&s.document));
    join(
        &s.desk.world,
        s.desk.org,
        s.desk.org,
        &s.reader,
        "viewer",
        &s.desk.ada,
    )
    .await;
    s.desk
        .world
        .app
        .call(NodeGet, &[&s.desk.project])
        .token(&s.reader.token)
        .send()
        .await
        .ok();
    let ordinary = shown(s.fetch(&s.reader.token, 20, None).await);
    assert!(!ids(&ordinary).contains(&s.topic) && !ids(&ordinary).contains(&s.document));
    assert!(
        ids(&ordinary).contains(&s.folder),
        "native folder entrances retain their own semantics"
    );
    for extra in [
        vec![],
        vec![rule("NodeGet.read", "project", s.desk.project)],
    ] {
        let token = s
            .key(&[("topic", s.topic), ("document", s.document)], extra)
            .await;
        let bounded = shown(s.fetch(&token, 20, None).await);
        assert!(ids(&bounded).is_empty());
        assert!(!bounded.to_string().contains("Existing shared node content"));
        assert!(
            !bounded
                .to_string()
                .contains("Existing shared document content")
        );
    }
}

#[tokio::test]
async fn shared_document_live_author_name_is_a_same_operation_source_and_stored_label_remains() {
    use orch_db::queries::documents::batch::{self, Limits, Outcome, Rules};
    use orch_documents::versions::Writer;
    let s = Scene::new().await;
    let thread = new_id();
    assert!(
        threads::insert_thread(
            &mut *s.desk.world.conn().await,
            thread,
            s.desk.topic,
            "Existing live shared writer thread",
            s.desk.ada.id(),
            s.desk.world.now()
        )
        .await
        .unwrap()
    );
    give(&s.desk, &s.reader, "nodes", thread).await;
    // The native serializer writes actual stored attribution. These are the same
    // shipped batch/history defaults used by ordinary document version controls.
    let mut tx = s.desk.world.db.pool.begin().await.unwrap();
    let outcome = batch::apply(
        &mut tx,
        s.document,
        &Writer::Agent {
            thread,
            agent: "Recorded agent label retained".into(),
        },
        None,
        &[put(paragraph("Native stored shared document content"))],
        Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        s.desk.world.now(),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, Outcome::Applied(_)));
    tx.commit().await.unwrap();
    let ordinary = shown(s.fetch(&s.reader.token, 20, None).await);
    let author = &item(&ordinary, s.document)["document"]["updated_by"];
    assert_eq!(author["agent"], "Recorded agent label retained");
    assert_eq!(
        author["thread"]["name"],
        "Existing live shared writer thread"
    );
    let token = s
        .key(
            &[("document", s.document)],
            vec![rule("ThreadSyncGet.read", "thread", thread)],
        )
        .await;
    let limited = shown(s.fetch(&token, 20, None).await);
    let author = &item(&limited, s.document)["document"]["updated_by"];
    assert_eq!(author["agent"], "Recorded agent label retained");
    assert_eq!(author["thread"]["id"], json!(thread));
    assert!(author["thread"]["name"].is_null());
    assert_eq!(author["thread"]["redacted"], true);
    assert!(
        !limited
            .to_string()
            .contains("Existing live shared writer thread")
    );
    let token = s
        .key(&[("document", s.document), ("thread", thread)], vec![])
        .await;
    let full = shown(s.fetch(&token, 20, None).await);
    assert_eq!(
        item(&full, s.document)["document"]["updated_by"]["thread"]["name"],
        "Existing live shared writer thread"
    );
}

#[tokio::test]
async fn shared_native_user_bound_cursor_validation_is_preserved_for_keys() {
    let s = Scene::new().await;
    let first = shown(s.fetch(&s.reader.token, 1, None).await);
    let cursor = first["next_cursor"]
        .as_str()
        .expect("ordinary mixed list has another page")
        .to_owned();
    let other = s.desk.stranger("other-shared-reader").await;
    for (object, id) in [
        ("nodes", s.topic),
        ("document-containers", s.folder),
        ("documents", s.document),
    ] {
        give(&s.desk, &other, object, id).await;
    }
    let ordinary = s.fetch(&other.token, 1, Some(cursor.clone())).await;
    assert_eq!(ordinary.status, StatusCode::BAD_REQUEST);
    let token = make_key(
        &s.desk,
        other.id(),
        s.rows()
            .into_iter()
            .map(|(kind, id)| rule("SharedList", kind, id))
            .collect(),
    )
    .await;
    let limited = s.fetch(&token, 1, Some(cursor)).await;
    assert_eq!(limited.status, ordinary.status);
    assert_eq!(limited.problem().code, ordinary.problem().code);
}
