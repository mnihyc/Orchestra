//! Document placement and visibility changes delivered to two readers' tree feeds, and sidebar state delivered to another device.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_contract::access::{PageChange, PageChangeWhole, PermissionPageUpdate};
use orch_api_contract::documents::{
    DocumentDelete, DocumentMove, DocumentMoveBody, DocumentRestore, NodeDocumentCreate,
    NodeDocumentCreateBody, NodeDocumentList, NodeDocumentListQuery,
};
use orch_api_contract::nodes::{OrgTreeGet, OrgTreeGetQuery};
use orch_api_contract::personal::{MeStateGet, MeStateSet};
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_db::queries::{access, hierarchy};
use orch_testkit::clock;
use orch_types::Id;
use realtime_support::{Client, Rig};
use serde_json::{Value, json};
use std::collections::HashSet;

async fn next(client: &mut Client, op: &str) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == op {
            return frame;
        }
    }
}

/// A reader of the tree, as a client keeps it: its socket, and what it reads the tree with when the feed
/// asks it to load again.
struct Reader<'a> {
    client: Client,
    app: &'a TestApp,
    token: &'a str,
    root: Id,
}

/// Waits until the reader's tree says how many documents each node has: from the rows its feed sends,
/// or, when the feed asks it to load again (who may read what changed), from the tree read anew, as a
/// client does, then follows again from the sequence it loaded.
async fn counts(reader: &mut Reader<'_>, expected: &[(Id, i64)]) {
    let mut seen = HashSet::new();
    while seen.len() < expected.len() {
        let frame = reader.client.frame().await;
        match frame["op"].as_str() {
            Some("sync") => {
                for row in frame["data"]["nodes"].as_array().unwrap() {
                    for (node, count) in expected {
                        if row["id"] == node.to_string() {
                            assert_eq!(row["docs"], *count, "{frame}");
                            seen.insert(*node);
                        }
                    }
                }
            }
            Some("resync") if frame["reload"] == true => {
                let tree = reader
                    .app
                    .call(OrgTreeGet, &[&reader.root])
                    .token(reader.token)
                    .query(&OrgTreeGetQuery::default())
                    .send()
                    .await
                    .ok();
                for node in tree.readable() {
                    for (id, count) in expected {
                        if node.id == *id {
                            assert_eq!(node.docs, *count, "{node:?}");
                            seen.insert(*id);
                        }
                    }
                }
                assert_eq!(
                    seen.len(),
                    expected.len(),
                    "the tree read anew has every node"
                );
                reader.client.send(json!({"op":"unsub", "id":1})).await;
                assert_eq!(
                    reader.client.expect("closed").await["reason"],
                    "unsubscribed"
                );
                reader
                    .client
                    .sub(1, &format!("org:{}", reader.root), Some(tree.seq))
                    .await;
                reader.client.expect("subscribed").await;
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_created_deleted_restored_moved_and_made_private_reaches_both_readers_trees() {
    let rig = Rig::new().await;
    let (bea, bea_token) = rig.person("bea").await;
    access::testing::give_role(
        &mut rig.pool.acquire().await.unwrap(),
        rig.root,
        "user",
        bea,
        "viewer",
        rig.ada,
        None,
    )
    .await
    .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(rig.server.app.clone(), clock::manual(), registry);
    let before = hierarchy::tree_seq(&mut rig.pool.acquire().await.unwrap(), rig.root)
        .await
        .unwrap();
    let mut ada = Reader {
        client: Client::signed_in(rig.server.addr, &rig.token).await,
        app: &app,
        token: &rig.token,
        root: rig.root,
    };
    let mut bea = Reader {
        client: Client::signed_in(rig.server.addr, &bea_token).await,
        app: &app,
        token: &bea_token,
        root: rig.root,
    };
    for reader in [&mut ada, &mut bea] {
        reader
            .client
            .sub(1, &format!("org:{}", rig.root), Some(before))
            .await;
        reader.client.expect("subscribed").await;
    }
    let document = app
        .call(NodeDocumentCreate, &[&rig.topic])
        .token(&rig.token)
        .body(&NodeDocumentCreateBody {
            title: "Notes".into(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id;
    for reader in [&mut ada, &mut bea] {
        counts(reader, &[(rig.topic, 1)]).await;
    }
    app.call(DocumentDelete, &[&document])
        .token(&rig.token)
        .send()
        .await
        .ok();
    for reader in [&mut ada, &mut bea] {
        counts(reader, &[(rig.topic, 0)]).await;
    }
    app.call(DocumentRestore, &[&document])
        .token(&rig.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .ok();
    for reader in [&mut ada, &mut bea] {
        counts(reader, &[(rig.topic, 1)]).await;
    }
    app.call(DocumentMove, &[&document])
        .token(&rig.token)
        .body(&DocumentMoveBody {
            node_id: rig.project,
        })
        .send()
        .await
        .ok();
    for reader in [&mut ada, &mut bea] {
        counts(reader, &[(rig.topic, 0), (rig.project, 1)]).await;
    }
    app.call(PermissionPageUpdate, &[&"documents", &document])
        .token(&rig.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    for reader in [&mut ada, &mut bea] {
        counts(reader, &[(rig.project, 0)]).await;
    }
    assert_eq!(
        app.call(NodeDocumentList, &[&rig.project])
            .token(&rig.token)
            .query(&NodeDocumentListQuery::default())
            .send()
            .await
            .ok()
            .items[0]
            .as_visible()
            .unwrap()
            .id,
        document
    );
    assert!(
        app.call(NodeDocumentList, &[&rig.project])
            .token(&bea_token)
            .query(&NodeDocumentListQuery::default())
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sidebar_change_on_one_device_tells_another_device_the_saved_value() {
    let rig = Rig::new().await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(rig.server.app.clone(), clock::manual(), registry);
    let mut another = Client::signed_in(rig.server.addr, &rig.token).await;
    another.sub(1, "user", None).await;
    another.expect("subscribed").await;
    let value = json!({"folded": ["recent"], "documents": false});
    app.call(MeStateSet, &[&"sidebar"])
        .token(&rig.token)
        .body(&value)
        .send()
        .await
        .ok();
    // The notice carries the value as it was saved, read after the commit, so that the other
    // device has nothing to ask again.
    let event = next(&mut another, "event").await;
    assert_eq!(
        event["data"],
        json!({"what": "state", "key": "sidebar", "value": value})
    );
    assert_eq!(
        app.call(MeStateGet, &[])
            .token(&rig.token)
            .send()
            .await
            .ok()["sidebar"],
        value
    );
    rig.server.stop().await;
}
