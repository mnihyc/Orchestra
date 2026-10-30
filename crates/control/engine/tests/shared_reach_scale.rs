//! Real agent tools retain reachable paths and activity without paying for unrelated branches or changing stable names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::Time;
use serde_json::json;
use shared::*;

#[tokio::test]
async fn unrelated_work_beyond_the_old_root_limit_does_not_truncate_the_agents_reach() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("My work").await;
    let elsewhere = world.project_in_root("Unrelated work").await;
    // 20,001 is the first count beyond the old root-wide 20,000-node limit, entirely outside this agent's reach.
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
        SELECT made.id,'topic',p.id,p.org_id,p.path || made.id,'Unrelated ' || number::text,$2,$2
        FROM nodes p CROSS JOIN (SELECT gen_random_uuid() id,number FROM generate_series(1,20001) number) made WHERE p.id=$1")
        .bind(elsewhere).bind(world.ada).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO topics(node_id) SELECT id FROM nodes WHERE parent_id=$1")
        .bind(elsewhere)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let shown = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":"/","depth":4}),
    )
    .await);
    assert_eq!(
        shown["truncated"], false,
        "unreachable branches cannot consume the reach's candidate budget: {shown}"
    );
    assert!(paths(&shown).iter().any(|path| path.ends_with("/My work/")));
    assert!(
        !serde_json::to_string(&shown)
            .unwrap()
            .contains("Unrelated work")
    );
    let missing = call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{elsewhere}")}),
    )
    .await;
    assert_eq!(refused(&missing).0, "not_found");
}

#[tokio::test]
async fn granted_pages_keep_ancestors_and_subtrees_and_names_do_not_change_when_a_hidden_twin_is_shared()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    let _hidden = world.project_in_root("Plans").await;
    let project = world.project_in_root("Plans").await;
    let topic = world.topic_in(project, "Design").await;
    // Two documents of one directory never share a name (the second is refused), so the twins are threads of
    // the topic; the document beside them is shared and withdrawn.
    let old = world.thread_in(topic, "Decision").await;
    let newer = world.thread_in(topic, "Decision").await;
    let decision = world
        .document_beside(topic, "Decision", &["Readable decision"])
        .await;
    share(&world, "node", newer, "live", "thread.view", "any", mine).await;
    let grant = share(
        &world, "document", decision, "live", "doc.view", "any", mine,
    )
    .await;
    let first = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{topic}"),"depth":2,"ids":true}),
    )
    .await);
    let first_entry = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == json!(newer))
        .unwrap();
    let stable = first_entry["path"].clone();
    assert!(
        stable.as_str().unwrap().contains("Plans~"),
        "ancestor names include older hidden siblings: {stable}"
    );
    assert!(
        first_entry["name"].as_str().unwrap().contains('~'),
        "a directly shared thread keeps its name among all siblings"
    );
    assert!(
        !first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == json!(old))
    );
    share(&world, "node", old, "live", "thread.view", "any", mine).await;
    let second = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{topic}"),"depth":2,"ids":true}),
    )
    .await);
    assert_eq!(
        second["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == json!(newer))
            .unwrap()["path"],
        stable
    );
    let nested = world
        .document_beside(topic, "Subtree", &["Given through the project's defaults"])
        .await;
    share(&world, "node", project, "defaults", "doc.view", "any", mine).await;
    let below = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{project}"),"depth":3,"ids":true}),
    )
    .await);
    assert!(
        below["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == json!(nested))
    );
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::access::remove_entry(&mut conn, grant, world.ada, chrono::Utc::now())
        .await
        .unwrap();
    // Cut the inherited source too: another current grant must not revive a withdrawn private document.
    sqlx::query("INSERT INTO permission_rows(object_kind,object_id,page,profile,permission,inherit)
        VALUES('document',$1,'live','any','doc.view',false) ON CONFLICT(object_kind,object_id,page,profile,permission) DO UPDATE SET inherit=false")
        .bind(decision).execute(&mut *conn).await.unwrap();
    drop(conn);
    assert_eq!(
        refused(
            &call(
                &world,
                mine,
                OrchTool::ReadShared,
                json!({"path":format!("id:{decision}")})
            )
            .await
        )
        .0,
        "not_found"
    );
}

#[tokio::test]
async fn project_policy_expands_only_that_project_and_conversation_age_uses_maintained_thread_activity()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    let nearby = world.topic("Other topic").await;
    let peer = thread_in_topic(&world, nearby, "Peer").await;
    let far_project = world.project_in_root("Other project").await;
    let far_topic = world.topic_in(far_project, "Outside").await;
    let far = thread_in_topic(&world, far_topic, "Far peer").await;
    world
        .set(world.topic, "sharing.agent_reach", json!("project"))
        .await;
    let visible = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{peer}")}),
    )
    .await);
    assert!(
        entry(&visible, "conversation.md")["readable"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        refused(
            &call(
                &world,
                mine,
                OrchTool::ReadShared,
                json!({"path":format!("id:{far}")})
            )
            .await
        )
        .0,
        "not_found"
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-10-08T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE threads SET last_activity_at=$2 WHERE node_id=$1")
        .bind(peer)
        .bind(stamp)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let dated = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{peer}")}),
    )
    .await);
    assert_eq!(
        entry(&dated, "conversation.md")["updated_at"],
        json!(Time::new(stamp))
    );
}

#[tokio::test]
async fn a_reachable_subtree_follows_its_raw_pages_and_keeps_both_ends_of_the_old_limit() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    // One beyond the former candidate cap, all genuinely inside the implicit topic.
    const COUNT: i64 = 20_001;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at)
        SELECT made.id,'thread',p.id,p.org_id,p.path || made.id,'Bound ' || lpad(number::text,5,'0'),$2,$2,
               $4::timestamptz - interval '1 day' + number * interval '1 second',
               $4::timestamptz - interval '1 day' + number * interval '1 second'
        FROM nodes p CROSS JOIN (SELECT gen_random_uuid() id,number FROM generate_series(1,$3::bigint) number) made WHERE p.id=$1")
        .bind(world.topic).bind(world.ada).bind(COUNT).bind(world.clock.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO threads(node_id,topic_id) SELECT id,parent_id FROM nodes WHERE parent_id=$1 AND name LIKE 'Bound %'")
        .bind(world.topic).execute(&mut *conn).await.unwrap();
    let raw = orch_db::queries::shares::below_page(&mut conn, world.topic, None, 2)
        .await
        .unwrap();
    assert!(raw.partial && raw.next_cursor.is_some());
    assert!(
        raw.items
            .windows(2)
            .all(|pair| (pair[0].created_at, pair[0].id) > (pair[1].created_at, pair[1].id))
    );
    drop(conn);
    let newest = ok(call(&world, mine, OrchTool::ListShared,
        json!({"path":format!("id:{}",world.topic),"names":"Bound *","depth":1,"limit":1,"sort":"changed"})).await);
    assert_eq!(newest["entries"][0]["name"], "Bound 20001");
    assert_eq!(newest["partial"], true);
    assert!(newest["next_cursor"].is_string());
    let oldest = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{}",world.topic),"names":"Bound 00001","depth":1,"limit":1}),
    )
    .await);
    assert_eq!(
        oldest["entries"][0]["name"], "Bound 00001",
        "following raw pages retains the oldest candidate too"
    );
    assert_eq!(oldest["partial"], false);
    assert!(oldest["next_cursor"].is_null());
}
