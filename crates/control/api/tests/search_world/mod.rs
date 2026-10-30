//! Conversation fixtures for search tests, with live ingestion and valid segment references on a real database.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each search test uses some of these fixtures")]

use crate::{desk::Desk, places::Who};
use orch_access_contract::pages::StoredKind;
use orch_db::queries::{
    hierarchy::testing::insert_node, resources::access_fixtures::insert_resource,
};
use orch_engine::{
    ingest::{ItemSpec, add_item},
    unit::Unit,
};
use orch_types::{Id, Time, new_id};
use serde_json::json;

pub async fn thread(desk: &Desk, by: &Who, topic: Id, name: &str) -> (Id, Id) {
    let (thread, segment) = (new_id(), new_id());
    let mut conn = desk.world.conn().await;
    insert_node(&mut conn, thread, "thread", Some(topic), name, by.id())
        .await
        .unwrap();
    orch_authz::pages::start(
        &mut conn,
        StoredKind::Node,
        thread,
        by.id(),
        Time::new(desk.world.now()),
    )
    .await
    .unwrap();
    let agent = insert_resource(
        &mut conn,
        "agent",
        Some(by.id()),
        Some(desk.org),
        &format!("Search agent {segment}"),
    )
    .await
    .unwrap();
    let provider = insert_resource(
        &mut conn,
        "provider",
        Some(by.id()),
        Some(desk.org),
        &format!("Search provider {segment}"),
    )
    .await
    .unwrap();
    let environment = insert_resource(
        &mut conn,
        "environment",
        Some(by.id()),
        Some(topic),
        &format!("Search environment {segment}"),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO segments (id,thread_id,ordinal,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,seq_updated) VALUES ($1,$2,1,$3,$4,$5,1,'separated','forwarded',1)")
        .bind(segment).bind(thread).bind(agent).bind(provider).bind(environment).execute(&mut *conn).await.unwrap();
    (thread, segment)
}

pub async fn say(
    desk: &Desk,
    by: &Who,
    thread: Id,
    segment: Id,
    words: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Id {
    let payload = json!({"parts":[{"type":"text","text":words}],"input_id":new_id(),"delivery":"queue","source":"user"});
    let mut tx = desk.world.app.app.db.begin().await.unwrap();
    let mut unit = Unit::new(&mut tx, at);
    let item = add_item(
        &mut unit,
        &ItemSpec {
            thread,
            segment,
            turn: None,
            stream: "main",
            parent: None,
            kind: "user_message",
            status: "completed",
            author: Some(by.id()),
            payload: &payload,
            native_ref: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    item.id
}

pub async fn cut_view(desk: &Desk, thread: Id) {
    let mut conn = desk.world.conn().await;
    sqlx::query("INSERT INTO permission_rows (object_kind,object_id,page,profile,permission,inherit) VALUES ('node',$1,'live','any','thread.view',false) ON CONFLICT (object_kind,object_id,page,profile,permission) DO UPDATE SET inherit=false")
        .bind(thread).execute(&mut *conn).await.unwrap();
}
