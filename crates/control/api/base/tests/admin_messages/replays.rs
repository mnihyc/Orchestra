//! Actual accepted org messages keep their frozen audience and single notice job across retries.
use super::*;
use http::StatusCode;

async fn send(world: &World, token: &str, org: Id, act: Id) -> RawReply {
    let call = world
        .app
        .call(OrgMessageCreate, &[&org])
        .token(token)
        .header("Idempotency-Key", &act.to_string())
        .body(&body());
    call.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn record(world: &World, user: Id, act: Id) -> (Json, String) {
    sqlx::query_as("SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2")
        .bind(user)
        .bind(act.to_string())
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn revoke(world: &World, token: &str) {
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(token_hash(token))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_org_message_header_keeps_one_frozen_audience_and_job_after_the_original_issuer_is_revoked()
 {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let member = places::person(&world, "accepted-message-member").await;
    places::join(&world, c.acme, c.acme, &member, "viewer", &c.boss).await;
    let writer = key(
        &world,
        c.boss.id(),
        vec![org_rule("OrgMessageCreate", c.acme, &["manage"])],
    )
    .await;
    let act = new_id();
    let receipt = accepted(send(&world, &writer, c.acme, act).await);
    assert_eq!(receipt["projection"], "receipt");
    let message: Id = serde_json::from_value(receipt["message_id"].clone()).unwrap();
    let job: Id = serde_json::from_value(receipt["delivery_job_id"].clone()).unwrap();
    let audience = recipients(&world, message).await;
    assert_eq!(receipt["recipient_count"], json!(audience.len()));
    let before = record(&world, c.boss.id(), act).await;
    assert_eq!(before.0["outcome"]["id"], json!(message));
    assert_eq!(before.0["outcome"]["data"]["job"], json!(job));
    assert_eq!(
        before.0["outcome"]["data"],
        json!({"count":audience.len(),"job":job})
    );
    let late = places::person(&world, "later-message-member").await;
    places::join(&world, c.acme, c.acme, &late, "viewer", &c.boss).await;
    revoke(&world, &writer).await;
    let reader = key(
        &world,
        c.boss.id(),
        vec![org_rule("OrgMessageCreate", c.acme, &["read"])],
    )
    .await;
    let read = accepted(send(&world, &reader, c.acme, act).await);
    assert_eq!(read["id"], json!(message));
    assert_eq!(read["body"], body().body);
    assert_eq!(read["recipient_count"], receipt["recipient_count"]);
    assert_eq!(read["delivery_job_id"], json!(job));
    let neighbour = key(
        &world,
        c.boss.id(),
        vec![org_rule("MessageGet", c.acme, &["read"])],
    )
    .await;
    assert_eq!(
        accepted(send(&world, &neighbour, c.acme, act).await),
        receipt
    );
    assert_eq!(record(&world, c.boss.id(), act).await, before);
    assert_eq!(recipients(&world, message).await, audience);
    assert_eq!(count(&world).await, 1);
    assert_eq!(
        admin_support::lines_of(&world, "message.sent").await.len(),
        1
    );
    world.app.app.engine.start();
    rig::eventually("the accepted notice job after its issuer ended", || async {
        let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        state == "succeeded"
    })
    .await;
    let delivered: Vec<Id> = sqlx::query_scalar(
        "SELECT user_id FROM notifications WHERE kind='admin_message' AND data->>'message_id'=$1",
    )
    .bind(message.to_string())
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(delivered.iter().copied().collect::<BTreeSet<_>>(), audience);
    assert_eq!(delivered.len(), audience.len());
    assert!(!delivered.contains(&late.id()));
    assert_eq!(record(&world, c.boss.id(), act).await, before);
    world.app.app.engine.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_an_org_message_waiter_after_its_real_commit_keeps_the_same_message_and_job() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let writer = key(
        &world,
        c.boss.id(),
        vec![org_rule("OrgMessageCreate", c.acme, &["manage", "read"])],
    )
    .await;
    let act = new_id();
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let org = c.acme;
    let token = writer.clone();
    let pending = tokio::spawn(async move {
        let call = app
            .call(OrgMessageCreate, &[&org])
            .token(&token)
            .header("Idempotency-Key", &act.to_string())
            .body(&body());
        call.send_bytes().await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!pending.is_finished(), "the real postcommit activity wait must be reached");
            let waits: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waits { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("observe the real committed message's activity wait");
    let (message, job): (Id, Id) =
        sqlx::query_as("SELECT id,delivery_job_id FROM admin_messages WHERE org_id=$1")
            .bind(org)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let before = record(&world, c.boss.id(), act).await;
    assert_eq!(before.0["outcome"]["id"], json!(message));
    pending.abort();
    assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
    blocker.commit().await.unwrap();
    revoke(&world, &writer).await;
    let no_read = key(
        &world,
        c.boss.id(),
        vec![org_rule("MessageGet", org, &["read"])],
    )
    .await;
    let receipt = accepted(send(&world, &no_read, org, act).await);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["message_id"], json!(message));
    assert_eq!(receipt["delivery_job_id"], json!(job));
    assert_eq!(count(&world).await, 1);
    assert_eq!(record(&world, c.boss.id(), act).await, before);
    assert_eq!(
        admin_support::lines_of(&world, "message.sent").await.len(),
        1
    );
}
