//! Explicit document gifts narrow independently, and first-reading material keeps stable identities and current paths without hidden input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_api_contract::{
    share::{
        ShareAgentUpdateBody, ShareDocumentsLevel, ShareParts, ShareTo, ShareToThread, ShareWhat,
        ShareWhatDocument, ShareWhatNode, ShareWithAgents,
    },
    threads::ShareMode,
};
use orch_authz::Caller;
use orch_db::audit::Origin;
use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::json;
use shared::{Places, a_world_whose_agent_takes_tools, call, entry, ok, share, thread_in_topic};

async fn topic(world: &common::World, thread: Id) -> Id {
    sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

async fn cut_document_rows(world: &common::World, target: orch_authz::pages::Target) {
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(world.ada),
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let change = serde_json::from_value(json!({"rows": [
        {"permission":"doc.view", "inherit":false},
        {"permission":"doc.edit", "inherit":false},
        {"permission":"doc.manage", "inherit":false}
    ]}))
    .unwrap();
    orch_authz::pages::change(
        &mut conn,
        &principal,
        target,
        &change,
        orch_authz::Policy::default(),
        &Origin::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn write_then_read_then_revoked_gifts_preserve_private_document_cuts_and_independent_creation_entries()
 {
    let world = a_world_whose_agent_takes_tools().await;
    // The server always has a writer of files into workspaces; without one a download says so before it asks
    // whether the material may be taken away.
    world
        .engine()
        .attach_parcels(std::sync::Arc::new(shared::MemoryParcels::default()));
    let origin = world.thread("Writer").await;
    let own_topic = topic(&world, origin).await;
    let gifted = world
        .engine()
        .share_with_agents(
            &Caller::session(world.ada),
            &Origin::default(),
            &ShareWithAgents {
                link_origin: None,
                viewers_read: None,
                read_first: None,
                what: ShareWhat::Node(ShareWhatNode { node_id: own_topic }),
                inside: Some(true),
                download: Some(false),
                parts: Some(ShareParts {
                    documents: Some(ShareDocumentsLevel::Edit),
                    ..ShareParts::default()
                }),
                mode: ShareMode::Live,
                to: ShareTo::Threads(vec![ShareToThread { thread_id: origin }]),
                comment: None,
                expires_at: None,
                client_id: "explicit-topic-material".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(gifted.len(), 1);
    let gift = gifted[0].id;
    let source = world
        .document_beside(own_topic, "Public plan", &["Public topic words"])
        .await;
    ok(call(
        &world,
        origin,
        OrchTool::ReadShared,
        json!({"path":format!("id:{source}")}),
    )
    .await);
    let listing = ok(call(
        &world,
        origin,
        OrchTool::ListShared,
        json!({"path":format!("id:{own_topic}")}),
    )
    .await);
    assert_eq!(entry(&listing, "docs")["writable"], true);
    let made = ok(call(&world, origin, OrchTool::EditDocument, json!({"command":"create","path":format!("{}Made.md",entry(&listing,"docs")["path"].as_str().unwrap()),"markdown":"The creation entry remains"})).await);
    let export = call(
        &world,
        origin,
        OrchTool::DownloadShared,
        json!({"path":format!("id:{source}")}),
    )
    .await;
    assert_eq!(
        shared::refused(&export).0,
        "not_downloadable",
        "the selected document gift grants no Export"
    );
    let private = world
        .document_beside(own_topic, "Private", &["Never implicit"])
        .await;
    cut_document_rows(
        &world,
        orch_authz::pages::Target::Resource(orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Document,
            id: private,
        }),
    )
    .await;

    assert!(
        call(
            &world,
            origin,
            OrchTool::ReadShared,
            json!({"path":format!("id:{private}")})
        )
        .await
        .error
        .is_some()
    );
    let peer = thread_in_topic(&world, own_topic, "Other main thread").await;
    let peer_doc = world.document_beside(peer, "Peer", &["Same topic"]).await;
    ok(call(
        &world,
        origin,
        OrchTool::ReadShared,
        json!({"path":format!("id:{peer_doc}")}),
    )
    .await);
    sqlx::query("INSERT INTO permission_rows(object_kind,object_id,page,profile,permission,inherit) VALUES('node',$1,'live','any','thread.view',false) ON CONFLICT(object_kind,object_id,page,profile,permission) DO UPDATE SET inherit=false")
        .bind(peer).execute(world.pool.pool()).await.unwrap();
    // Conversation and document rows are independent: privacy of these documents is an
    // actual Doc cut, not an assumption that cutting conversation View cuts another area.
    cut_document_rows(&world, orch_authz::pages::Target::Live(peer)).await;
    assert!(
        call(
            &world,
            origin,
            OrchTool::ReadShared,
            json!({"path":format!("id:{peer_doc}")})
        )
        .await
        .error
        .is_some()
    );
    let side = thread_in_topic(&world, own_topic, "Side").await;
    sqlx::query("UPDATE threads SET side_of=$2 WHERE node_id=$1")
        .bind(side)
        .bind(origin)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let side_doc = world
        .document_beside(side, "Side material", &["Private side material"])
        .await;
    cut_document_rows(
        &world,
        orch_authz::pages::Target::Resource(orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Document,
            id: side_doc,
        }),
    )
    .await;
    assert!(
        call(
            &world,
            origin,
            OrchTool::ReadShared,
            json!({"path":format!("id:{side_doc}")})
        )
        .await
        .error
        .is_some()
    );
    world
        .engine()
        .update_share(
            &Caller::session(world.ada),
            &Origin::default(),
            gift,
            &ShareAgentUpdateBody {
                parts: Some(ShareParts {
                    documents: Some(ShareDocumentsLevel::Read),
                    ..ShareParts::default()
                }),
                ..ShareAgentUpdateBody::default()
            },
        )
        .await
        .unwrap();
    ok(call(
        &world,
        origin,
        OrchTool::ReadShared,
        json!({"path":format!("id:{source}")}),
    )
    .await);
    let read = ok(call(
        &world,
        origin,
        OrchTool::ListShared,
        json!({"path":format!("id:{own_topic}")}),
    )
    .await);
    assert_eq!(entry(&read, "docs")["writable"], false);
    world
        .engine()
        .revoke_share(&Caller::session(world.ada), &Origin::default(), gift)
        .await
        .unwrap();
    assert!(
        call(
            &world,
            origin,
            OrchTool::ReadShared,
            json!({"path":format!("id:{source}")})
        )
        .await
        .error
        .is_some()
    );
    ok(call(
        &world,
        origin,
        OrchTool::ReadShared,
        json!({"path":format!("id:{}",made["id"].as_str().unwrap())}),
    )
    .await);
    share(
        &world, "document", source, "live", "doc.view", "any", origin,
    )
    .await;
    ok(call(
        &world,
        origin,
        OrchTool::ReadShared,
        json!({"path":format!("id:{source}")}),
    )
    .await);
}

#[tokio::test]
async fn read_first_follows_renames_and_revocation_without_creating_an_input_or_copying_any_body() {
    let world = a_world_whose_agent_takes_tools().await;
    let origin = world.thread("Reader").await;
    let elsewhere = world.topic("Material").await;
    let source = world
        .document_beside(elsewhere, "Before name", &["Body must never be context"])
        .await;
    let inputs = world
        .count(
            "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
            origin,
        )
        .await;
    let request = ShareWithAgents {
        link_origin: None,
        viewers_read: None,
        read_first: Some(true),
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: source,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: origin }]),
        comment: None,
        expires_at: None,
        client_id: "material".to_owned(),
    };
    let records = world
        .engine()
        .share_with_agents(&Caller::session(world.ada), &Origin::default(), &request)
        .await
        .unwrap();
    assert!(records[0].read_first);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                origin
            )
            .await,
        inputs
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let first = world
        .engine()
        .topic_material(&mut conn, origin)
        .await
        .unwrap();
    assert_eq!(first.paths.len(), 1);
    assert_eq!(first.paths[0].id, source);
    assert!(first.paths[0].path.contains("Before name"));
    sqlx::query("UPDATE documents SET title='After name' WHERE id=$1")
        .bind(source)
        .execute(&mut *conn)
        .await
        .unwrap();
    let renamed = world
        .engine()
        .topic_material(&mut conn, origin)
        .await
        .unwrap();
    assert!(renamed.paths[0].path.contains("After name"));
    assert!(
        !serde_json::to_string(&renamed.paths)
            .unwrap()
            .contains("Body must never")
    );
    sqlx::query("UPDATE permission_entries SET expires_at='2000-01-01T00:00:00Z' WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents'")
        .bind(source).execute(&mut *conn).await.unwrap();
    assert!(
        world
            .engine()
            .topic_material(&mut conn, origin)
            .await
            .unwrap()
            .paths
            .is_empty()
    );
}
