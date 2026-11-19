//! Real list_shared calls page stable paths, keep recent active work first and fold archives without losing a continuation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::Time;
use serde_json::{Value, json};
use shared::*;
use std::collections::BTreeSet;

fn ids(page: &Value) -> Vec<String> {
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn every_named_page_and_a_connector_row_cut_resume_without_skipping_or_duplicating_work() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    let other = world.thread("Second reader").await;
    let alpha = world.thread_in(world.topic, "Alpha").await;
    let beta = world.thread_in(world.topic, "Beta").await;
    let zulu = world.thread_in(world.topic, "Zulu").await;
    let query = json!({"sort":"name","kinds":["thread"],"limit":2,"ids":true});
    let first = ok(call(&world, mine, OrchTool::ListShared, query.clone()).await);
    assert_eq!(ids(&first), vec![alpha.to_string(), beta.to_string()]);
    assert_eq!(first["truncated"], true);
    let row_cursor = first["entries"][0]["cursor"].as_str().unwrap();
    let cut = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"sort":"name","kinds":["thread"],"limit":1,"cursor":row_cursor,"ids":true}),
    )
    .await);
    assert_eq!(
        ids(&cut),
        vec![beta.to_string()],
        "a byte cut resumes after its last displayed row, not the engine's page end"
    );
    let mut seen: BTreeSet<_> = ids(&first).into_iter().collect();
    let mut page = first.clone();
    while let Some(cursor) = page["next_cursor"].as_str() {
        let mut next = query.clone();
        next["cursor"] = json!(cursor);
        page = ok(call(&world, mine, OrchTool::ListShared, next).await);
        for id in ids(&page) {
            assert!(seen.insert(id), "no row may repeat on an unchanged listing");
        }
    }
    assert_eq!(
        seen,
        [mine, other, alpha, beta, zulu]
            .into_iter()
            .map(|id| id.to_string())
            .collect()
    );
    assert_eq!(page["truncated"], false);
    let cursor = first["next_cursor"].as_str().unwrap();
    for changed in [
        json!({"sort":"changed","kinds":["thread"],"limit":2,"cursor":cursor}),
        json!({"sort":"name","kinds":["document"],"limit":2,"cursor":cursor}),
    ] {
        assert_eq!(
            refused(&call(&world, mine, OrchTool::ListShared, changed).await).0,
            "invalid"
        );
    }
    let foreign = call(
        &world,
        other,
        OrchTool::ListShared,
        json!({"sort":"name","kinds":["thread"],"limit":2,"cursor":cursor}),
    )
    .await;
    assert_eq!(
        refused(&foreign).0,
        "invalid",
        "a continuation belongs to its asking thread"
    );
    let gone = first["entries"][1]["id"].as_str().unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE nodes SET deleted_at=now() WHERE id=$1")
        .bind(gone.parse::<orch_types::Id>().unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        refused(
            &call(
                &world,
                mine,
                OrchTool::ListShared,
                json!({"sort":"name","kinds":["thread"],"limit":2,"cursor":cursor})
            )
            .await
        )
        .0,
        "invalid"
    );
}

#[tokio::test]
async fn shipped_order_prioritizes_active_work_then_maintained_activity_and_name_is_an_explicit_choice()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    let active = world.thread("Zulu active").await;
    let alpha = world.thread_in(world.topic, "Alpha recent").await;
    let old = world.thread_in(world.topic, "Beta older").await;
    share(&world, "node", active, "live", "thread.view", "any", mine).await;
    // The reader is a real active agent too. Establish it before fixing the
    // chronology, so this call cannot manufacture a newer activity timestamp.
    shared::prepare_call_turn(&world, mine).await;
    let before = world.turn_starts(active).len();
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            active,
            common::queued("Keep working"),
        )
        .await
        .unwrap();
    let turn = world.turn_start(active, before).await;
    world
        .answer(active, &turn, "active-listing-order", "Working")
        .await;
    world.wait_state(active, "running").await;
    the_agent_takes_tools_again(&world).await;
    let recent = chrono::DateTime::parse_from_rfc3339("2026-10-10T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let older = recent - chrono::Duration::days(1);
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE threads SET last_activity_at=$2 WHERE node_id=ANY($1)")
        .bind(vec![mine, active, old])
        .bind(older)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET last_activity_at=$2 WHERE node_id=$1")
        .bind(active)
        .bind(older + chrono::Duration::seconds(1))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET last_activity_at=$2 WHERE node_id=$1")
        .bind(alpha)
        .bind(recent)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let changed = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"kinds":["thread"],"ids":true}),
    )
    .await);
    assert_eq!(changed["sort"], "changed");
    assert_eq!(
        ids(&changed)[0],
        active.to_string(),
        "active work precedes a newer idle thread"
    );
    let ordered = ids(&changed);
    assert!(
        ordered
            .iter()
            .position(|id| id == &alpha.to_string())
            .unwrap()
            < ordered
                .iter()
                .position(|id| id == &old.to_string())
                .unwrap()
    );
    assert_eq!(
        entry(&changed, "Alpha recent")["updated_at"],
        json!(Time::new(recent))
    );
    assert!(
        entry(&changed, "Zulu active")["children"]
            .as_i64()
            .is_some_and(|count| count > 0)
    );
    let named = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"kinds":["thread"],"sort":"name","ids":true}),
    )
    .await);
    assert_eq!(ids(&named)[0], alpha.to_string());
}

#[tokio::test]
async fn archives_are_counted_once_until_asked_for_and_topic_rows_carry_their_actual_status() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    let archived = world.thread_in(world.topic, "Old work").await;
    let nested = world.topic("Old topic").await;
    let _inside = world.thread_in(nested, "Old child").await;
    share_threads_inside(&world, world.project, mine, false).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE nodes SET archived_at=now() WHERE id=ANY($1)")
        .bind(vec![archived, nested])
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET status='done' WHERE node_id=$1")
        .bind(nested)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let normal = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{}",world.project),"depth":2,"ids":true}),
    )
    .await);
    assert_eq!(
        normal["archived_count"], 2,
        "an archived topic is one folded entry; its descendants are not traversed"
    );
    assert!(!ids(&normal).contains(&archived.to_string()));
    assert!(!ids(&normal).contains(&nested.to_string()));
    let included = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"path":format!("id:{}",world.project),"depth":2,"include_archived":true}),
    )
    .await);
    let done = entry(&included, "Old topic");
    assert_eq!(done["status"], "done");
    assert_eq!(done["archived"], true);
    assert!(done["children"].as_i64().is_some_and(|count| count > 0));
    assert!(!entry(&included, "Old work")["writable"].as_bool().unwrap());
    assert_eq!(included["archived_count"], 0);
    let bad = call(&world, mine, OrchTool::ListShared, json!({"limit":0})).await;
    assert_eq!(refused(&bad).0, "invalid");
}

#[tokio::test]
async fn the_default_fifty_row_page_reaches_the_end_and_the_existing_two_hundred_row_bound_is_enforced()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Reader").await;
    let mut conn = world.pool.acquire().await.unwrap();
    // One beyond the shared pager's 200-row maximum proves continuation, not a larger invented page bound.
    sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
        SELECT made.id,'thread',p.id,p.org_id,p.path || made.id,'Work ' || lpad(number::text,4,'0'),$2,$2
        FROM nodes p CROSS JOIN (SELECT gen_random_uuid() id,number FROM generate_series(1,201) number) made WHERE p.id=$1")
        .bind(world.topic).bind(world.ada).execute(&mut *conn).await.unwrap();
    // A thread's row starts its history (its first continuation), so only the threads made here get one.
    sqlx::query("INSERT INTO threads(node_id,topic_id) SELECT id,parent_id FROM nodes WHERE parent_id=$1 AND kind='thread' AND name LIKE 'Work %'")
        .bind(world.topic).execute(&mut *conn).await.unwrap();
    drop(conn);
    let mut page = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"kinds":["thread"],"sort":"name","ids":true}),
    )
    .await);
    assert_eq!(ids(&page).len(), 50);
    let mut seen: BTreeSet<_> = ids(&page).into_iter().collect();
    while let Some(cursor) = page["next_cursor"].as_str() {
        page = ok(call(
            &world,
            mine,
            OrchTool::ListShared,
            json!({"kinds":["thread"],"sort":"name","cursor":cursor,"ids":true}),
        )
        .await);
        for id in ids(&page) {
            assert!(seen.insert(id));
        }
    }
    assert_eq!(seen.len(), 202);
    let max = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"kinds":["thread"],"limit":200,"ids":true}),
    )
    .await);
    assert_eq!(ids(&max).len(), 200);
    assert!(max["next_cursor"].is_string());
    assert_eq!(
        refused(&call(&world, mine, OrchTool::ListShared, json!({"limit":201})).await).0,
        "invalid"
    );
}
