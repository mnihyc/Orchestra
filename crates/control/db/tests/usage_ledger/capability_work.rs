use super::*;

#[tokio::test]
async fn concurrent_recovery_keeps_one_work_identity_and_its_original_backend() {
    use orch_db::queries::capability_work::{self, NewProgramWork};
    let (_test, pool) = world().await;
    let helper = chain(&mut pool.acquire().await.unwrap(), true).await;
    let answer = new_id();
    sqlx::query("INSERT INTO thread_tool_calls(answer_id,thread_id,turn_id,segment_id,continuation_id,execution_revision,tool,arguments_hash) VALUES($1,$2,$3,$4,$5,1,'image_generation',$6)")
        .bind(answer).bind(helper.thread).bind(helper.turn).bind(helper.segment).bind(new_id())
        .bind(orch_types::Hash::of(b"one real accepted source").as_bytes().as_slice())
        .execute(pool.pool()).await.unwrap();
    let backend = json!({"route":"image_generation.delegation","offering":{"provider":helper.provider,"offering":"model"}});
    let record = json!({"original":"source-bound record"});
    let now = Utc::now();
    let work = NewProgramWork {
        id: new_id(),
        source_answer: answer,
        attempt: new_id(),
        helper: new_id(),
        actor: helper.person,
        node: helper.thread,
        purpose: "image_generation",
        backend: &backend,
        record: &record,
        at: now,
        deadline: now + chrono::Duration::minutes(5),
    };
    let mut first = pool.acquire().await.unwrap();
    let mut second = pool.acquire().await.unwrap();
    let (one, two) = tokio::join!(
        capability_work::retain_program(&mut first, &work),
        capability_work::retain_program(&mut second, &work)
    );
    let one = one.unwrap().unwrap();
    let two = two.unwrap().unwrap();
    assert_eq!(one.id, two.id);
    assert_eq!(one.record, record);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM delegated_program_work WHERE source_answer_id=$1")
            .bind(answer)
            .fetch_one(&mut *first)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let other_backend = json!({"route":"another-bill"});
    let changed = NewProgramWork {
        backend: &other_backend,
        ..work
    };
    let replay = capability_work::retain_program(&mut first, &changed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        replay.backend, backend,
        "recovery never edits the accepted backend"
    );
    assert_eq!(replay.record, record);
    assert_eq!(replay.deadline, one.deadline);
}

#[tokio::test]
async fn a_work_claim_binds_only_its_real_initial_input_and_cannot_be_replaced() {
    use orch_db::queries::capability_work::{self, NewProgramWork};
    let (_test, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
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
    sqlx::query("INSERT INTO thread_tool_calls(answer_id,thread_id,turn_id,segment_id,continuation_id,execution_revision,tool,arguments_hash) VALUES($1,$2,$3,$4,$5,1,'image_generation',$6)")
        .bind(answer).bind(requester).bind(helper.turn).bind(helper.segment).bind(new_id())
        .bind(orch_types::Hash::of(b"initial-input claim").as_bytes().as_slice())
        .execute(&mut *conn).await.unwrap();
    let now = Utc::now();
    let body = json!({});
    let work = NewProgramWork {
        id: new_id(),
        source_answer: answer,
        attempt: new_id(),
        helper: helper.thread,
        actor: helper.person,
        node: requester,
        purpose: "image_generation",
        backend: &body,
        record: &body,
        at: now,
        deadline: now + chrono::Duration::minutes(5),
    };
    capability_work::retain_program(&mut conn, &work)
        .await
        .unwrap()
        .unwrap();
    let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(helper.turn)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let wrong = new_id();
    assert!(
        !capability_work::bind_program_input(&mut conn, work.id, wrong)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE thread_inputs SET client_id=$2,submission_authority=submission_authority || $3 WHERE id=$1")
        .bind(input).bind(format!("capability-helper:{}",work.attempt)).bind(json!({"capability_parent":{"work":work.id,"helper":helper.thread}}))
        .execute(&mut *conn).await.unwrap();
    assert!(
        capability_work::bind_program_input(&mut conn, work.id, input)
            .await
            .unwrap()
    );
    assert!(
        capability_work::bind_program_input(&mut conn, work.id, input)
            .await
            .unwrap()
    );
    assert!(
        !capability_work::bind_program_input(&mut conn, work.id, wrong)
            .await
            .unwrap()
    );
    assert_eq!(
        capability_work::program(&mut conn, work.id)
            .await
            .unwrap()
            .unwrap()
            .input,
        Some(input)
    );
}

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
    sqlx::query("INSERT INTO thread_tool_calls(answer_id,thread_id,turn_id,segment_id,continuation_id,execution_revision,tool,arguments_hash) VALUES($1,$2,$3,$4,$5,1,'image_generation',$6)")
        .bind(answer).bind(requester).bind(new_id()).bind(new_id()).bind(new_id())
        .bind(orch_types::Hash::of(b"attribution-only").as_bytes().as_slice())
        .execute(&mut *conn).await.unwrap();
    let mut expected_authority: Json = sqlx::query_scalar(
        "SELECT submission_authority FROM thread_inputs WHERE id=(SELECT input_id FROM turns WHERE id=$1)",
    )
    .bind(helper.turn)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let work = new_id();
    let attempt = new_id();
    let now = Utc::now();
    orch_db::queries::capability_work::retain_program(
        &mut conn,
        &orch_db::queries::capability_work::NewProgramWork {
            id: work,
            source_answer: answer,
            attempt,
            helper: helper.thread,
            actor: helper.person,
            node: requester,
            purpose: "image_generation",
            backend: &json!({}),
            record: &json!({}),
            at: now,
            deadline: now + chrono::Duration::minutes(5),
        },
    )
    .await
    .unwrap()
    .unwrap();
    let parent = json!({"work":work,"helper":helper.thread});
    expected_authority["capability_parent"] = parent.clone();
    // Attribution extends the input's real credential origin; it cannot replace that origin.
    let actual_authority: Json = sqlx::query_scalar("UPDATE thread_inputs SET submission_authority=submission_authority || $2 WHERE id=(SELECT input_id FROM turns WHERE id=$1) RETURNING submission_authority")
        .bind(helper.turn).bind(json!({"capability_parent":parent}))
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(actual_authority, expected_authority);
    let input: Id = sqlx::query_scalar("UPDATE thread_inputs SET client_id=$2 WHERE id=(SELECT input_id FROM turns WHERE id=$1) RETURNING id")
        .bind(helper.turn).bind(format!("capability-helper:{attempt}")).fetch_one(&mut *conn).await.unwrap();
    assert!(
        orch_db::queries::capability_work::bind_program_input(&mut conn, work, input)
            .await
            .unwrap()
    );
    sqlx::query("DELETE FROM thread_tool_calls WHERE answer_id=$1")
        .bind(answer)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        orch_db::queries::capability_work::program(&mut conn, work)
            .await
            .unwrap()
            .unwrap()
            .source_answer
            .is_none(),
        "removing the source ends its live association without erasing factual usage scope"
    );
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
