//! Compact waiting words arrive and clear on the existing org feed, without a second read of each thread.

mod realtime_support;

use orch_db::queries::hierarchy;
use orch_types::{Id, new_id};
use realtime_support::{Client, PATIENCE, Rig};
use serde_json::{Value, json};

/// The entire wait is bounded even if unrelated frames keep arriving.
async fn changed_thread(
    client: &mut Client,
    thread: Id,
    waiting: bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if frame["op"] != "sync" {
                continue;
            }
            if let Some(row) = frame["data"]["nodes"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["id"] == thread.to_string()))
                && row["thread"]["waiting"].is_object() == waiting
            {
                return row.clone();
            }
        }
    })
    .await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_org_feed_names_the_persons_outstanding_request_and_clears_it_after_the_decision()
-> Result<(), Box<dyn std::error::Error>> {
    let rig = Rig::new().await;
    let thread = rig.thread("Review deployment").await;
    let mut conn = rig.pool.acquire().await?;
    let before = hierarchy::tree_seq(&mut conn, rig.root).await?;
    drop(conn);
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &format!("org:{}", rig.root), Some(before))
        .await;
    client.expect("subscribed").await;
    let approval = new_id();
    let mut tx = rig.pool.begin().await?;
    sqlx::query("INSERT INTO approvals(id,kind,node_id,thread_id,title,request,required_permission,created_at,updated_at)
        VALUES($1,'harness',$2,$2,'Review deployment',$3,'thread.manage',$4,$4)")
        .bind(approval).bind(thread).bind(json!({"request_kind":"command","detail":{"command":"deploy","cwd":"/work","reason":null},"options":[]}))
        .bind(rig.clock.now()).execute(&mut *tx).await?;
    hierarchy::stamp(&mut tx, thread).await?;
    tx.commit().await?;
    let waiting = changed_thread(&mut client, thread, true).await?;
    assert_eq!(waiting["name"], "Review deployment");
    assert_eq!(waiting["thread"]["display"]["state"], "waiting_approval");
    assert_eq!(
        waiting["thread"]["waiting"],
        json!({"kind":"approval","for_caller":true})
    );
    let mut tx = rig.pool.begin().await?;
    sqlx::query("UPDATE approvals SET state='approved',decided_at=$2,updated_at=$2 WHERE id=$1")
        .bind(approval)
        .bind(rig.clock.now())
        .execute(&mut *tx)
        .await?;
    hierarchy::stamp(&mut tx, thread).await?;
    tx.commit().await?;
    let ready = changed_thread(&mut client, thread, false).await?;
    assert!(ready["thread"]["waiting"].is_null());
    assert_eq!(ready["thread"]["display"]["state"], "idle");
    rig.server.stop().await;
    Ok(())
}
