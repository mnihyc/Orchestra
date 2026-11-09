use super::*;

#[tokio::test]
async fn native_helper_membership_and_requesting_cap_scope_survive_replay_and_rollup_rebuild() {
    let (_test, pool) = world().await;
    let mut conn = pool.begin().await.unwrap();
    let helper = chain(&mut conn, true).await;
    let requester = new_id();
    assert!(
        insert_thread(
            &mut conn,
            requester,
            helper.topic,
            "Requesting thread",
            helper.person,
            Utc::now()
        )
        .await
        .unwrap()
    );
    let answer = new_id();
    // These are deliberately only journal/attribution fixtures. They grant no native command authority.
    sqlx::query("INSERT INTO thread_tool_calls(answer_id,thread_id,turn_id,segment_id,continuation_id,execution_revision,tool,arguments_hash,plan) VALUES($1,$2,$3,$4,$5,1,'image_generation',$6,$7)")
        .bind(answer).bind(requester).bind(new_id()).bind(new_id()).bind(new_id())
        .bind(orch_types::Hash::of(b"attribution-only").as_bytes().as_slice())
        .bind(json!({"delegated_capability":{"helper":helper.thread,"actor":helper.person,"capability":"image_generation"}}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE thread_inputs SET submission_authority=$2 WHERE id=(SELECT input_id FROM turns WHERE id=$1)")
        .bind(helper.turn).bind(json!({"capability_parent":{"answer":answer,"helper":helper.thread}}))
        .execute(&mut *conn).await.unwrap();
    let attribution = attribution_of_lease(&mut conn, helper.lease)
        .await
        .unwrap()
        .unwrap();
    let id = new_id();
    let mut event = a_request(id, &attribution, used(100, 0, 10), true);
    event.origin = ledger::origin::HARNESS;
    assert!(record(&mut conn, &event).await.unwrap());
    assert!(!record(&mut conn, &event).await.unwrap());
    let actual: (Id, Id, Id, String, Id) = sqlx::query_as(
        "SELECT thread_id,turn_id,provider_id,purpose,work_node_id FROM usage_events WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        actual,
        (
            helper.thread,
            helper.turn,
            helper.provider,
            "image_generation".into(),
            requester
        )
    );
    assert!(
        ledger::has_harness_requests(&mut conn, helper.turn)
            .await
            .unwrap()
    );
    assert_eq!(
        ledger::counted_request_totals(&mut conn, helper.turn)
            .await
            .unwrap(),
        used(100, 0, 10)
    );
    let cap = new_id();
    sqlx::query("INSERT INTO usage_caps(id,node_id,meter,period,amount,created_by,purposes) VALUES($1,$2,'tokens','day',1000,$3,ARRAY['image_generation'])")
        .bind(cap).bind(requester).bind(helper.person).execute(&mut *conn).await.unwrap();
    let cap = orch_db::usage::caps::get(&mut conn, cap)
        .await
        .unwrap()
        .unwrap();
    let used = orch_db::usage::caps::used(&mut conn, &cap, None, None, Utc::now())
        .await
        .unwrap();
    assert_eq!(used.iter().map(|row| row.total_tokens).sum::<i64>(), 110);
    let before: Json = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(d) ORDER BY d.day,d.thread_id,d.purpose) FROM usage_daily d",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    orch_db::usage::rollup::rebuild(&mut conn, None)
        .await
        .unwrap();
    let after: Json = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(d) ORDER BY d.day,d.thread_id,d.purpose) FROM usage_daily d",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "folding and rebuilding retain the identical separate scope without a second charge"
    );
}
