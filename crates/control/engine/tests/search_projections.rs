//! Live and rebuilt search projections: secret masking, side-thread exclusion and current chat/document text.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::{
    queries::{
        hierarchy::testing::insert_node, identity::testing::insert_user,
        resources::access_fixtures::insert_resource, threads,
    },
    secrets::MasterKey,
};
use orch_engine::{
    ingest::{ItemSpec, add_item},
    projections,
    unit::Unit,
};
use orch_settings::secrets::{Owner, Put};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Json, new_id};
use serde_json::json;

struct World {
    db: TestDb,
    owner: Id,
    topic: Id,
    thread: Id,
    segment: Id,
}

impl World {
    async fn new() -> Self {
        let db = TestDb::fresh().await;
        let (owner, org, project, topic, thread, segment) =
            (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        let mut conn = db.pool.acquire().await.unwrap();
        insert_user(&mut conn, owner, "ada").await.unwrap();
        insert_node(&mut conn, org, "org", None, "Acme", owner)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(org), "Web", owner)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Plans", owner)
            .await
            .unwrap();
        insert_node(&mut conn, thread, "thread", Some(topic), "Main", owner)
            .await
            .unwrap();
        let agent = insert_resource(&mut conn, "agent", Some(owner), Some(org), "Agent")
            .await
            .unwrap();
        let provider = insert_resource(&mut conn, "provider", Some(owner), Some(org), "Provider")
            .await
            .unwrap();
        let environment =
            insert_resource(&mut conn, "environment", Some(owner), Some(topic), "Main")
                .await
                .unwrap();
        sqlx::query("INSERT INTO segments (id,thread_id,ordinal,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,seq_updated) VALUES ($1,$2,1,$3,$4,$5,1,'separated','forwarded',1)")
            .bind(segment).bind(thread).bind(agent).bind(provider).bind(environment).execute(&mut *conn).await.unwrap();
        drop(conn);
        Self {
            db,
            owner,
            topic,
            thread,
            segment,
        }
    }

    async fn item(&self, kind: &str, payload: &Json) -> threads::ItemRow {
        let mut tx = self.db.pool.begin().await.unwrap();
        let mut unit = Unit::new(&mut tx, clock::start());
        let row = add_item(
            &mut unit,
            &ItemSpec {
                thread: self.thread,
                segment: self.segment,
                turn: None,
                stream: "main",
                parent: None,
                kind,
                status: "completed",
                author: Some(self.owner),
                payload,
                native_ref: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        unit.finish().await.unwrap();
        tx.commit().await.unwrap();
        row
    }
}

#[tokio::test]
async fn live_item_text_and_a_full_rebuild_agree_without_changing_attention_or_sequences() {
    let world = World::new().await;
    let item = world
        .item(
            "agent_message",
            &json!({"text":"你好 release notes","text_len":16,"phase":"final"}),
        )
        .await;
    assert_eq!(item.search_text.as_deref(), Some("你好 release notes"));
    let mut conn = world.db.pool.acquire().await.unwrap();
    let before = threads::thread(&mut conn, world.thread)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE items SET search_text = 'broken' WHERE id=$1")
        .bind(item.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let result = projections::rebuild(&mut conn).await.unwrap();
    assert_eq!(result.items, 1);
    assert_eq!(
        threads::item(&mut conn, item.id)
            .await
            .unwrap()
            .unwrap()
            .search_text,
        item.search_text
    );
    let after = threads::thread(&mut conn, world.thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (before.next_seq, before.activity_seq),
        (after.next_seq, after.activity_seq)
    );
    assert_eq!(
        projections::rebuild(&mut conn).await.unwrap(),
        projections::Rebuilt::default()
    );
}

#[tokio::test]
async fn a_side_thread_never_has_search_text_even_when_an_old_index_is_rebuilt() {
    let world = World::new().await;
    let mut conn = world.db.pool.acquire().await.unwrap();
    let side = new_id();
    insert_node(
        &mut conn,
        side,
        "thread",
        Some(world.thread),
        "Aside",
        world.owner,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET topic_id=$2,side_of=$3 WHERE node_id=$1")
        .bind(side)
        .bind(world.topic)
        .bind(world.thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        projections::item_text(
            &mut conn,
            side,
            "agent_message",
            &json!({"text":"private aside"})
        )
        .await
        .unwrap(),
        None
    );
    // A legacy bad projection still belongs to this side thread's actual segment.
    // History ownership guards must remain in force while its search text is repaired.
    let side_segment = new_id();
    sqlx::query("INSERT INTO segments (id,thread_id,ordinal,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,seq_updated) SELECT $1,$2,1,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,1 FROM segments WHERE id=$3")
        .bind(side_segment).bind(side).bind(world.segment).execute(&mut *conn).await.unwrap();
    let id = new_id();
    sqlx::query("INSERT INTO items (id,thread_id,segment_id,stream,kind,status,payload,search_text,seq_created,seq_updated) VALUES ($1,$2,$3,'main','agent_message','completed',$4,'private aside',1,1)")
        .bind(id).bind(side).bind(side_segment).bind(json!({"text":"private aside"})).execute(&mut *conn).await.unwrap();
    projections::rebuild(&mut conn).await.unwrap();
    assert_eq!(
        threads::item(&mut conn, id)
            .await
            .unwrap()
            .unwrap()
            .search_text,
        None
    );
}

#[tokio::test]
async fn patterns_and_resolved_secrets_are_masked_before_item_text_is_indexed() {
    let world = World::new().await;
    let key = MasterKey::new([7; 32]);
    let _installed = projections::install_master_key(key.clone());
    let mut conn = world.db.pool.acquire().await.unwrap();
    orch_db::queries::settings::put(
        &mut conn,
        Some(world.topic),
        "redaction.patterns",
        &orch_db::queries::settings::Stored {
            value: &json!([{"name":"token","regex":"sk-[a-z0-9]+"}]),
            locked: false,
            by: Some(world.owner),
            at: clock::start(),
        },
        None,
    )
    .await
    .unwrap();
    let actor = orch_db::audit::Actor::User(world.owner);
    let origin = orch_db::audit::Origin::default();
    orch_settings::secrets::put(
        &mut conn,
        &key,
        &Put {
            owner: Owner::Node(world.topic),
            name: "API_KEY",
            value: "sensitive-value",
            actor,
            origin: &origin,
        },
        clock::start(),
    )
    .await
    .unwrap();
    drop(conn);
    let item = world
        .item(
            "agent_message",
            &json!({"text":"use sensitive-value and sk-abcdef now","text_len":42,"phase":"final"}),
        )
        .await;
    assert_eq!(
        item.search_text.as_deref(),
        Some("use [masked] and [masked] now")
    );
}

#[tokio::test]
async fn rebuild_uses_current_chat_parts_and_document_blocks_and_clears_removed_content() {
    let world = World::new().await;
    let mut conn = world.db.pool.acquire().await.unwrap();
    let chat: Id = sqlx::query_scalar("SELECT id FROM chats WHERE node_id=$1")
        .bind(world.topic)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let (message, gone_message, doc, block, removed) =
        (new_id(), new_id(), new_id(), new_id(), new_id());
    for (id, deleted, seq) in [(message, false, 1), (gone_message, true, 2)] {
        sqlx::query("INSERT INTO chat_messages (id,chat_id,kind,author_id,content,search_text,seq_created,seq_updated,deleted_at) VALUES ($1,$2,'message',$3,$4,'old text',$5,$5,CASE WHEN $6 THEN now() END)")
            .bind(id).bind(chat).bind(world.owner).bind(json!([{"type":"text","text":"current chat text"}])).bind(seq as i64).bind(deleted).execute(&mut *conn).await.unwrap();
    }
    sqlx::query("INSERT INTO documents (id,node_id,org_id,owner_id,title) SELECT $1,$2,org_id,$3,'Handbook' FROM nodes WHERE id=$2")
        .bind(doc).bind(world.topic).bind(world.owner).execute(&mut *conn).await.unwrap();
    for (id, gone, position) in [(block, false, 1), (removed, true, 2)] {
        sqlx::query("INSERT INTO document_blocks (id,document_id,position,type,props,content,text,seq_created,seq_updated,created_by,removed_at) VALUES ($1,$2,$3,'paragraph','{}',$4,'old text',1,1,$5,CASE WHEN $6 THEN now() END)")
            .bind(id).bind(doc).bind(position as i64).bind(json!([{"type":"text","text":"current block text","marks":[]}])).bind(world.owner).bind(gone).execute(&mut *conn).await.unwrap();
    }
    let changed = projections::rebuild(&mut conn).await.unwrap();
    assert_eq!((changed.messages, changed.blocks), (2, 2));
    let texts: Vec<(Id, Option<String>)> =
        sqlx::query_as("SELECT id,search_text FROM chat_messages ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        texts
            .iter()
            .find(|(id, _)| *id == message)
            .unwrap()
            .1
            .as_deref(),
        Some("current chat text")
    );
    assert_eq!(
        texts.iter().find(|(id, _)| *id == gone_message).unwrap().1,
        None
    );
    let texts: Vec<(Id, String)> =
        sqlx::query_as("SELECT id,text FROM document_blocks ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        texts.iter().find(|(id, _)| *id == block).unwrap().1,
        "current block text"
    );
    assert_eq!(texts.iter().find(|(id, _)| *id == removed).unwrap().1, "");
}

#[tokio::test]
async fn repair_clears_a_legacy_projection_even_when_its_source_is_json_null() {
    let world = World::new().await;
    let item = world.item("marker", &Json::Null).await;
    let mut conn = world.db.pool.acquire().await.unwrap();
    sqlx::query("UPDATE items SET search_text='obsolete text' WHERE id=$1")
        .bind(item.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    projections::rebuild(&mut conn).await.unwrap();
    assert_eq!(
        threads::item(&mut conn, item.id)
            .await
            .unwrap()
            .unwrap()
            .search_text,
        None
    );
}
