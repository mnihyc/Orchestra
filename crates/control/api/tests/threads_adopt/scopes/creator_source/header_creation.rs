//! Retrying the accepted birth never rediscovers, reimports or adopts its native session again.
use super::*;

async fn keyed(s: &Scene, token: &str, act: Id, body: &ThreadAdoptBody) -> RawReply {
    let call = s
        .rig
        .world
        .app
        .call(ThreadAdopt, &[&s.tree.topic.base.id])
        .token(token)
        .header("Idempotency-Key", &act.to_string())
        .body(body);
    call.send_bytes().await
}

async fn one_birth(history: bool) {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    if history {
        the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    } else {
        a_history_read_fails(&s);
    }
    let environment = registered(&s, "/work/web").await;
    let (original, secret) = key(&s, act(&s, environment)).await;
    let act = new_id();
    let wanted = request(&s, environment, "One adopted birth");
    let first = accepted(&keyed(&s, &secret, act, &wanted).await);
    assert_eq!(first["projection"], "receipt");
    assert_eq!(first["history_items"], if history { 4 } else { 0 });
    let thread: Id = serde_json::from_value(first["thread_id"].clone()).unwrap();
    let segment: Id = serde_json::from_value(first["segment_id"].clone()).unwrap();
    assert_eq!(first["topic_id"], json!(s.tree.topic.base.id));
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(s.ada.id())
    .bind(act.to_string())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(before.0["outcome"]["kind"], "adopted_thread");
    assert_eq!(before.0["outcome"]["id"], json!(thread));
    assert_eq!(before.0["outcome"]["data"]["segment"], json!(segment));
    assert!(!before.0.to_string().contains("One adopted birth"));
    let commands = s.rig.machine.handler.executions().len();
    let before_items: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    identity::revoke_api_token(
        &mut *s.rig.world.conn().await,
        original,
        s.ada.id(),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE nodes SET name='The adopted thread today' WHERE id=$1")
        .bind(thread)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let current_rules: Vec<Json> = all(&s, environment)
        .into_iter()
        .map(|mut rule| {
            rule["allow"][0]["actions"] = json!(["read"]);
            rule
        })
        .collect();
    let (_, reader) = key(&s, current_rules).await;
    let current = accepted(&keyed(&s, &reader, act, &wanted).await);
    assert_eq!(current["id"], json!(thread));
    assert_eq!(current["name"], "The adopted thread today");
    assert!(current.get("projection").is_none());
    let (_, neighbour) = key(
        &s,
        vec![rule(
            "ThreadGet.read",
            "thread",
            thread,
            &["thread"],
            &["read"],
        )],
    )
    .await;
    assert_eq!(accepted(&keyed(&s, &neighbour, act, &wanted).await), first);
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(s.ada.id())
    .bind(act.to_string())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "current readers do not rewrite the accepted birth"
    );
    assert_eq!(s.rig.machine.handler.executions().len(), commands);
    let after_items: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(before_items, after_items);
    let sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM segments WHERE provider_id=$1 AND agent_id=$2 AND native_session_ref='native-a'",
    )
    .bind(s.on.provider)
    .bind(s.on.agent)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(sessions, 1);
    assert_eq!(segment_ref(&s, thread).await.as_deref(), Some("native-a"));
    assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
}

#[tokio::test]
async fn a_header_retries_one_adopted_thread_and_import_under_only_current_read() {
    one_birth(true).await;
}

#[tokio::test]
async fn a_header_keeps_the_adopted_birth_with_missing_history_without_trying_the_native_record_again()
 {
    one_birth(false).await;
}
